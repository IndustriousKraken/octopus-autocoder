use super::*;

pub(crate) enum ResumeDisposition {
    Archived,
    CompletedNoDiff,
    EscalatedAgain,
    Failed,
    Errored,
    /// Resume returned `SpecNeedsRevision`. Marker has been written and
    /// the operator alerted; treat as a non-counter-bumping failure-
    /// equivalent (the marker handles exclusion).
    SpecRevisionMarked,
    /// a39: resume returned `Aborted` (subprocess killed by the daemon's
    /// own SIGTERM cascade). Treat as a non-counter-bumping failure-
    /// equivalent — the failure budget is not the right tool for an
    /// operator-initiated shutdown.
    Aborted,
}

impl ResumeDisposition {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            ResumeDisposition::Archived => "archived",
            ResumeDisposition::CompletedNoDiff => "failed_no_diff",
            ResumeDisposition::EscalatedAgain => "escalated",
            ResumeDisposition::Failed => "failed",
            ResumeDisposition::Errored => "errored",
            ResumeDisposition::SpecRevisionMarked => "spec_needs_revision",
            ResumeDisposition::Aborted => "aborted",
        }
    }
}

/// Post a question to ChatOps and write a fresh `.question.json`. Called
/// from the initial AskUser handling (pending → waiting) AND from the
/// resume path when the agent asks ANOTHER question.
pub(crate) async fn escalate_to_chatops(
    _paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    ctx: &ChatOpsContext,
    change: &str,
    question: &str,
    resume_handle: serde_json::Value,
) -> Result<()> {
    let thread_ts = ctx
        .chatops
        .post_question(&ctx.channel, change, question)
        .await
        .with_context(|| format!("posting Slack question for `{change}`"))?;
    let payload = QuestionPayload {
        thread_ts,
        channel: ctx.channel.clone(),
        resume_handle,
        asked_at: chrono::Utc::now(),
    };
    chatops::write_question_file(workspace, change, &payload)?;
    tracing::info!(
        url = repo.url.as_str(),
        "escalated `{change}` to Slack channel {} (thread {})",
        ctx.channel,
        payload.thread_ts
    );
    Ok(())
}

/// Iterate the pending queue, invoking the executor for each ready change.
/// Returns the names of changes that were archived (i.e. those for which the
/// executor returned `Completed`, regardless of diff). On `AskUser`:
///   - if `chatops_ctx` is `Some`, post the question to Slack, write a
///     fresh `.question.json`, unlock, and proceed to the next change;
///   - if `chatops_ctx` is `None`, log an error and break the pass (the
///     architecture-foundation behavior is preserved when chatops is
///     not configured).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn walk_queue(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    executor: &dyn Executor,
    chatops_ctx: Option<&ChatOpsContext>,
    perma_stuck_threshold: u32,
    max_changes: u32,
    pending: Vec<String>,
) -> Result<(Vec<String>, bool)> {
    let mut archived: Vec<String> = Vec::new();
    let mut includes_self_heal = false;

    for change in pending {
        let result = process_one_pending_change(
            paths,
            workspace,
            repo,
            github_cfg,
            executor,
            chatops_ctx,
            &change,
        )
        .await;

        if matches!(
            apply_pending_outcome(
                paths,
                workspace,
                repo,
                chatops_ctx,
                perma_stuck_threshold,
                max_changes,
                change,
                result,
                &mut archived,
                &mut includes_self_heal,
            )
            .await,
            WalkControl::Halt
        ) {
            break;
        }

        // a71: yield to pending operator chatops requests. The change just
        // processed reached its outcome (recorded above) and the walk would
        // otherwise start the next change. If any operator request (`send
        // it` / `propose` / `changelog`) is now pending for this repo, end
        // the batch here — the caller opens its PR with the changes
        // accumulated so far AND the next iteration's iteration-top drain
        // consumes the operator request before starting a new batch. This is
        // an ADDITIONAL, request-driven halt on top of the existing
        // "any non-Archive outcome halts the walk" rule; the currently-
        // executing change is never interrupted (the check runs strictly
        // between changes). The peek does NOT drain — the iteration-top drain
        // remains the sole consumer. `current()` is `None` (so the walk never
        // yields) when the surrounding task did not bind the queue handles —
        // every test that does not opt in, preserving pre-a71 behavior.
        if let Some(queues) = operator_requests::current()
            && queues.any_pending()
        {
            tracing::info!(
                url = %repo.url,
                archived = archived.len(),
                "operator chatops request pending; ending the batch after the current change so the next iteration drains it (a71)"
            );
            break;
        }
    }

    Ok((archived, includes_self_heal))
}

