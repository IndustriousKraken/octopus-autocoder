use super::*;

pub(crate) async fn handle_outcome(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    change: &str,
    outcome: Result<ExecutorOutcome>,
) -> Result<QueueStep> {
    match outcome {
        Err(e) => {
            // Executor task error (e.g. spawn failure). This is closer to
            // an infrastructure flake than an agent-decided Failed, but
            // the architecture-foundation contract treats it as Failed and
            // we follow suit; the reason carries the error text.
            let reason = format!("{e:#}");
            tracing::error!("executor errored on `{change}`: {reason}");
            Ok(QueueStep::Failed { reason })
        }
        Ok(ExecutorOutcome::Failed { reason }) => {
            tracing::error!("executor reported Failed for `{change}`: {reason}");
            Ok(QueueStep::Failed { reason })
        }
        Ok(ExecutorOutcome::PreconditionUnmet { reason }) => {
            // a74: surfaced only on the revise path today (the pending/
            // implementer path still propagates the gate refusal as an `Err`,
            // handled above). The pending-change precondition case is out of
            // scope for a74 — treat it as `Failed` here so the implementer
            // path's behavior is unchanged if a future executor entry point
            // ever produces this variant.
            tracing::error!(
                "executor reported PreconditionUnmet for `{change}`: {reason}"
            );
            Ok(QueueStep::Failed { reason })
        }
        Ok(ExecutorOutcome::Aborted { reason }) => {
            // a39: the executor's subprocess was killed by the
            // daemon's own SIGTERM cascade. The classifier set this
            // outcome because `SHUTDOWN_REQUESTED == true` AND the
            // exit status was 143. Drop the `.in-progress` lock per
            // the canonical unlock-on-any-outcome rule; do NOT
            // increment the failure counter, do NOT write
            // `.perma-stuck.json`, do NOT post a chatops failure
            // alert (operator initiated the shutdown), AND leave any
            // `.iteration-pending.json` marker in place (the next
            // iteration after restart resumes context).
            tracing::info!(
                url = %repo.url,
                change = %change,
                "executor aborted: {reason}"
            );
            // Don't propagate the unlock error to the walker — the
            // walker would otherwise treat a stale-lock cleanup
            // hiccup as a post-executor Err AND bump the counter for
            // an outcome we explicitly chose to exempt. Best-effort
            // is consistent with how the `IterationRequested` arm
            // unlocks below.
            if let Err(e) = queue::unlock(workspace, change) {
                tracing::warn!(
                    url = %repo.url,
                    change = %change,
                    "Aborted arm: dropping .in-progress failed (continuing): {e:#}"
                );
            }
            Ok(QueueStep::Aborted)
        }
        Ok(ExecutorOutcome::SpecNeedsRevision {
            unimplementable_tasks,
            revision_suggestion,
        }) => {
            handle_spec_needs_revision_outcome(
                paths,
                workspace,
                repo,
                chatops_ctx,
                change,
                unimplementable_tasks,
                revision_suggestion,
            )
            .await
        }
        Ok(ExecutorOutcome::AskUser {
            question,
            resume_handle,
        }) => match chatops_ctx {
            Some(ctx) => {
                // Unlock BEFORE posting so the change is in a clean
                // "waiting" state (no .in-progress) as the spec mandates.
                queue::unlock(workspace, change)?;
                escalate_to_chatops(
                    paths,
                    workspace,
                    repo,
                    ctx,
                    change,
                    &question,
                    resume_handle.0,
                )
                .await?;
                Ok(QueueStep::Escalated)
            }
            None => {
                tracing::warn!("executor asked a question on `{change}`: {question}");
                Ok(QueueStep::AskUserExitEarly)
            }
        },
        Ok(ExecutorOutcome::IterationRequested {
            completed_tasks,
            remaining_tasks,
            reason,
            iteration_number,
        }) => {
            handle_iteration_requested(
                paths,
                workspace,
                repo,
                github_cfg,
                change,
                completed_tasks,
                remaining_tasks,
                reason,
                iteration_number,
            )
            .await
        }
        Ok(ExecutorOutcome::Completed { .. }) => {
            handle_completed_outcome(paths, workspace, repo, change)
        }
    }
}

