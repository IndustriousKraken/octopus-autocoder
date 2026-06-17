use super::*;

// ====================================================================
// Audit-triage processing (audit-reply-acts `send it` flow)
// ====================================================================

/// Process every queued audit-triage `thread_ts` for this repo. The
/// caller passes the per-repo queue snapshot already drained; this
/// function loads each `AuditThreadState`, runs the executor in triage
/// mode, discards non-spec writes, and opens at most one spec PR (a43).
///
/// Failures inside one triage do NOT abort the others — each entry is
/// processed independently, errors are logged and the audit-thread
/// state's `status` is updated to `TriageFailed` so the operator can
/// retry via `@<bot> send it` again.
pub async fn process_audit_triages(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    executor: &dyn Executor,
    github_cfg: &GithubConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    thread_tses: &[String],
) -> Result<()> {
    use crate::audits::threads;
    // Workspace must be clean and on a fresh agent_branch off base
    // before we let the executor loose on it. The downstream
    // `run_pass_through_commits` does the same setup; we duplicate it
    // here because triage runs OUTSIDE the normal pass and leaves the
    // workspace in whatever state the executor produces.
    let fork_url = match github_cfg.fork_owner.as_deref() {
        Some(owner) => Some(crate::github::derive_fork_url(&repo.url, owner)?),
        None => None,
    };
    let fork_arg = fork_url.as_deref().map(|u| (u, repo.agent_branch.as_str()));
    crate::workspace::ensure_initialized(paths, workspace, &repo.url, fork_arg)
        .with_context(|| "audit-triage: workspace ensure_initialized".to_string())?;
    let _ = crate::queue::clear_stale_locks(workspace);
    let _ = git::reset_hard_head(workspace);
    let _ = git::clean_force(workspace);
    git::fetch(workspace).with_context(|| "audit-triage: git fetch".to_string())?;
    git::checkout(workspace, &repo.base_branch)
        .with_context(|| format!("audit-triage: checkout `{}`", repo.base_branch))?;
    git::pull_ff_only(workspace, &repo.base_branch)
        .with_context(|| format!("audit-triage: pull --ff-only `{}`", repo.base_branch))?;
    git::recreate_branch(workspace, &repo.agent_branch)
        .with_context(|| format!("audit-triage: recreate `{}`", repo.agent_branch))?;

    for thread_ts in thread_tses {
        let state_root = threads::default_state_root(paths);
        let mut state = match threads::read_state(&state_root, thread_ts) {
            Ok(Some(s)) => s,
            Ok(None) => {
                tracing::warn!(
                    thread_ts = %thread_ts,
                    "audit-triage: no state file (entry pruned between trigger and processing); skipping"
                );
                continue;
            }
            Err(e) => {
                tracing::warn!(
                    thread_ts = %thread_ts,
                    "audit-triage: state read failed: {e:#}"
                );
                continue;
            }
        };

        // Build the canonical-specs index from openspec/specs/<name>/.
        let canonical_specs_index = build_canonical_specs_index(workspace);
        let ctx = crate::executor::TriageContext {
            findings: state.findings_excerpt.clone(),
            audit_type: state.audit_type.clone(),
            repo_url: state.repo_url.clone(),
            canonical_specs_index,
        };

        tracing::info!(
            url = %repo.url,
            thread_ts = %thread_ts,
            audit_type = %state.audit_type,
            "audit-triage: invoking executor in triage mode"
        );

        let outcome = executor.run_triage(workspace, &ctx).await;
        match outcome {
            Ok(crate::executor::ExecutorOutcome::Completed { final_answer }) => {
                if let Err(e) = process_completed_triage(
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
                        thread_ts = %thread_ts,
                        "audit-triage: post-Completed processing failed: {e:#}"
                    );
                    mark_triage_failed(
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
                    thread_ts = %thread_ts,
                    "audit-triage: executor returned Failed: {reason}"
                );
                mark_triage_failed(paths, &state_root, &mut state, reason, chatops_ctx).await;
            }
            Ok(crate::executor::ExecutorOutcome::PreconditionUnmet { reason }) => {
                // a74: surfaced only on the revise path today; triage is out of
                // scope. Treat it as a failure (defensive — never produced here
                // at runtime).
                tracing::error!(
                    url = %repo.url,
                    thread_ts = %thread_ts,
                    "audit-triage: executor returned PreconditionUnmet: {reason}"
                );
                mark_triage_failed(paths, &state_root, &mut state, reason, chatops_ctx).await;
            }
            Ok(crate::executor::ExecutorOutcome::AskUser { .. }) => {
                // Triage's escalation: the agent asked a question. The
                // existing chatops escalation machinery is per-change;
                // for triage we treat AskUser as a no-op (status stays
                // TriagePending so a future iteration could retry).
                tracing::info!(
                    url = %repo.url,
                    thread_ts = %thread_ts,
                    "audit-triage: executor returned AskUser; leaving status TriagePending"
                );
            }
            Ok(crate::executor::ExecutorOutcome::SpecNeedsRevision { .. }) => {
                tracing::warn!(
                    url = %repo.url,
                    thread_ts = %thread_ts,
                    "audit-triage: executor returned SpecNeedsRevision; treating as failure"
                );
                mark_triage_failed(
                    paths,
                    &state_root,
                    &mut state,
                    "executor flagged SpecNeedsRevision during triage".to_string(),
                    chatops_ctx,
                )
                .await;
            }
            Ok(crate::executor::ExecutorOutcome::IterationRequested { .. }) => {
                tracing::warn!(
                    url = %repo.url,
                    thread_ts = %thread_ts,
                    "audit-triage: executor returned IterationRequested; treating as failure (iteration sequences not applicable to triage mode)"
                );
                mark_triage_failed(
                    paths,
                    &state_root,
                    &mut state,
                    "executor returned IterationRequested during triage".to_string(),
                    chatops_ctx,
                )
                .await;
            }
            Ok(crate::executor::ExecutorOutcome::Aborted { reason }) => {
                // a39: subprocess killed by the daemon's own SIGTERM
                // cascade. Leave state at TriagePending so the next
                // iteration after restart retries the triage; do NOT
                // mark_triage_failed (operator initiated the shutdown).
                tracing::info!(
                    url = %repo.url,
                    thread_ts = %thread_ts,
                    "audit-triage: executor aborted by daemon shutdown: {reason}"
                );
            }
            Err(e) => {
                tracing::error!(
                    url = %repo.url,
                    thread_ts = %thread_ts,
                    "audit-triage: executor task errored: {e:#}"
                );
                mark_triage_failed(
                    paths,
                    &state_root,
                    &mut state,
                    format!("executor task error: {e:#}"),
                    chatops_ctx,
                )
                .await;
            }
        }
        // After triage (success or failure), reset to clean working tree
        // so the next operation isn't contaminated by triage leftovers.
        // best-effort — failures are logged but never propagated.
        if let Err(e) = git::reset_hard_head(workspace) {
            tracing::warn!(
                url = %repo.url,
                "audit-triage: post-triage reset_hard_head failed: {e:#}"
            );
        }
        let _ = git::clean_force(workspace);
        // Move back to base branch so subsequent steps in the iteration
        // start from a known state.
        let _ = git::checkout(workspace, &repo.base_branch);
    }
    Ok(())
}