/// Outcome of processing one pending change: whether the queue walk
/// should continue to the next change or halt for this iteration.
enum WalkControl {
    Continue,
    Halt,
}

/// Apply the per-change [`QueueStep`] outcome from `process_one_pending_change`:
/// log the result, run the archive/failure/escalation side-effects, and report
/// whether `walk_queue` should continue or halt. Behavior-identical to the
/// inline match this was extracted from (a68 function-size split).
#[allow(clippy::too_many_arguments)]
async fn apply_pending_outcome(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    perma_stuck_threshold: u32,
    max_changes: u32,
    change: String,
    result: Result<QueueStep>,
    archived: &mut Vec<String>,
    includes_self_heal: &mut bool,
) -> WalkControl {
    let outcome_label = match &result {
        Ok(QueueStep::Archived) => "archived",
        Ok(QueueStep::ArchivedSelfHeal) => "archived_self_heal",
        Ok(QueueStep::Failed { .. }) => "failed",
        Ok(QueueStep::Escalated) => "escalated",
        Ok(QueueStep::AskUserExitEarly) => "ask_user_exit_early",
        Ok(QueueStep::SpecRevisionMarked) => "spec_needs_revision",
        Ok(QueueStep::IterationPending) => "iteration_pending",
        Ok(QueueStep::Aborted) => "aborted",
        Err(_) => "error",
    };
    tracing::info!(
        url = %repo.url,
        change = %change,
        outcome = outcome_label,
        "change finished"
    );

    // Any non-Archive outcome halts the walk. Later changes in the
    // queue may depend on this one having succeeded; attempting them
    // now would either produce wrong-shape commits or contaminate
    // this change's retry. Perma-stuck (default threshold 2) bounds
    // repeat failures: a persistently-failing change is excluded
    // from `list_pending` after the threshold, freeing the queue.
    match result {
        Ok(QueueStep::Archived) | Ok(QueueStep::ArchivedSelfHeal) => {
            let was_self_heal = matches!(&result, Ok(QueueStep::ArchivedSelfHeal));
            if was_self_heal {
                *includes_self_heal = true;
            }
            // Archived (regular or self-heal) → reset the per-change
            // consecutive-failure counter so the next failure starts
            // fresh.
            if let Err(e) = failure_state::clear(paths, workspace, &change) {
                tracing::warn!(
                    url = %repo.url,
                    change = %change,
                    "failed to clear failure-state entry after archive: {e:#}"
                );
            }
            // Canonical-spec RAG post-archive hook (a21). Inspect
            // the just-landed commit (HEAD vs HEAD~1) for canonical
            // spec changes; re-embed affected capabilities. Fail-
            // open via the hook itself.
            let touched_caps = crate::rag::capabilities_touched_between(workspace, "HEAD~1..HEAD");
            if !touched_caps.is_empty() {
                crate::rag::post_archive_hook(workspace, &touched_caps).await;
            }
            archived.push(change);
            if archived.len() as u32 >= max_changes {
                tracing::info!(
                    url = %repo.url,
                    cap = max_changes,
                    "reached max_changes_per_pr cap; deferring remaining pending changes to next iteration"
                );
                return WalkControl::Halt;
            }
        }
        Ok(QueueStep::Failed { reason }) => {
            // Failed (or transformed-to-Failed) → bump the counter and,
            // if the threshold is hit, mark perma-stuck + alert. Then
            // halt the walk: later pending changes may depend on this
            // one and should not be attempted until the next iteration.
            handle_failure_counter(
                paths,
                workspace,
                repo,
                chatops_ctx,
                &change,
                &reason,
                perma_stuck_threshold,
            )
            .await;
            tracing::info!(
                url = %repo.url,
                change = %change,
                "change failed; halting queue walk this iteration (later changes may depend on this one)"
            );
            return WalkControl::Halt;
        }
        Ok(QueueStep::Escalated) => {
            // Escalation posts a question to chatops and leaves the
            // change in the waiting set. Later pending changes may
            // depend on it; halt the walk so they wait for the human
            // reply on the next iteration.
            tracing::info!(
                url = %repo.url,
                change = %change,
                "change escalated to chatops; halting queue walk this iteration"
            );
            return WalkControl::Halt;
        }
        Ok(QueueStep::AskUserExitEarly) => {
            tracing::error!(
                url = repo.url.as_str(),
                "executor returned AskUser for `{change}` AND chatops is not configured; exiting pass. Set the `chatops:` config block to enable escalation."
            );
            return WalkControl::Halt;
        }
        Ok(QueueStep::SpecRevisionMarked) => {
            // Operator-action territory. The marker file, the chatops
            // alert, and the unlock have already been written by
            // `handle_outcome`. We must NOT bump the perma-stuck
            // counter (this isn't repeat-execution-failure territory)
            // but we DO halt the walk so later changes don't run
            // against an environment we just decided we can't
            // implement against.
            tracing::info!(
                url = %repo.url,
                change = %change,
                "change flagged as needing spec revision; halting queue walk this iteration"
            );
            return WalkControl::Halt;
        }
        Ok(QueueStep::IterationPending) => {
            // a27a1: the executor wants another iteration on this
            // change. The WIP has been committed + force-pushed to
            // the agent branch, `.iteration-pending.json` carries the
            // continuation state, AND `.in-progress` has been dropped
            // inside `handle_outcome`. The next polling iteration on
            // this repo will pick the change up first (queue front-
            // insertion via marker preference). Halt the walk now —
            // we do NOT chain a follow-up commit on top of the WIP
            // (PRs are reserved for the FINAL `Completed`).
            tracing::info!(
                url = %repo.url,
                change = %change,
                "change requested another iteration; halting queue walk this iteration"
            );
            return WalkControl::Halt;
        }
        Ok(QueueStep::Aborted) => {
            // a39: the executor's subprocess was killed by the
            // daemon's own SIGTERM cascade. `.in-progress` has been
            // dropped inside `handle_outcome`. We must NOT bump the
            // perma-stuck counter (operator-initiated shutdown is
            // not a repeat-execution-failure) AND we halt the walk
            // — the daemon is shutting down; later changes belong
            // to the next process's iteration.
            tracing::info!(
                url = %repo.url,
                change = %change,
                "change aborted by daemon shutdown; halting queue walk this iteration"
            );
            return WalkControl::Halt;
        }
        Err(e) => {
            // A vanished workspace `current_dir` at post-executor time (the
            // workspace dir was evicted/recreated between the executor run and
            // the post-executor `git status`) is a RECOVERABLE environmental
            // condition, not a per-change defect. Route it like the mid-
            // iteration recovery path (transient; the next iteration's
            // workspace-init re-clones it) instead of bumping the per-change
            // failure counter / writing a perma-stuck marker, so it does not
            // linger as a terminal `last failure` in `status`.
            if git::is_missing_workspace_dir(&e) {
                let class = classify_recovery_failure(&e);
                tracing::warn!(
                    url = repo.url.as_str(),
                    change = %change,
                    class = class.log_tag(),
                    "post-executor workspace directory vanished; treating as transient (re-init next iteration), not a per-change failure: {e:#}"
                );
                handle_classified_recovery_failure(
                    paths,
                    workspace,
                    &repo.url,
                    chatops_ctx,
                    chatops_ctx
                        .map(|c| c.failure_alerts_enabled)
                        .unwrap_or(false),
                    AlertCategory::WorkspaceInitFailure,
                    &e,
                    class,
                )
                .await;
                return WalkControl::Halt;
            }
            // The per-change processing function returned Err from a
            // non-executor source (e.g. queue::archive collision,
            // post-executor commit failure, lock I/O, an unlock
            // propagated by handle_outcome). The Failed outcome path
            // is consumed inside handle_outcome → Ok(QueueStep::Failed)
            // and already records via handle_failure_counter, so this
            // wrapper covers the OTHER per-change Err sources without
            // double-counting.
            let reason = format!("post-executor error: {e:#}");
            tracing::error!(
                url = repo.url.as_str(),
                change = %change,
                "fatal error processing change `{change}`: {e:#}"
            );
            handle_failure_counter(
                paths,
                workspace,
                repo,
                chatops_ctx,
                &change,
                &reason,
                perma_stuck_threshold,
            )
            .await;
            return WalkControl::Halt;
        }
    }

    WalkControl::Continue
}