/// Backoff base for the in-pass bounded executor retry
/// (executor-outcome-legibility-and-retry). Exponential per attempt
/// (`base * 2^(attempt-1)`, capped); a small default so a short transient (an
/// upstream-API blip) gets a couple of quick re-tries before the failure is
/// surfaced, while the daemon's separate next-pass re-pickup still covers a
/// longer outage.
pub(crate) const EXECUTOR_RETRY_BACKOFF_BASE: Duration = Duration::from_secs(2);

/// Run the executor for `change` under the bounded no-committable-result retry
/// (executor-outcome-legibility-and-retry). Re-invokes `executor.run` up to
/// `executor.session_retries()` ADDITIONAL times, with backoff between
/// attempts, while the outcome is a no-committable-result failure AND the
/// strategy's `is_retryable` hint does not veto — then returns the final
/// outcome for [`handle_outcome`]. The caller holds `.in-progress` across the
/// whole loop, so a retrying failure stays observably "working" (the retained
/// lock + busy marker), distinct from a terminal, retries-exhausted failure.
///
/// The "no committable result" guard reuses the existing success signal — a
/// clean working tree (`has_executor_changes` false), the same signal that maps
/// a no-diff `Completed` to `Failed`. A session that produced committable work
/// is never blindly re-run. No error classification: a transient clears on a
/// retry; a deterministic failure recurs through every attempt and is surfaced
/// with its assembled reason. This is an IN-PASS retry — it does not alter the
/// daemon's separate next-pass re-pickup scheduling.
pub(crate) async fn run_executor_with_retry(
    executor: &dyn Executor,
    repo: &RepositoryConfig,
    workspace: &Path,
    change: &str,
    backoff_base: Duration,
) -> Result<ExecutorOutcome> {
    let session_retries = executor.session_retries();
    let mut attempt: u32 = 0;
    loop {
        let outcome = executor.run(workspace, change).await;
        // `session_retries == 0` returns here on the first pass (retry disabled,
        // AND any `Some(true)` strategy hint suppressed — the bound is absolute).
        if attempt >= session_retries {
            return outcome;
        }
        let retry = match &outcome {
            Ok(o) => should_retry_executor_outcome(executor, repo, workspace, change, o),
            // An infrastructure `Err` (e.g. a spawn failure) is not a
            // no-committable-result agent failure; surface it without re-running.
            Err(_) => false,
        };
        if !retry {
            return outcome;
        }
        attempt += 1;
        // Observably distinct from a terminal failure: this WARN (plus the
        // retained lock + busy marker) marks a retry-in-progress; no failure
        // notification is posted until the loop's FINAL outcome reaches
        // `handle_outcome`.
        tracing::warn!(
            url = %repo.url,
            change = %change,
            attempt,
            session_retries,
            "executor produced no committable result; retrying in-pass (bounded) before surfacing the failure"
        );
        tokio::time::sleep(retry_backoff(attempt, backoff_base)).await;
    }
}

/// Exponential backoff for the Nth retry attempt (1-indexed): `base *
/// 2^(attempt-1)`, capped at 60s. `base == 0` (tests) yields zero delay.
fn retry_backoff(attempt: u32, base: Duration) -> Duration {
    let factor = 1u32
        .checked_shl(attempt.saturating_sub(1))
        .unwrap_or(u32::MAX);
    base.saturating_mul(factor).min(Duration::from_secs(60))
}