/// Inspect the changed paths in `workspace` after a Completed triage and
/// open AT MOST ONE PR — the spec PR (a43). Code-path writes outside
/// `openspec/changes/<derived-slug>/` are discarded before the commit so
/// the spec PR's diff is genuinely spec-only; the dropped paths are
/// logged AND surfaced to chatops. On the empty-diff path, post the
/// agent's final-summary text into the audit thread reply chain and flip
/// the state to `Acted`. `final_summary` carries the executor's
/// final-answer text (used for the empty-diff reply).
pub(crate) async fn process_completed_triage(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    state: &mut crate::audits::threads::AuditThreadState,
    final_summary: Option<&str>,
) -> Result<()> {
    use crate::audits::threads::{self, AuditThreadStatus};
    let state_root = threads::default_state_root(paths);

    let changed: Vec<String> = git::status_entries(workspace)
        .with_context(|| "audit-triage: reading post-Completed git status".to_string())?
        .into_iter()
        .flat_map(|e| std::iter::once(e.path).chain(e.orig_path))
        .collect();

    // A stable slug derived from `<audit_type>-<short_hash>`, retained
    // purely as a diagnostic label for logs (the executor picks its own
    // output-directory name; the keep boundary is each planning lane's
    // root, NOT this slug).
    let new_slug = derive_unique_triage_slug(workspace, &state.audit_type, &state.findings_excerpt);

    // Triage produces ONE PR — the spec PR OR the issue PR (whichever lane
    // the run wrote for). Out-of-lane writes (code, docs, config) are
    // discarded before commit; implementation flows through the standard
    // pipeline on a later iteration after the operator merges the PR.
    let push_remote = if github_cfg.fork_owner.is_some() {
        "fork"
    } else {
        "origin"
    };
    let agent_branch = &repo.agent_branch;
    let base_branch = &repo.base_branch;

    let was_empty = changed.is_empty();
    let issues_prefix = format!("{}/", crate::lanes::issues::ISSUES_SUBDIR);
    let has_spec = changed.iter().any(|p| p.starts_with("openspec/changes/"));
    let has_issue = changed.iter().any(|p| p.starts_with(&issues_prefix));

    // Discard every out-of-lane write so the PR's diff carries only
    // planning-lane content (spec lane OR issues lane).
    let discarded = discard_non_spec_writes(workspace, &new_slug)
        .with_context(|| "audit-triage: discarding out-of-lane writes".to_string())?;
    if !discarded.is_empty() {
        tracing::warn!(
            url = %repo.url,
            audit_type = %state.audit_type,
            slug = %new_slug,
            dropped = ?discarded,
            "audit-triage: discarded out-of-lane writes (planning-lane-only enforcement)"
        );
    }

    // No content in EITHER lane → no PR. Distinguish "nothing produced"
    // (empty diff → Acted) from "only code, now dropped" (→ TriageFailed).
    if !has_spec && !has_issue {
        triage_reply_no_content(chatops_ctx, state, &state_root, was_empty, final_summary).await;
        return Ok(());
    }

    // Spec lane takes priority if both were (unexpectedly) written; the
    // default for a behavior-preserving refactor is the issue lane.
    let lane_is_spec = has_spec;
    let lane_label = if lane_is_spec { "spec" } else { "issue" };

    // If the agent also wrote out-of-lane paths (now discarded), warn the
    // operator so the dropped fixes can be captured as tasks.md items.
    if !discarded.is_empty()
        && let Some(ctx) = chatops_ctx
    {
        let body = format!(
            "⚠️ The triage agent attempted to write {n} path(s) outside the planning lanes: {list}. \
            Code fixes go through the standard implementer pipeline. The {lane_label} PR has been opened; \
            if the dropped fixes were load-bearing, revise the {lane_label} to capture them as tasks.md items.",
            n = discarded.len(),
            list = discarded.join(", "),
        );
        if let Err(e) = ctx
            .chatops
            .post_threaded_reply(&state.channel, &state.thread_ts, &body)
            .await
        {
            tracing::warn!(
                thread_ts = %state.thread_ts,
                "audit-triage: dropped-paths thread reply failed: {e:#}"
            );
        }
    }

    git::checkout(workspace, base_branch)
        .with_context(|| format!("audit-triage: checkout base branch `{base_branch}`"))?;
    let branch = format!("{agent_branch}-triage-spec");
    git::recreate_branch(workspace, &branch)
        .with_context(|| format!("audit-triage: recreate `{branch}`"))?;
    git::add_all(workspace).with_context(|| "audit-triage: staging planning-lane paths".to_string())?;
    let subject = if lane_is_spec {
        format!("audit-triage spec proposal from {}", state.audit_type)
    } else {
        format!("audit-triage issue from {}", state.audit_type)
    };
    git::commit(workspace, &subject)
        .with_context(|| "audit-triage: commit triage branch".to_string())?;
    if let Err(e) = git::push_force_with_lease(workspace, &branch, push_remote) {
        return Err(anyhow!("audit-triage: pushing triage branch failed: {e:#}"));
    }
    let body = if lane_is_spec {
        format!(
            "This PR carries the new spec change(s) from the `{at}` audit on `{ru}`. \
            After merge, the next polling iteration's implementer will produce the code fixes through the standard pipeline.",
            at = state.audit_type,
            ru = state.repo_url,
        )
    } else {
        format!(
            "This PR carries the new issue(s) from the `{at}` audit on `{ru}` — a behavior-preserving refactor. \
            After merge, the issues-lane walker picks up the issue and implements it through the standard pipeline.",
            at = state.audit_type,
            ru = state.repo_url,
        )
    };
    let pr_title = format!("audit-triage {lane_label} ({})", state.audit_type);
    let pr_url = match open_triage_pull_request(
        paths,
        repo,
        github_cfg,
        &branch,
        base_branch,
        &pr_title,
        &body,
    )
    .await
    {
        Ok(url) => Some(url),
        Err(e) => {
            tracing::error!(url = %repo.url, "audit-triage: {lane_label} PR creation failed: {e:#}");
            None
        }
    };

    if let Some(ctx) = chatops_ctx
        && let Some(u) = &pr_url
    {
        let reply = format!(
            "✓ Triage for `{}` complete.\n{} PR: {u}",
            state.audit_type,
            if lane_is_spec { "Spec" } else { "Issue" },
        );
        let _ = ctx
            .chatops
            .post_threaded_reply(&state.channel, &state.thread_ts, &reply)
            .await;
    }

    state.status = AuditThreadStatus::Acted;
    let _ = threads::write_state(&state_root, state);
    Ok(())
}