/// Per-change processing scoped to one entry of the pending queue: lock →
/// optional start-of-work notification → executor.run → handle_outcome →
/// unlock. Any Err this function returns is a non-executor error (the
/// executor-Failed path is consumed inside `handle_outcome` and surfaces
/// as `Ok(QueueStep::Failed)`) and the caller in `walk_queue` records it
/// against the per-change counter before halting the walk.
pub(crate) async fn process_one_pending_change(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    executor: &dyn Executor,
    chatops_ctx: Option<&ChatOpsContext>,
    change: &str,
) -> Result<QueueStep> {
    // a006: set this repository's effective OS-sandbox credential toggles for
    // the duration of the whole change pipeline (pre-flight contradiction
    // checks, the executor, AND the in-iteration review). The guard resets the
    // override on every return path so the next iteration starts from the
    // daemon-global default.
    let _sandbox_repo_guard = crate::sandbox::enter_repo(repo.sandbox.as_ref());

    // Canon-editing-tasks pre-flight. Scans the change's `tasks.md` for a
    // task that directs a direct edit to the canonical specs
    // (`openspec/specs/`). The implementer implements code and tests only;
    // its spec delta is folded into canon by `openspec archive`. A task that
    // pre-folds the delta into canon makes the archive abort on a duplicate
    // requirement, so the change goes perma-stuck — catch it here, before any
    // executor or `[in]`/`[canon]` gate run is spent. Same marker + halt
    // semantics as the archivability pre-flight below.
    match handle_canon_editing_tasks_preflight(paths, workspace, repo, chatops_ctx, change).await {
        Ok(Some(step)) => return Ok(step),
        Ok(None) => {}
        Err(e) => {
            // A `tasks.md` read error is non-fatal: log + proceed (the check
            // already treats an unreadable tasks.md as "nothing to flag").
            tracing::warn!(
                url = %repo.url,
                change = %change,
                "canon-editing-tasks pre-flight errored; proceeding: {e:#}"
            );
        }
    }

    // Spec-delta archivability pre-flight (a17). Catches the a07-style
    // class of failures — a `## MODIFIED Requirements` block whose
    // `### Requirement:` header doesn't exist in canonical, etc. —
    // BEFORE the executor runs. Saves the LLM cost on changes whose
    // deltas would abort `openspec archive` later anyway. No lock is
    // taken on this path: the marker file is the operator-action gate;
    // failing-archivability changes never lock the queue dir.
    match handle_archivability_preflight(paths, workspace, repo, chatops_ctx, change).await {
        Ok(Some(step)) => return Ok(step),
        Ok(None) => {}
        Err(e) => {
            // Pre-flight should never fail (it's filesystem reads against
            // the change's own dir), but if it does we log + proceed to
            // the executor — better to incur a redundant Claude run than
            // halt the queue on an unexpected I/O glitch.
            tracing::warn!(
                url = %repo.url,
                change = %change,
                "spec-archivability pre-flight check errored; proceeding to executor: {e:#}"
            );
        }
    }

    // Verifier-gate framework (a61) + default-deny verdict ledger
    // (verifier-gates-fail-closed §5): the pre-executor `[in]`/`[canon]` gates'
    // fail-closed disposition is enforced STRUCTURALLY, not by per-arm
    // inspection. Every gate slot starts `Pending` (a non-passing state); a
    // runner ALWAYS records a verdict — an enabled gate records its
    // `Pass`/`Fail`/`FailedToRun`, a disabled gate records `Disabled` via a
    // stub. The executor runs ONLY when `ledger.blocking_ok()` (every blocking
    // gate `Pass` or `Disabled`); any `Pending` left by a runner that never
    // recorded holds the change by construction. There is NO skip/absent code
    // path — that is the thing that used to let a gate silently fail open.
    let mut ledger = crate::gate_ledger::GateLedger::new();

    // `[in]` gate: run its runner (enabled → handler verdict; disabled → stub
    // `Disabled`), record the verdict + model in the ledger. A short-circuit
    // holds the change immediately on a blocking-fail so we do NOT run `[canon]`
    // on a change already held (preserving the single-marker behavior).
    run_in_gate(paths, workspace, repo, chatops_ctx, change, &mut ledger).await?;
    if let Some(step) = hold_if_blocking_fail(workspace, change, ledger.r#in.verdict, &ledger) {
        return Ok(step);
    }

    // `[canon]` gate: same no-skip runner + record.
    run_canon_gate(paths, workspace, repo, chatops_ctx, change, &mut ledger).await?;
    if let Some(step) = hold_if_blocking_fail(workspace, change, ledger.canon.verdict, &ledger) {
        return Ok(step);
    }

    // `[rules]` gate (global-rules-gate): same no-skip runner + record.
    run_rules_gate(paths, workspace, repo, chatops_ctx, change, &mut ledger).await?;
    if let Some(step) = hold_if_blocking_fail(workspace, change, ledger.rules.verdict, &ledger) {
        return Ok(step);
    }

    // DEFENSIVE proceed-gate (the structural fail-closed guarantee): read the
    // ledger. If any blocking gate is NOT `Pass`/`Disabled` — including a
    // `Pending` left by a runner that returned without recording — HOLD. This
    // catches the "missed path" class the inspect-and-branch dispatch was prone
    // to: the hold happens without this code anticipating the specific failure.
    if !ledger.blocking_ok() {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            in_verdict = ledger.r#in.verdict.as_str(),
            canon_verdict = ledger.canon.verdict.as_str(),
            rules_verdict = ledger.rules.verdict.as_str(),
            "gate ledger is not blocking-ok; HOLDING the change (fail-closed by construction)"
        );
        // Persist the ledger so the PR-render path (if reached) sees the held
        // state; best-effort.
        let _ = crate::gate_ledger::write_ledger(workspace, change, &ledger);
        return Ok(QueueStep::SpecRevisionMarked);
    }

    // Every blocking gate is `Pass`/`Disabled`: persist the ledger for the PR
    // render AND proceed to the executor. Best-effort — the in-memory ledger
    // already gated the executor; only the PR-render record is at risk on a
    // write failure.
    if let Err(e) = crate::gate_ledger::write_ledger(workspace, change, &ledger) {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            "failed to persist gate ledger (PR will render only the post-executor verdict): {e:#}"
        );
    }

    queue::lock(workspace, change).with_context(|| format!("locking change `{change}`"))?;

    // Record which change this iteration is working on so the chatops
    // `status` reply can render `currently: working on <change>`. The
    // marker is held by the caller; best-effort update — failures are
    // logged at DEBUG and don't abort the iteration.
    busy_marker::update_change(paths, workspace, change);

    tracing::info!(
        url = %repo.url,
        change = %change,
        "starting work on change"
    );

    // Start-of-work notification: post a one-liner to chatops when the
    // operator has it enabled. Suppressed entirely when chatops is not
    // wired OR when `notifications.start_work` is false. A failed post
    // logs at WARN and does NOT prevent the executor from running.
    maybe_post_start_of_work(workspace, repo, chatops_ctx, change).await;

    // executor-outcome-legibility-and-retry: re-invoke the session up to
    // `executor.session_retries()` times on a no-committable-result failure,
    // with backoff, BEFORE handing the outcome to `handle_outcome`. The
    // `.in-progress` lock taken above is held across the whole loop (only the
    // final outcome's `handle_outcome` drops it), so a retry-in-progress stays
    // observably "working" rather than surfacing as a terminal failure.
    let outcome =
        run_executor_with_retry(executor, repo, workspace, change, EXECUTOR_RETRY_BACKOFF_BASE)
            .await;
    let result = handle_outcome(
        paths,
        workspace,
        repo,
        github_cfg,
        chatops_ctx,
        change,
        outcome,
    )
    .await;
    // Always unlock, even after a Completed → archive (archive moved the
    // dir, so the lock is gone, but `queue::unlock` is idempotent).
    let _ = queue::unlock(workspace, change);
    result
}