/// Decide whether a (successful-call) executor outcome is a no-committable-
/// result failure the bounded retry should re-run. Consults the strategy's
/// `is_retryable` hint, then the clean-working-tree guard.
fn should_retry_executor_outcome(
    executor: &dyn Executor,
    repo: &RepositoryConfig,
    workspace: &Path,
    change: &str,
    outcome: &ExecutorOutcome,
) -> bool {
    let hint = executor.is_retryable(outcome);
    match outcome {
        ExecutorOutcome::Failed { .. } => match hint {
            Some(false) => false,
            // Retry even when a committable result exists (the strategy has
            // overridden the guard); still bounded by `session_retries`.
            Some(true) => true,
            None => !workspace_has_committable_result(workspace, change),
        },
        ExecutorOutcome::Completed { .. } => match hint {
            // A `Some(false)` hint vetoes — consistent with the `Failed` arm.
            Some(false) => false,
            // A `Some(true)` hint retries even when a committable result exists,
            // overriding the no-result guard AND the self-heal guard below —
            // consistent with the `Failed` arm AND the spec's "`Some(true)`
            // SHALL retry even when a committable result exists" (a general
            // statement, not scoped to the `Failed` outcome). Still bounded by
            // `session_retries` (the loop suppresses it when the bound is 0).
            Some(true) => true,
            // No strategy opinion: a no-diff `Completed` is the no-op-completion
            // failure `handle_completed_outcome` maps to `Failed`; retry it under
            // the no-result rule. A `Completed` WITH a diff is a real success —
            // never retried. A self-heal `Completed` (every task `[x]`) is NOT a
            // failure (it is archived) AND re-running an agent that believes it
            // is done would not help, so skip it.
            None => {
                if tasks_all_complete(repo, workspace, change) {
                    false
                } else {
                    !workspace_has_committable_result(workspace, change)
                }
            }
        },
        // Deliberate halt / progress / shutdown signals are never blindly
        // re-run.
        _ => false,
    }
}

/// Whether the executor left a committable result — a non-empty working-tree
/// diff attributable to the executor (ignoring autocoder's own meta-files). A
/// clean tree → no committable result → eligible for the no-result retry.
fn workspace_has_committable_result(workspace: &Path, change: &str) -> bool {
    let status = match git::status_porcelain(workspace) {
        Ok(s) => s,
        Err(e) => {
            // A git failure (corrupt repo, lock contention) yields an empty
            // status, which reads as "no committable result" → the session
            // becomes retry-eligible. That is self-limiting (a re-run hits the
            // same git failure), but WARN so a spurious retry is diagnosable
            // rather than silent.
            tracing::warn!(
                workspace = %workspace.display(),
                change = %change,
                "git status failed while checking for a committable result; \
                 treating as no result (retry-eligible): {e:#}"
            );
            String::new()
        }
    };
    has_executor_changes(&status, change)
}

/// Whether every `tasks.md` checkbox for `change` is `[x]` — the self-heal
/// signal that distinguishes a complete change (re-running would not help) from
/// a genuine no-op `Completed`.
fn tasks_all_complete(repo: &RepositoryConfig, workspace: &Path, change: &str) -> bool {
    let spec_root = crate::spec_root::SpecRoot::for_repo(repo, workspace);
    tasks_md_all_complete(&spec_root, change).unwrap_or(false)
}