/// Post the "no actionable / no planning-lane content" triage thread reply
/// and set the terminal status. Extracted from `process_completed_triage`.
async fn triage_reply_no_content(
    chatops_ctx: Option<&ChatOpsContext>,
    state: &mut crate::audits::threads::AuditThreadState,
    state_root: &std::path::Path,
    was_empty: bool,
    final_summary: Option<&str>,
) {
    use crate::audits::threads::{self, AuditThreadStatus};
    // No spec OR issue content survived the discard. Distinguish "nothing
    // was produced" (empty diff → Acted) from "only code, now dropped"
    // (code-only → TriageFailed, retryable).
    if let Some(ctx) = chatops_ctx {
        let body = if was_empty {
            match final_summary.map(str::trim).filter(|s| !s.is_empty()) {
                Some(summary) => format!(
                    "ℹ️ Triage for `{at}` on `{ru}` completed with no actionable changes.\n\n{summary}",
                    at = state.audit_type,
                    ru = state.repo_url,
                ),
                None => format!(
                    "ℹ️ Triage for `{at}` on `{ru}` completed with no actionable changes.",
                    at = state.audit_type,
                    ru = state.repo_url,
                ),
            }
        } else {
            format!(
                "ℹ️ Triage for `{at}` on `{ru}`: no spec or issue content produced; retry with a clearer directive.",
                at = state.audit_type,
                ru = state.repo_url,
            )
        };
        if let Err(e) = ctx
            .chatops
            .post_threaded_reply(&state.channel, &state.thread_ts, &body)
            .await
        {
            tracing::warn!(
                thread_ts = %state.thread_ts,
                "audit-triage: no-PR thread reply failed: {e:#}"
            );
        }
    }
    state.status = if was_empty {
        AuditThreadStatus::Acted
    } else {
        AuditThreadStatus::TriageFailed
    };
    let _ = threads::write_state(state_root, state);
}