/// Run the `[in]` gate's runner AND record its verdict in `ledger` (no-skip
/// dispatch, verifier-gates-fail-closed §5). When the gate is enabled (the
/// scoped context is `Some` AND the registry installs `ContradictionCheck`),
/// the handler runs and its [`crate::gate_ledger::GateVerdict`] is recorded; a
/// Rust `Err` from the dispatch maps to `FailedToRun` and STILL holds (the
/// marker/alert are written here via [`handle_gate_error`]). When the gate is
/// disabled, a STUB records `Disabled` — never an absence. Either way the slot
/// is left with an affirmatively-recorded verdict, never `Pending`.
async fn run_in_gate(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    change: &str,
    ledger: &mut crate::gate_ledger::GateLedger,
) -> Result<()> {
    use crate::gate_ledger::GateVerdict;
    let gate = crate::verifier_gate::VerifierGate::In;
    let enabled = crate::preflight::change_contradiction::current().filter(|_| {
        matches!(
            crate::verifier_gate::GateRegistry::standard().resolve(gate),
            Some(crate::verifier_gate::GateImpl::ContradictionCheck)
        )
    });
    let Some(cc_ctx) = enabled else {
        // Disabled-gate STUB: record `Disabled` (a non-blocking verdict), NOT an
        // absence a reader must remember to treat as a pass.
        ledger.set_in(GateVerdict::Disabled, None, None);
        return Ok(());
    };
    let model = Some(cc_ctx.model.model.clone());
    let verdict = match handle_contradiction_preflight(
        paths, workspace, repo, chatops_ctx, change, &cc_ctx,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            // A Rust `Err` from the dispatch maps to FailedToRun and STILL
            // holds: write the marker/alert here (the handler's own error
            // paths already did so on their `Errored` arm; this covers the
            // unexpected-dispatch-error case).
            tracing::warn!(
                url = %repo.url,
                change = %change,
                "{} change-contradiction pre-flight errored unexpectedly; recording FAILED TO RUN (fail-closed): {e:#}",
                gate.label()
            );
            let _ = handle_gate_error(
                paths,
                workspace,
                repo,
                chatops_ctx,
                change,
                gate,
                &format!("gate dispatch errored: {e:#}"),
                cc_ctx.attribution.as_deref(),
            )
            .await;
            GateVerdict::FailedToRun
        }
    };
    ledger.set_in(verdict, model, gate_verdict_summary(gate, verdict));
    Ok(())
}