/// Handle `ExecutorOutcome::Completed`: unlock, inspect the working tree,
/// run the self-heal / lazy-archive probes, then archive+commit. Extracted
/// verbatim from `handle_outcome` (a68 function-size split).
fn handle_completed_outcome(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    change: &str,
) -> Result<QueueStep> {
    // Remove the `.in-progress` lock BEFORE inspecting the working
    // tree: the lock file is untracked and would otherwise show up
    // in `git status --porcelain`, contaminating the dirty check
    // and getting swept into the commit by `git add -A`.
    queue::unlock(workspace, change)?;
    // a27a1: lifecycle — if a stale `.iteration-pending.json`
    // marker is present (the prior iteration emitted
    // IterationRequested AND this iteration emitted Completed),
    // delete it after the commit + archive step completes
    // successfully. This is done AFTER the archive section
    // below; we just stash the workspace + change here so the
    // delete-after-success site is easy to spot.
    let dirty = git::status_porcelain(workspace)?;
    if dirty.is_empty() {
        // Self-heal probe: if every task is `[x]` AND
        // `openspec validate --strict` exits 0, the change's
        // implementation is already on the base branch and the
        // only thing missing is the archive move. Run the archive
        // ourselves rather than burn another iteration on a no-op
        // Completed.
        let spec_root = crate::spec_root::SpecRoot::for_repo(repo, workspace);
        let tasks_complete = tasks_md_all_complete(&spec_root, change).unwrap_or(false);
        if tasks_complete && openspec_validate_strict_passes(&spec_root, change) {
            tracing::info!(
                url = %repo.url,
                change = %change,
                "self-heal: implementation already in HEAD, archiving"
            );
            let subject = format!("archive: {change}: implementation already in base");
            if let Err(e) = queue::archive_at(&spec_root, change) {
                tracing::error!(
                    url = %repo.url,
                    change = %change,
                    "self-heal: queue::archive failed: {e:#}"
                );
                return Ok(QueueStep::Failed {
                    reason: format!("self-heal archive failed: {e:#}"),
                });
            }
            if let Err(e) = git::add_all(workspace) {
                tracing::error!(
                    url = %repo.url,
                    change = %change,
                    "self-heal: git add -A failed: {e:#}"
                );
                return Ok(QueueStep::Failed {
                    reason: format!("self-heal git add failed: {e:#}"),
                });
            }
            if let Err(e) = git::commit(workspace, &subject) {
                tracing::error!(
                    url = %repo.url,
                    change = %change,
                    "self-heal: git commit failed: {e:#}"
                );
                return Ok(QueueStep::Failed {
                    reason: format!("self-heal git commit failed: {e:#}"),
                });
            }
            return Ok(QueueStep::ArchivedSelfHeal);
        }
        tracing::warn!(
            "agent reported Completed for `{change}` without modifying the workspace; marking Failed"
        );
        return Ok(QueueStep::Failed {
            reason: "agent reported Completed without modifying the workspace".into(),
        });
    } else if is_lazy_archive(&dirty) {
        tracing::warn!(
            "agent appears to have archived `{change}` without implementing the change; reverting and marking Failed"
        );
        // Revert the staged moves so the next iteration starts clean.
        if let Err(e) = git::reset_hard_head(workspace) {
            tracing::error!("failed to revert lazy-archive moves for `{change}`: {e:#}");
        }
        return Ok(QueueStep::Failed {
            reason: "agent attempted lazy archive (rename only, no implementation)".into(),
        });
    } else {
        // Build the commit subject BEFORE the archive rename: it
        // reads `openspec/changes/<change>/proposal.md`, which the
        // archive step moves to `openspec/changes/archive/...`.
        let subject = build_commit_subject(workspace, change)?;
        // a27a1: lifecycle — if this Completed terminates a
        // multi-iteration sequence, delete the iteration-pending
        // marker (now in state_dir; no longer in the archived
        // directory regardless). Idempotent — absent marker is
        // fine.
        let basename_for_marker = workspace
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        if let Err(e) = crate::iteration_pending::remove_marker(paths, basename_for_marker, change)
        {
            tracing::warn!(
                url = %repo.url,
                change = %change,
                "failed to remove iteration-pending marker on Completed: {e:#}"
            );
        }
        // Archive BEFORE the commit so the single commit captures
        // both the executor's implementation diff AND the archive
        // rename. After this sequence the working tree is clean,
        // even for the trailing change of a pass — no dangling
        // rename for the next iteration's dirty-check to trip on.
        let spec_root = crate::spec_root::SpecRoot::for_repo(repo, workspace);
        queue::archive_at(&spec_root, change)?;
        git::add_all(workspace)?;
        git::commit(workspace, &subject)?;
    }
    Ok(QueueStep::Archived)
}

/// Handle `ExecutorOutcome::SpecNeedsRevision`: unlock, write the marker,
/// drop the iteration-pending marker, and post the operator alert. Extracted
/// verbatim from `handle_outcome` (a68 function-size split).
async fn handle_spec_needs_revision_outcome(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    change: &str,
    unimplementable_tasks: Vec<UnimplementableTask>,
    revision_suggestion: String,
) -> Result<QueueStep> {
    tracing::warn!(
        url = %repo.url,
        change = %change,
        flagged = unimplementable_tasks.len(),
        "executor returned SpecNeedsRevision; writing marker and alerting operator"
    );
    // (a) Unlock the change so it's not left in an in-progress
    // state. Mirrors how every other Failed-equivalent outcome
    // hands the change back to operator-managed territory.
    queue::unlock(workspace, change)?;
    // (b) Write the marker. A failure here is logged but does NOT
    // propagate: the alert still goes out, and the next iteration
    // would simply re-trigger the outcome (the agent's pre-flight
    // is deterministic for a given tasks.md).
    let detail = SpecNeedsRevisionDetail {
        unimplementable_tasks: unimplementable_tasks.clone(),
        unarchivable_deltas: Vec::new(),
        revision_suggestion: revision_suggestion.clone(),
        gate_error: None,
        contradictions: Vec::new(),
    };
    if let Err(e) = spec_revision::write_marker(workspace, change, &detail) {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            "failed to write spec-needs-revision marker: {e:#}"
        );
    }
    // a27a1: SpecNeedsRevision terminates the iteration sequence
    // (operator action is required from here on); drop the
    // iteration-pending marker so the change reverts to normal
    // queue ordering on the next iteration. Idempotent — absent
    // marker is OK.
    let basename_for_marker = workspace
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    if let Err(e) = crate::iteration_pending::remove_marker(paths, basename_for_marker, change) {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            "failed to remove iteration-pending marker on SpecNeedsRevision: {e:#}"
        );
    }
    // (c) Post the chatops alert. Best-effort: any failure is
    // logged at WARN and does not propagate.
    maybe_post_spec_revision_alert(
        paths,
        chatops_ctx,
        repo,
        change,
        &unimplementable_tasks,
        &revision_suggestion,
    )
    .await;
    // (d) Halt the queue walk this iteration. Do NOT increment
    // the perma-stuck counter — the marker handles exclusion
    // directly; the counter is for repeat-execution-failure
    // territory, which this is not.
    Ok(QueueStep::SpecRevisionMarked)
}

