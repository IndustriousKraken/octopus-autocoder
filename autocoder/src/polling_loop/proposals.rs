use super::*;

/// Drain handler for chat-driven proposal requests. The polling loop's
/// `run` calls this once per iteration with the per-iteration drained
/// queue snapshot. Each entry loads its `ProposalRequestState`, runs
/// the chat-triage executor, and routes the outcome through:
///   - QUESTION → post `.chat-reply.md` contents to the lifecycle
///     thread, set status to `Discussed`.
///   - DIRECTIVE → discard non-spec writes and open at most one spec PR
///     (a43; reusing the same helper that powers `audit-reply-acts`),
///     set status to `Acted`.
///   - AskUser → leave status at `TriagePending` (existing chatops
///     escalation posts the question into the lifecycle thread).
///   - Failed → post a failure reply, set status to `TriageFailed`.
///
/// Failures inside one entry do NOT abort the others — each is processed
/// independently.
pub async fn process_proposal_requests(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    executor: &dyn Executor,
    github_cfg: &GithubConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    requests: &[crate::control_socket::ProposalRequest],
) -> Result<()> {
    // Workspace preparation mirrors the audit-triage path: ensure clean
    // base branch checkout, recreate the agent branch, so the chat-triage
    // executor sees a known state. The downstream pass-through uses the
    // same convention; we duplicate it here because chat-triage runs
    // OUTSIDE the normal pass.
    let fork_url = match github_cfg.fork_owner.as_deref() {
        Some(owner) => Some(crate::github::derive_fork_url(&repo.url, owner)?),
        None => None,
    };
    let fork_arg = fork_url.as_deref().map(|u| (u, repo.agent_branch.as_str()));
    crate::workspace::ensure_initialized(paths, workspace, &repo.url, fork_arg)
        .with_context(|| "chat-triage: workspace ensure_initialized".to_string())?;
    let _ = crate::queue::clear_stale_locks(workspace);
    let _ = git::reset_hard_head(workspace);
    let _ = git::clean_force(workspace);
    git::fetch(workspace).with_context(|| "chat-triage: git fetch".to_string())?;
    git::checkout(workspace, &repo.base_branch)
        .with_context(|| format!("chat-triage: checkout `{}`", repo.base_branch))?;
    git::pull_ff_only(workspace, &repo.base_branch)
        .with_context(|| format!("chat-triage: pull --ff-only `{}`", repo.base_branch))?;
    git::recreate_branch(workspace, &repo.agent_branch)
        .with_context(|| format!("chat-triage: recreate `{}`", repo.agent_branch))?;

    let state_root = crate::proposal_requests::default_state_root(paths);
    for request in requests {
        let mut state = match crate::proposal_requests::read_state(
            &state_root,
            &repo.url,
            &request.request_id,
        ) {
            Ok(Some(s)) => s,
            Ok(None) => {
                tracing::warn!(
                    request_id = %request.request_id,
                    "chat-triage: no state file (entry pruned between enqueue and processing); skipping"
                );
                continue;
            }
            Err(e) => {
                tracing::warn!(
                    request_id = %request.request_id,
                    "chat-triage: state read failed: {e:#}"
                );
                continue;
            }
        };

        // Flip Pending → TriagePending up front so a daemon crash mid-
        // run is observable on disk.
        state.status = crate::proposal_requests::ProposalRequestStatus::TriagePending;
        let _ = crate::proposal_requests::write_state(&state_root, &state);

        let canonical_specs_index = build_canonical_specs_index(workspace);
        let ctx = crate::executor::ChatTriageContext {
            request_text: state.request_text.clone(),
            repo_url: state.repo_url.clone(),
            canonical_specs_index,
        };

        tracing::info!(
            url = %repo.url,
            request_id = %state.request_id,
            "chat-triage: invoking executor"
        );
        let outcome = executor.run_chat_triage(workspace, &ctx).await;
        match outcome {
            Ok(crate::executor::ExecutorOutcome::Completed { final_answer }) => {
                if let Err(e) = process_completed_proposal(
                    paths,
                    workspace,
                    repo,
                    github_cfg,
                    chatops_ctx,
                    &mut state,
                    final_answer.as_deref(),
                )
                .await
                {
                    tracing::error!(
                        url = %repo.url,
                        request_id = %state.request_id,
                        "chat-triage: post-Completed processing failed: {e:#}"
                    );
                    mark_proposal_failed(
                        paths,
                        &state_root,
                        &mut state,
                        format!("post-Completed processing: {e:#}"),
                        chatops_ctx,
                    )
                    .await;
                }
            }
            Ok(crate::executor::ExecutorOutcome::Failed { reason }) => {
                tracing::error!(
                    url = %repo.url,
                    request_id = %state.request_id,
                    "chat-triage: executor returned Failed: {reason}"
                );
                mark_proposal_failed(paths, &state_root, &mut state, reason, chatops_ctx).await;
            }
            Ok(crate::executor::ExecutorOutcome::AskUser { .. }) => {
                tracing::info!(
                    url = %repo.url,
                    request_id = %state.request_id,
                    "chat-triage: executor returned AskUser; leaving status TriagePending"
                );
            }
            Ok(crate::executor::ExecutorOutcome::SpecNeedsRevision { .. }) => {
                tracing::warn!(
                    url = %repo.url,
                    request_id = %state.request_id,
                    "chat-triage: executor returned SpecNeedsRevision; treating as failure"
                );
                mark_proposal_failed(
                    paths,
                    &state_root,
                    &mut state,
                    "executor flagged SpecNeedsRevision during chat-triage".to_string(),
                    chatops_ctx,
                )
                .await;
            }
            Ok(crate::executor::ExecutorOutcome::IterationRequested { .. }) => {
                tracing::warn!(
                    url = %repo.url,
                    request_id = %state.request_id,
                    "chat-triage: executor returned IterationRequested; treating as failure (iteration sequences not applicable to chat-triage mode)"
                );
                mark_proposal_failed(
                    paths,
                    &state_root,
                    &mut state,
                    "executor returned IterationRequested during chat-triage".to_string(),
                    chatops_ctx,
                )
                .await;
            }
            Ok(crate::executor::ExecutorOutcome::Aborted { reason }) => {
                // a39: subprocess killed by the daemon's own SIGTERM
                // cascade. Leave state at TriagePending so the next
                // iteration after restart retries; do NOT
                // mark_proposal_failed (operator initiated the
                // shutdown).
                tracing::info!(
                    url = %repo.url,
                    request_id = %state.request_id,
                    "chat-triage: executor aborted by daemon shutdown: {reason}"
                );
            }
            Err(e) => {
                tracing::error!(
                    url = %repo.url,
                    request_id = %state.request_id,
                    "chat-triage: executor task errored: {e:#}"
                );
                mark_proposal_failed(
                    paths,
                    &state_root,
                    &mut state,
                    format!("executor task error: {e:#}"),
                    chatops_ctx,
                )
                .await;
            }
        }
        // Always reset to clean working tree so the next operation isn't
        // contaminated by leftovers. Best-effort — failures are logged.
        if let Err(e) = git::reset_hard_head(workspace) {
            tracing::warn!(
                url = %repo.url,
                "chat-triage: post-run reset_hard_head failed: {e:#}"
            );
        }
        let _ = git::clean_force(workspace);
        let _ = git::checkout(workspace, &repo.base_branch);
    }
    Ok(())
}