/// Run the `[canon]` gate's runner AND record its verdict (no-skip dispatch).
/// Mirrors [`run_in_gate`]: enabled → handler verdict; disabled → `Disabled`
/// stub; dispatch `Err` → `FailedToRun` + hold marker.
async fn run_canon_gate(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    change: &str,
    ledger: &mut crate::gate_ledger::GateLedger,
) -> Result<()> {
    use crate::gate_ledger::GateVerdict;
    let gate = crate::verifier_gate::VerifierGate::Canon;
    let enabled = crate::preflight::canon_contradiction::current().filter(|_| {
        matches!(
            crate::verifier_gate::GateRegistry::standard().resolve(gate),
            Some(crate::verifier_gate::GateImpl::CanonContradictionCheck)
        )
    });
    let Some(canon_ctx) = enabled else {
        ledger.set_canon(GateVerdict::Disabled, None, None);
        return Ok(());
    };
    let model = Some(canon_ctx.model.model.clone());
    let verdict = match handle_canon_contradiction_preflight(
        paths, workspace, repo, chatops_ctx, change, &canon_ctx,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                url = %repo.url,
                change = %change,
                "{} change-vs-canonical pre-flight errored unexpectedly; recording FAILED TO RUN (fail-closed): {e:#}",
                gate.label()
            );
            let _ = handle_gate_error(
                paths,
                workspace,
                repo,
                chatops_ctx,
                change,
                gate,
                &format!("gate dispatch errored: {e:#}"),
                canon_ctx.attribution.as_deref(),
            )
            .await;
            GateVerdict::FailedToRun
        }
    };
    ledger.set_canon(verdict, model, gate_verdict_summary(gate, verdict));
    Ok(())
}