/// Polling-loop arm for `ExecutorOutcome::IterationRequested` (a27a1).
/// Performs, in order:
///
/// 1. Commit the workspace's diff to the agent branch with the message
///    `iteration <N> of <change>: <reason-truncated-to-80-chars>`. If
///    the working tree is clean (the agent emitted iteration_request
///    without modifying anything), the commit step is skipped with a
///    `tracing::warn!` AND the function proceeds to step 3.
/// 2. Force-push the agent branch to the remote. Push failure aborts:
///    `tracing::error!` AND skip steps 3 (no marker written, so the next
///    polling iteration treats the change as normally-pending).
/// 3. Write `.iteration-pending.json` atomically with the new state.
/// 4. Drop `.in-progress`.
///
/// Step 4 ALWAYS runs (even on push failure, AND even if the marker
/// write also fails), so the change is never left locked.
///
/// This arm SHALL NOT call any PR-open OR PR-comment routine. PRs are
/// reserved for the FINAL iteration's `Completed` outcome.
#[allow(clippy::too_many_arguments)]
async fn handle_iteration_requested(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    change: &str,
    completed_tasks: Vec<String>,
    remaining_tasks: Vec<String>,
    reason: String,
    iteration_number: u32,
) -> Result<QueueStep> {
    // Always unlock at the end of the arm — collect any deferred
    // errors first AND treat unlock as a best-effort cleanup.
    let result = run_iteration_requested_steps(
        paths,
        workspace,
        repo,
        github_cfg,
        change,
        completed_tasks,
        remaining_tasks,
        reason,
        iteration_number,
    )
    .await;
    if let Err(e) = queue::unlock(workspace, change) {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            "failed to drop .in-progress on IterationRequested arm: {e:#}"
        );
    }
    result
}