/// Handle a `Completed` chat-triage outcome. Checks for the
/// `.chat-reply.md` marker FIRST; if present, posts the contents to the
/// lifecycle thread and flips to `Discussed`. Otherwise discards non-spec
/// writes and opens AT MOST ONE PR — the spec PR (a43) — identical in
/// shape to the audit-triage handler. `final_summary` carries the
/// executor's final-answer text (used for the empty-diff reply).
pub(crate) async fn process_completed_proposal(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    state: &mut crate::proposal_requests::ProposalRequestState,
    final_summary: Option<&str>,
) -> Result<()> {
    use crate::proposal_requests::{self, ProposalRequestStatus};
    let state_root = proposal_requests::default_state_root(paths);

    // Marker-file check: a `.chat-reply.md` means the LLM classified the
    // request as a QUESTION; handle it inline and return early on a reply.
    if handle_chat_reply_marker(workspace, chatops_ctx, state, &state_root).await? {
        return Ok(());
    }

    // 2. No `.chat-reply.md`. a43: produce a SPEC-ONLY PR. Code-path
    //    writes are discarded before commit; implementation flows through
    //    the standard implementer pipeline on a later iteration after the
    //    operator merges the spec PR. Mirrors `process_completed_triage`.
    let changed: Vec<String> = git::status_entries(workspace)
        .with_context(|| "chat-triage: reading post-Completed git status".to_string())?
        .into_iter()
        .flat_map(|e| std::iter::once(e.path).chain(e.orig_path))
        .collect();

    // Stable diagnostic label only; the spec/code boundary is the
    // universal `openspec/changes/` root, NOT this slug (the executor
    // picks its own change-directory name).
    let new_slug = derive_unique_chat_request_slug(workspace, &state.request_text);

    let was_empty = changed.is_empty();
    let has_spec = changed.iter().any(|p| p.starts_with("openspec/changes/"));

    let push_remote = if github_cfg.fork_owner.is_some() {
        "fork"
    } else {
        "origin"
    };
    let agent_branch = &repo.agent_branch;
    let base_branch = &repo.base_branch;

    // Discard every non-spec write so the spec PR's diff is spec-only.
    let discarded = discard_non_spec_writes(workspace, &new_slug)
        .with_context(|| "chat-triage: discarding non-spec writes".to_string())?;
    if !discarded.is_empty() {
        tracing::warn!(
            url = %repo.url,
            request_id = %state.request_id,
            slug = %new_slug,
            dropped = ?discarded,
            "chat-triage: discarded non-spec writes (a43 spec-only enforcement)"
        );
    }

    if !has_spec {
        reply_no_spec(chatops_ctx, state, &state_root, was_empty, final_summary).await;
        return Ok(());
    }

    // Spec content exists → open exactly one PR (the spec PR). If the
    // agent also wrote code (now discarded), warn the operator so the
    // dropped fixes can be captured as tasks.md items if load-bearing.
    if !discarded.is_empty()
        && let Some(ctx) = chatops_ctx
    {
        let body = format!(
            "⚠️ The triage agent attempted to write {n} path(s) outside `openspec/changes/`: {list}. \
            Per a43, code fixes go through the standard implementer pipeline. The spec PR has been opened; \
            if the dropped fixes were load-bearing, revise the spec to capture them as tasks.md items.",
            n = discarded.len(),
            list = discarded.join(", "),
        );
        if let Err(e) = ctx
            .chatops
            .post_threaded_reply(&state.channel, &state.thread_ts, &body)
            .await
        {
            tracing::warn!(
                request_id = %state.request_id,
                "chat-triage: dropped-paths thread reply failed: {e:#}"
            );
        }
    }

    git::checkout(workspace, base_branch)
        .with_context(|| format!("chat-triage: checkout base branch `{base_branch}`"))?;
    let spec_branch = format!("{agent_branch}-chat-spec");
    git::recreate_branch(workspace, &spec_branch)
        .with_context(|| format!("chat-triage: recreate `{spec_branch}`"))?;
    git::add_all(workspace).with_context(|| "chat-triage: staging spec paths".to_string())?;
    let subject = format!("chat-triage spec proposal (request {})", state.request_id);
    git::commit(workspace, &subject)
        .with_context(|| "chat-triage: commit spec branch".to_string())?;
    if let Err(e) = git::push_force_with_lease(workspace, &spec_branch, push_remote) {
        return Err(anyhow!("chat-triage: pushing spec branch failed: {e:#}"));
    }
    let body = format!(
        "This PR carries the new spec change(s) from the `propose` request on `{repo_url}`. \
        After merge, the next polling iteration's implementer will produce the code fixes through the standard pipeline.\n\nOperator's request:\n\n> {request_excerpt}",
        repo_url = state.repo_url,
        request_excerpt = short_request_excerpt(&state.request_text),
    );
    let spec_pr_url = match open_triage_pull_request(
        paths,
        repo,
        github_cfg,
        &spec_branch,
        base_branch,
        &format!(
            "chat-triage spec ({})",
            short_request_excerpt(&state.request_text)
        ),
        &body,
    )
    .await
    {
        Ok(url) => Some(url),
        Err(e) => {
            tracing::error!(url = %repo.url, "chat-triage: spec PR creation failed: {e:#}");
            None
        }
    };

    if let Some(ctx) = chatops_ctx
        && let Some(u) = &spec_pr_url
    {
        let reply = format!("✓ Chat-triage complete.\nSpec PR: {u}");
        let _ = ctx
            .chatops
            .post_threaded_reply(&state.channel, &state.thread_ts, &reply)
            .await;
    }

    state.status = ProposalRequestStatus::Acted;
    let _ = proposal_requests::write_state(&state_root, state);
    Ok(())
}