/// Run the `[rules]` gate's runner AND record its verdict (no-skip dispatch,
/// global-rules-gate). Mirrors [`run_canon_gate`]: enabled (the scoped context
/// is `Some` AND the registry installs `GlobalRulesCheck`) → handler verdict;
/// disabled → `Disabled` stub; dispatch `Err` → `FailedToRun` + hold marker.
async fn run_rules_gate(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    change: &str,
    ledger: &mut crate::gate_ledger::GateLedger,
) -> Result<()> {
    use crate::gate_ledger::GateVerdict;
    let gate = crate::verifier_gate::VerifierGate::Rules;
    let enabled = crate::preflight::global_rules::current().filter(|_| {
        matches!(
            crate::verifier_gate::GateRegistry::standard().resolve(gate),
            Some(crate::verifier_gate::GateImpl::GlobalRulesCheck)
        )
    });
    let Some(rules_ctx) = enabled else {
        ledger.set_rules(GateVerdict::Disabled, None, None);
        return Ok(());
    };
    let model = Some(rules_ctx.model.model.clone());
    let verdict = match handle_rules_violations_preflight(
        paths, workspace, repo, chatops_ctx, change, &rules_ctx,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                url = %repo.url,
                change = %change,
                "{} global-rules pre-flight errored unexpectedly; recording FAILED TO RUN (fail-closed): {e:#}",
                gate.label()
            );
            let _ = handle_gate_error(
                paths,
                workspace,
                repo,
                chatops_ctx,
                change,
                gate,
                &format!("gate dispatch errored: {e:#}"),
                rules_ctx.attribution.as_deref(),
            )
            .await;
            GateVerdict::FailedToRun
        }
    };
    ledger.set_rules(verdict, model, gate_verdict_summary(gate, verdict));
    Ok(())
}