/// Inner workflow of [`handle_iteration_requested`]. Pulled out so the
/// outer wrapper can guarantee `.in-progress` is dropped on every exit
/// path (success, push failure, marker-write failure).
#[allow(clippy::too_many_arguments)]
async fn run_iteration_requested_steps(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    change: &str,
    completed_tasks: Vec<String>,
    remaining_tasks: Vec<String>,
    reason: String,
    iteration_number: u32,
) -> Result<QueueStep> {
    // Step 1: commit the diff (or skip if clean).
    // The .in-progress file is untracked, but `git add -A` would sweep
    // it into the commit. Drop the lock first (matches the other
    // outcome arms' discipline). The outer wrapper's unlock-on-exit is
    // idempotent against this drop.
    queue::unlock(workspace, change)?;
    let dirty = git::status_porcelain(workspace)?;
    if dirty.is_empty() {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            iteration_number,
            "IterationRequested with clean working tree: agent emitted iteration_request without modifying any files; writing marker anyway (lack-of-progress will count against the cap on the next iteration)"
        );
    } else {
        let subject = build_iteration_commit_subject(change, iteration_number, &reason);
        git::add_all(workspace)?;
        if let Err(e) = git::commit(workspace, &subject) {
            // Mirror the clean-tree case: log AND proceed to write the
            // marker. A non-clean tree that nonetheless fails to commit
            // is an anomaly (probably a config issue like missing
            // user.email); the marker still belongs because the agent
            // INTENDED to advance, AND the cap will catch a loop.
            tracing::warn!(
                url = %repo.url,
                change = %change,
                iteration_number,
                "iteration-request commit failed (proceeding to marker): {e:#}"
            );
        }
    }

    // Step 2: force-push the agent branch to the remote.
    let push_remote = if github_cfg.fork_owner.is_some() {
        "fork"
    } else {
        "origin"
    };
    if let Err(e) = git::push_force_with_lease(workspace, &repo.agent_branch, push_remote) {
        tracing::error!(
            url = %repo.url,
            change = %change,
            iteration_number,
            "iteration-request force-push failed; NOT writing marker: {e:#}"
        );
        // Per D5: push failure leaves no marker. The change reverts to
        // normal pending behaviour on the next polling cycle.
        return Ok(QueueStep::IterationPending);
    }

    // Step 3: write the iteration-pending marker atomically. The marker
    // lives under `<state>/iteration-pending/<basename>/<change>.json`
    // (NOT in the workspace) per a16's "daemon bookkeeping never appears
    // in the managed repo's working tree" rule; this avoids the
    // `git clean -fd` wipe that broke earlier in-workspace implementations.
    let marker = crate::iteration_pending::IterationPendingMarker {
        completed_tasks,
        remaining_tasks,
        reason,
        iteration_number,
    };
    let basename_for_marker = workspace
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    if let Err(e) =
        crate::iteration_pending::write_marker(paths, basename_for_marker, change, &marker)
    {
        tracing::error!(
            url = %repo.url,
            change = %change,
            iteration_number,
            "iteration-pending marker write failed; next iteration will see no continuation context: {e:#}"
        );
    }
    Ok(QueueStep::IterationPending)
}

/// Build the commit subject for an `IterationRequested` arm's WIP
/// commit. Format: `iteration <N> of <change>: <reason>` truncated to
/// keep the subject under 80 chars (the same discipline as
/// `build_commit_subject`).
pub(crate) fn build_iteration_commit_subject(
    change: &str,
    iteration_number: u32,
    reason: &str,
) -> String {
    const MAX_SUBJECT_LEN: usize = 80;
    let prefix = format!("iteration {iteration_number} of {change}: ");
    let room = MAX_SUBJECT_LEN.saturating_sub(prefix.len());
    let trimmed_reason: String = reason
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .chars()
        .take(room)
        .collect();
    format!("{prefix}{trimmed_reason}")
}

/// Detect the lazy-archive failure mode: the executor returned Completed
/// but the only thing it did was rename the change directory into
/// `openspec/changes/archive/`. Returns true when:
/// - `status` is non-empty, AND
/// - every line is a rename (status code contains `R`), AND
/// - every rename's destination path starts with `openspec/changes/archive/`.
///
/// Returns false for any mix that includes a non-rename or a rename outside
/// the archive path — those are treated as legitimate implementations.
pub(crate) fn is_lazy_archive(status: &str) -> bool {
    let mut any = false;
    for line in status.lines() {
        if line.len() < 4 {
            return false; // malformed; bail rather than misclassify
        }
        // Porcelain format: two status chars in cols 0-1, space, then paths.
        let staged = line.as_bytes()[0] as char;
        let unstaged = line.as_bytes()[1] as char;
        if staged != 'R' && unstaged != 'R' {
            return false;
        }
        // Rename lines look like `R  old_path -> new_path`.
        let payload = &line[3..];
        let dest = match payload.split_once(" -> ") {
            Some((_old, new)) => new,
            None => return false,
        };
        if !dest.starts_with("openspec/changes/archive/") {
            return false;
        }
        any = true;
    }
    any
}