/// Handle the `.chat-reply.md` QUESTION marker: post the threaded reply,
/// scrub stray writes, mark Discussed. Returns true when the caller should
/// return early. Extracted from `process_completed_proposal` (a68 split).
async fn handle_chat_reply_marker(
    workspace: &Path,
    chatops_ctx: Option<&ChatOpsContext>,
    state: &mut crate::proposal_requests::ProposalRequestState,
    state_root: &std::path::Path,
) -> Result<bool> {
    use crate::proposal_requests::{self, ProposalRequestStatus};
    let chat_reply_path = workspace.join(".chat-reply.md");
    if chat_reply_path.exists() {
        let contents = match std::fs::read_to_string(&chat_reply_path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    path = %chat_reply_path.display(),
                    "chat-triage: reading .chat-reply.md failed: {e}; treating as empty"
                );
                String::new()
            }
        };
        // Non-empty? Treat as a QUESTION outcome.
        if !contents.trim().is_empty() {
            let truncated = proposal_requests::truncate_chat_reply_with_pointer(
                &contents,
                &state.request_id,
                proposal_requests::CHAT_REPLY_BODY_CAP,
            );
            if let Some(ctx) = chatops_ctx
                && let Err(e) = ctx
                    .chatops
                    .post_threaded_reply(&state.channel, &state.thread_ts, &truncated)
                    .await
            {
                tracing::warn!(
                    request_id = %state.request_id,
                    "chat-triage: posting Discussed reply failed: {e:#}"
                );
            }
            // Best-effort: delete the marker.
            if let Err(e) = std::fs::remove_file(&chat_reply_path) {
                tracing::warn!(
                    path = %chat_reply_path.display(),
                    "chat-triage: removing .chat-reply.md failed: {e}"
                );
            }
            // Detect any OTHER modifications and WARN + revert.
            let unexpected: Vec<String> = git::status_entries(workspace)
                .unwrap_or_default()
                .into_iter()
                .flat_map(|e| std::iter::once(e.path).chain(e.orig_path))
                .filter(|p| !p.is_empty() && p != ".chat-reply.md")
                .collect();
            if !unexpected.is_empty() {
                tracing::warn!(
                    request_id = %state.request_id,
                    "chat-triage: Discussed-mode run produced unexpected modifications: {unexpected:?} — reverting"
                );
                let _ = git::reset_hard_head(workspace);
                let _ = git::clean_force(workspace);
            }
            state.status = ProposalRequestStatus::Discussed;
            let _ = proposal_requests::write_state(state_root, state);
            return Ok(true);
        }
        // Empty file: treat as "no reply"; fall through to the
        // diff-split path (likely an empty diff too, which posts the
        // no-action reply).
        let _ = std::fs::remove_file(&chat_reply_path);
    }
    Ok(false)
}