/// A one-line PR summary for a blocking-gate verdict. `Fail`/`FailedToRun`
/// carry a terse cause for the PR row; `Pass`/`Disabled`/`Pending` carry none.
/// The `Fail` noun is gate-specific: the `[rules]` gate writes "rule
/// violations" (global-rules-gate), the `[in]`/`[canon]` gates write
/// "contradiction findings" — both land in `.needs-spec-revision.json`.
fn gate_verdict_summary(
    gate: crate::verifier_gate::VerifierGate,
    verdict: crate::gate_ledger::GateVerdict,
) -> Option<String> {
    use crate::gate_ledger::GateVerdict;
    use crate::verifier_gate::VerifierGate;
    match verdict {
        GateVerdict::Fail => {
            let noun = match gate {
                VerifierGate::Rules => "rule violations",
                _ => "contradiction findings",
            };
            Some(format!("{noun} written to .needs-spec-revision.json"))
        }
        GateVerdict::FailedToRun => {
            Some("gate could not run — change held (see .needs-spec-revision.json)".into())
        }
        GateVerdict::Pass | GateVerdict::Disabled | GateVerdict::Pending => None,
    }
}

/// Short-circuit hold: if `verdict` is a blocking-fail (`Fail`/`FailedToRun`),
/// persist the ledger AND return the held step so the caller stops immediately —
/// the handler already wrote the marker/alert, AND we do NOT run a later gate on
/// a change already held (single-marker behavior). Returns `None` to proceed.
fn hold_if_blocking_fail(
    workspace: &Path,
    change: &str,
    verdict: crate::gate_ledger::GateVerdict,
    ledger: &crate::gate_ledger::GateLedger,
) -> Option<QueueStep> {
    use crate::gate_ledger::GateVerdict;
    if matches!(verdict, GateVerdict::Fail | GateVerdict::FailedToRun) {
        // Best-effort persist so the PR-render path (if it is ever reached for a
        // held change) sees the recorded verdict.
        let _ = crate::gate_ledger::write_ledger(workspace, change, ledger);
        return Some(QueueStep::SpecRevisionMarked);
    }
    None
}