/// Decide whether a `git status --porcelain` block (taken after a resume
/// returned `Completed`) contains any change attributable to the executor,
/// as opposed to autocoder's own bookkeeping. In the resume path autocoder
/// itself writes/deletes `.question.json` and `.answer.json` inside the
/// change directory; those entries are NOT executor output and must not
/// be counted when deciding whether the executor produced an artifact.
///
/// Returns true iff at least one porcelain entry references a path that
/// is NOT one of the meta-files for `change`.
pub(crate) fn has_executor_changes(status: &str, change: &str) -> bool {
    let q = format!("openspec/changes/{change}/.question.json");
    let a = format!("openspec/changes/{change}/.answer.json");
    let is_meta = |path: &str| path == q || path == a;
    for raw_line in status.lines() {
        // `git::status_porcelain` trims the entire blob, which strips the
        // leading column-1 space on the first/last line of unstaged
        // changes (e.g. ` D path` -> `D path`). Re-normalize per line by
        // skipping the leading status block and the whitespace that
        // separates it from the path, rather than fixed `line[3..]`.
        let line = raw_line.trim_start();
        if line.is_empty() {
            continue;
        }
        let path_start = match line.find(char::is_whitespace) {
            Some(i) => i,
            None => continue, // malformed; skip rather than misclassify
        };
        let payload = line[path_start..].trim_start();
        if payload.is_empty() {
            continue;
        }
        // Rename: `<old> -> <new>` — both sides must be meta to skip.
        let (left, right) = match payload.split_once(" -> ") {
            Some((l, r)) => (l, Some(r)),
            None => (payload, None),
        };
        if !is_meta(left) {
            return true;
        }
        if let Some(r) = right
            && !is_meta(r)
        {
            return true;
        }
    }
    false
}

/// Build a commit subject from the change name and the first non-empty line of
/// the `## Why` section of `proposal.md`. Truncated to 72 characters total.
pub(crate) fn build_commit_subject(workspace: &Path, change: &str) -> Result<String> {
    let proposal = workspace
        .join("openspec/changes")
        .join(change)
        .join("proposal.md");
    let raw = std::fs::read_to_string(&proposal).with_context(|| {
        format!(
            "reading proposal for commit subject: {}",
            proposal.display()
        )
    })?;
    let why_summary = first_line_of_section(&raw, "## Why").unwrap_or_else(|| change.to_string());
    let mut subject = format!("{change}: {why_summary}");
    if subject.chars().count() > 72 {
        subject = subject.chars().take(72).collect();
    }
    Ok(subject)
}

/// Return the first non-empty line under the named markdown header. Returns
/// `None` if the header is absent or has no non-empty body line.
pub(crate) fn first_line_of_section(text: &str, header: &str) -> Option<String> {
    let mut in_section = false;
    for raw_line in text.lines() {
        let line = raw_line.trim_end();
        if line.trim_start().starts_with("## ") {
            in_section = line.trim_start() == header;
            continue;
        }
        if in_section {
            let stripped = line.trim();
            if !stripped.is_empty() {
                return Some(stripped.to_string());
            }
        }
    }
    None
}

/// Read `openspec/changes/<change>/tasks.md` and decide whether every task
/// checkbox is `[x]`. Scans each line for the regex `^\s*-\s*\[([ x])\]`.
/// Returns `Ok(true)` iff at least one match is present AND every match
/// captures `x`. Any match capturing ` ` yields `Ok(false)`. An empty
/// match-set yields `Ok(false)` — a tasks.md with no checkboxes is not
/// "all complete". Returns `Err(_)` only on file-read failure or
/// regex-init failure.
pub fn tasks_md_all_complete(spec_root: &crate::spec_root::SpecRoot, change: &str) -> Result<bool> {
    let tasks_path = spec_root.changes_dir().join(change).join("tasks.md");
    let raw = std::fs::read_to_string(&tasks_path)
        .with_context(|| format!("reading {}", tasks_path.display()))?;
    let re =
        regex::Regex::new(r"^\s*-\s*\[([ x])\]").context("compiling tasks.md checkbox regex")?;
    let mut any = false;
    for line in raw.lines() {
        if let Some(caps) = re.captures(line) {
            any = true;
            if &caps[1] != "x" {
                return Ok(false);
            }
        }
    }
    Ok(any)
}

/// Shell out to `openspec validate <change> --strict` in `workspace` and
/// report whether it exited 0. Any error — binary missing, non-zero exit,
/// transport problem — returns `false`. The caller falls through to the
/// existing Failed path when self-heal preconditions are unmet, which is
/// the conservative behavior.
pub fn openspec_validate_strict_passes(
    spec_root: &crate::spec_root::SpecRoot,
    change: &str,
) -> bool {
    match std::process::Command::new("openspec")
        .args(["validate", change, "--strict"])
        .current_dir(spec_root.openspec_cwd())
        .output()
    {
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
}