/// Post the "no actionable / no spec content" thread reply and set the
/// terminal status (Acted vs. TriageFailed). Extracted from
/// `process_completed_proposal` (a68 split).
async fn reply_no_spec(
    chatops_ctx: Option<&ChatOpsContext>,
    state: &mut crate::proposal_requests::ProposalRequestState,
    state_root: &std::path::Path,
    was_empty: bool,
    final_summary: Option<&str>,
) {
    use crate::proposal_requests::{self, ProposalRequestStatus};
    // No spec content survived the discard. Distinguish "nothing was
    // produced" (empty diff → Acted) from "only code, now dropped"
    // (code-only → TriageFailed, retryable).
    if let Some(ctx) = chatops_ctx {
        let body = if was_empty {
            match final_summary.map(str::trim).filter(|s| !s.is_empty()) {
                Some(summary) => format!(
                    "ℹ️ Chat-triage for `{ru}` completed with no actionable changes.\n\n{summary}",
                    ru = state.repo_url,
                ),
                None => format!(
                    "ℹ️ Chat-triage for `{ru}` completed with no actionable changes.",
                    ru = state.repo_url,
                ),
            }
        } else {
            format!(
                "ℹ️ Chat-triage for `{ru}` produced no spec content; retry with a clearer directive.",
                ru = state.repo_url,
            )
        };
        if let Err(e) = ctx
            .chatops
            .post_threaded_reply(&state.channel, &state.thread_ts, &body)
            .await
        {
            tracing::warn!(
                request_id = %state.request_id,
                "chat-triage: no-PR thread reply failed: {e:#}"
            );
        }
    }
    state.status = if was_empty {
        ProposalRequestStatus::Acted
    } else {
        ProposalRequestStatus::TriageFailed
    };
    let _ = proposal_requests::write_state(state_root, state);
}