#[derive(Debug)]
pub(crate) enum QueueStep {
    Archived,
    /// Same archive bookkeeping as `Archived`, but the implementation was
    /// already on the base branch — autocoder ran the archive move itself
    /// instead of treating Completed-without-diff as Failed. The walker
    /// uses this to flip the pass-level `includes_self_heal` flag, which
    /// adds a disclaimer paragraph to the PR body.
    ArchivedSelfHeal,
    /// The executor (or post-execution classification) marked this change
    /// as Failed. `reason` is either the executor's explicit Failed
    /// reason or a synthetic one for the no-op / lazy-archive cases.
    Failed {
        reason: String,
    },
    Escalated,
    AskUserExitEarly,
    /// The executor returned `SpecNeedsRevision`. The change's marker has
    /// been written and the chatops alert posted. The walker halts the
    /// queue this iteration (operator-action territory). Unlike `Failed`,
    /// this MUST NOT increment the perma-stuck counter — the marker
    /// handles exclusion directly, the counter is irrelevant here.
    SpecRevisionMarked,
    /// a27a1: the executor returned `IterationRequested`. The WIP has
    /// been committed + force-pushed to the agent branch AND the
    /// `.iteration-pending.json` marker has been written. The walker
    /// halts this iteration; the next polling iteration picks the
    /// change up first via the queue's marker-preference ordering.
    /// Unlike `Failed`, this MUST NOT increment the perma-stuck counter
    /// — iteration sequences are part of the normal lifecycle, not a
    /// repeat-execution-failure.
    IterationPending,
    /// a39: the executor returned `Aborted`. The subprocess was killed
    /// by the daemon's own SIGTERM cascade (operator-initiated
    /// shutdown). The `.in-progress` lock has been dropped AND the
    /// `.iteration-pending.json` marker (if any) has been left
    /// untouched. The walker halts this iteration; the next polling
    /// iteration after restart picks the change up fresh. Like
    /// `IterationPending`, this MUST NOT increment the perma-stuck
    /// counter — operator-initiated shutdown is not a repeat-execution-
    /// failure.
    Aborted,
}