/// Derive a unique `openspec/changes/<slug>/` path for a triage run.
/// The slug is `<audit_type-sanitized>-<short-hash>`; if it already
/// exists on disk, we append `-2`, `-3`, ... until we find a free path.
fn derive_unique_triage_slug(workspace: &Path, audit_type: &str, findings: &str) -> String {
    let mut sanitized: String = audit_type
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    sanitized = sanitized.trim_matches('-').to_string();
    if sanitized.is_empty() {
        sanitized = "triage".to_string();
    }
    // Short hash: first 8 hex chars of a non-crypto fold over the
    // findings string. Deterministic per identical findings, so re-running
    // the same `send it` reuses the same slug instead of forking a new one.
    let hash = short_findings_hash(findings);
    let base_slug = format!("{sanitized}-{hash}");
    let mut slug = base_slug.clone();
    let mut suffix = 2u32;
    while workspace.join("openspec/changes").join(&slug).exists() {
        slug = format!("{base_slug}-{suffix}");
        suffix += 1;
        if suffix > 100 {
            // Pathological case: bail out with whatever we have.
            break;
        }
    }
    slug
}

/// 8-hex-char fold over `findings`. Not cryptographic — only used as a
/// slug uniqueness suffix.
pub(crate) fn short_findings_hash(findings: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    for b in findings.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3); // FNV prime
    }
    format!("{:08x}", h as u32)
}