/// Flip the proposal-request state to `TriageFailed` and post the
/// failure to the request's lifecycle thread. Best-effort — every
/// failure path here logs and continues.
async fn mark_proposal_failed(
    _paths: &DaemonPaths,
    state_root: &Path,
    state: &mut crate::proposal_requests::ProposalRequestState,
    reason: String,
    chatops_ctx: Option<&ChatOpsContext>,
) {
    use crate::proposal_requests::{self, ProposalRequestStatus};
    state.status = ProposalRequestStatus::TriageFailed;
    state.reason = Some(reason.clone());
    if let Err(e) = proposal_requests::write_state(state_root, state) {
        tracing::warn!(
            request_id = %state.request_id,
            "chat-triage: recording TriageFailed state failed: {e:#}"
        );
    }
    if let Some(ctx) = chatops_ctx {
        let body = format!(
            "✗ Chat-triage for `{repo_url}` failed: {reason}",
            repo_url = state.repo_url,
        );
        if let Err(e) = ctx
            .chatops
            .post_threaded_reply(&state.channel, &state.thread_ts, &body)
            .await
        {
            tracing::warn!(
                request_id = %state.request_id,
                "chat-triage: TriageFailed thread reply failed: {e:#}"
            );
        }
    }
}

/// Derive a unique `openspec/changes/<slug>/` path for a chat-triage
/// run. The slug is `chat-request-<short-hash-of-request-text>`; if it
/// already exists on disk, we append `-2`, `-3`, ... until we find a
/// free path.
fn derive_unique_chat_request_slug(workspace: &Path, request_text: &str) -> String {
    let hash = short_findings_hash(request_text);
    let base_slug = format!("chat-request-{hash}");
    let mut slug = base_slug.clone();
    let mut suffix = 2u32;
    while workspace.join("openspec/changes").join(&slug).exists() {
        slug = format!("{base_slug}-{suffix}");
        suffix += 1;
        if suffix > 100 {
            break;
        }
    }
    slug
}

/// Render a short single-line excerpt of the operator's request for PR
/// titles. Replaces internal newlines with spaces and truncates at 60
/// chars with a trailing `…`.
fn short_request_excerpt(request_text: &str) -> String {
    let one_line = request_text.replace('\n', " ");
    let cleaned: String = one_line.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.chars().count() <= 60 {
        cleaned
    } else {
        let mut out: String = cleaned.chars().take(60).collect();
        out.push('…');
        out
    }
}
