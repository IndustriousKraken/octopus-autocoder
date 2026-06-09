use super::*;

/// Single-pass workflow: workspace init → stale-lock cleanup → dirty-workspace
/// check → branch recreation → queue walk → push + PR if commits were
/// produced.
#[allow(clippy::too_many_arguments)]
pub async fn execute_one_pass(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    executor: &dyn Executor,
    github_cfg: &GithubConfig,
    reviewer: Option<&CodeReviewer>,
    chatops_ctx: Option<&ChatOpsContext>,
    stuck_threshold_secs: u64,
    perma_stuck_threshold: u32,
    max_changes_per_pr: u32,
    revision_cap: u32,
    human_revise_cap: u32,
    audit_registry: &AuditRegistry,
    audits_cfg: Option<&AuditsConfig>,
    audit_settings: &HashMap<String, AuditSettings>,
    queued_audit_types: &std::collections::HashSet<String>,
) -> Result<()> {
    // Acquire the per-repo busy marker. Held across the entire pass
    // (executor → review → push → PR); released by Drop on every return.
    // A crash that bypasses Drop leaves the marker for the next pass to
    // detect and (depending on age + PID liveness) auto-recover from.
    let mut guard = match busy_marker::try_acquire(
        paths,
        workspace,
        &repo.url,
        stuck_threshold_secs,
    ) {
        Ok(busy_marker::AcquireOutcome::Acquired(g)) => g,
        Ok(busy_marker::AcquireOutcome::SkipFreshInProgress(details)) => {
            tracing::info!(
                url = %repo.url,
                pid = details.marker.pid,
                stage = %details.marker.stage.as_str(),
                age = %busy_marker::format_age_human(details.age_secs),
                threshold = %busy_marker::format_age_human(details.threshold_secs),
                pid_alive = details.pid_alive,
                recovery_eligible = details.recovery_eligible(),
                "busy marker present; skipping iteration"
            );
            return Ok(());
        }
        Ok(busy_marker::AcquireOutcome::SkipAmbiguous(m)) => {
            tracing::error!(
                url = %repo.url,
                pid = m.pid,
                recorded_comm = %m.comm,
                "busy marker is stuck with ambiguous PID state; skipping iteration — investigate manually"
            );
            post_stuck_alert(chatops_ctx, repo, &m, true).await;
            return Ok(());
        }
        Err(e) => {
            tracing::error!(url = %repo.url, "busy marker acquire failed: {e:#}");
            return Err(e);
        }
    };

    // Run the PR-comment revision dispatcher BEFORE the open-PR
    // short-circuit so revisions reach open PRs. A v1 simplification:
    // when `revision_cap` is `0`, the feature is disabled entirely.
    run_revision_dispatchers(
        paths,
        workspace,
        repo,
        github_cfg,
        reviewer,
        executor,
        chatops_ctx,
        revision_cap,
        human_revise_cap,
    )
    .await;

    // Before doing any iteration work, check whether an open PR already
    // exists on the agent branch. If yes, this iteration would burn
    // tokens re-implementing, force-update the PR's commits under any
    // reviewer mid-review, and 422 at PR creation. Skip entirely.
    if open_pr_exists_for_agent_branch(paths, repo, github_cfg).await {
        return Ok(());
    }
    let (processed, includes_self_heal) = run_pass_through_commits(
        paths,
        workspace,
        repo,
        github_cfg,
        executor,
        chatops_ctx,
        perma_stuck_threshold,
        max_changes_per_pr,
        audit_registry,
        audits_cfg,
        audit_settings,
        queued_audit_types,
    )
    .await?;

    if should_stop_after_commit_check(paths, workspace, repo)? {
        return Ok(());
    }

    let (review_report, draft, reviewer_revision_concerns) = run_reviewer_step(
        workspace,
        repo,
        &processed,
        reviewer,
        chatops_ctx,
        revision_cap,
        &mut guard,
    )
    .await?;

    // a63: the `[out]` gate — code-implements-spec verification. Runs AFTER
    // the executor implemented the change(s) (the agent branch carries the
    // implementation), before PR-body assembly, when the operator opted in
    // (`code_implements_spec::current()` is `Some`) AND there are
    // implementer-processed changes to verify. Advisory: it renders a
    // `## Spec Verification` PR-body section AND posts a chatops note ONLY when
    // gaps are found; it NEVER opens a revision AND NEVER blocks PR creation. A
    // gate failure WARNs (labeled `[verifier:out]`) AND omits the section.
    let spec_verification_section =
        run_spec_verification_gate(workspace, repo, &processed, chatops_ctx).await;

    let push_remote = if github_cfg.fork_owner.is_some() {
        "fork"
    } else {
        "origin"
    };
    let _ = guard.set_stage(busy_marker::Stage::Push);
    if let Err(e) = git::push_force_with_lease(workspace, &repo.agent_branch, push_remote) {
        handle_predictable_failure(
            paths,
            workspace,
            &repo.url,
            chatops_ctx,
            chatops_ctx
                .map(|c| c.failure_alerts_enabled)
                .unwrap_or(false),
            AlertCategory::BranchPushFailure,
            &e,
        )
        .await;
        return Err(e);
    }
    let _ = guard.set_stage(busy_marker::Stage::Pr);
    open_pull_request(
        paths,
        repo,
        github_cfg,
        &processed,
        includes_self_heal,
        review_report.as_ref(),
        reviewer,
        revision_cap,
        draft,
        &reviewer_revision_concerns,
        chatops_ctx,
        workspace,
        spec_verification_section.as_deref(),
    )
    .await?;
    // End-of-pass success: push and PR creation both succeeded. Clear the
    // entire alert-state map so the next failure (whatever category) re-
    // alerts immediately. Per design.md, this is intentionally coarse —
    // any successful iteration resets every category's throttle.
    if let Err(e) = AlertState::clear(paths, workspace) {
        tracing::warn!(
            url = %repo.url,
            "failed to clear alert-state on success: {e:#}"
        );
    }
    Ok(())
}

/// Run the PR-comment revision dispatcher and the chat-driven changelog
/// revision dispatcher (both best-effort) before the open-PR short-circuit.
/// Extracted from `execute_one_pass` (a68 function-size split).
#[allow(clippy::too_many_arguments)]
async fn run_revision_dispatchers(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    reviewer: Option<&CodeReviewer>,
    executor: &dyn Executor,
    chatops_ctx: Option<&ChatOpsContext>,
    revision_cap: u32,
    human_revise_cap: u32,
) {
    if revision_cap > 0 {
        let chatops_ctx_for_revisions = chatops_ctx.map(|c| crate::revisions::ChatOpsCtx {
            chatops: c.chatops.as_ref(),
            channel: c.channel.as_str(),
            failure_alerts_enabled: c.failure_alerts_enabled,
        });
        if let Err(e) = crate::revisions::process_revision_requests(
            paths,
            workspace,
            repo,
            github_cfg,
            reviewer,
            executor,
            chatops_ctx_for_revisions,
            revision_cap,
            human_revise_cap,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        {
            tracing::warn!(
                url = %repo.url,
                "revision dispatcher errored (iteration continues): {e:#}"
            );
        }
        // Same dispatcher pattern for chat-driven changelog PRs (per
        // `a06-chat-driven-changelog`): walk open PRs whose head matches
        // `changelog-*` AND re-run the stylist on revision triggers.
        if let Err(e) = crate::changelog_triage::process_changelog_revision_requests(
            paths,
            workspace,
            repo,
            github_cfg,
            executor,
            chatops_ctx,
        )
        .await
        {
            tracing::warn!(
                url = %repo.url,
                "changelog-revision dispatcher errored (iteration continues): {e:#}"
            );
        }
    }
}

/// Commit-count + spec-storage termination gate. Returns true when the pass
/// should stop early (no commits, or iteration-pending markers suppress the
/// audit-only PR). Clears alert-state on the stop paths. Extracted from
/// `execute_one_pass` (a68 function-size split).
fn should_stop_after_commit_check(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
) -> Result<bool> {
    // a34: "detect working-tree state" prelude per the canonical
    // orchestrator-cli requirement. Before the iteration's commit +
    // push + PR step, classify the iteration's outcome by probing the
    // spec_storage tree's uncommitted state. The workspace's status
    // is implicit: the agent-branch commit count below is the
    // primary signal for "code-only has work". The spec_storage
    // tree's dirty state — populated by brownfield / scout spec-it /
    // archive flows when `spec_storage` is configured — is logged
    // here so operators see which routing branch the iteration is
    // about to take. The full spec-storage commit + push + PR fanout
    // lives in `crate::spec_storage_routing` AND is exercised by the
    // brownfield / scout / archive callers when they route through
    // the new helpers.
    let spec_storage_resolved = repo.resolved_spec_storage_dir(workspace);
    let spec_storage_dirty = match spec_storage_resolved.as_deref() {
        Some(p) => match git::status_porcelain(p) {
            Ok(s) => !s.is_empty(),
            Err(e) => {
                tracing::warn!(
                    url = %repo.url,
                    spec_storage_path = %p.display(),
                    "spec_storage status_porcelain probe failed; treating tree as clean: {e:#}"
                );
                false
            }
        },
        None => false,
    };

    // Termination is gated EXCLUSIVELY on the agent branch's commit count
    // relative to base — see `polling-iteration-termination-is-commit-count
    // -gated`. Using `processed.is_empty()` would miss commits produced by
    // the audit phase that runs AFTER the queue walk, silently dropping
    // them on the next iteration's recreate_branch step.
    let range = format!("{}..{}", repo.base_branch, repo.agent_branch);
    let commit_count = git::rev_list_count(workspace, &range)?;
    if commit_count == 0 {
        if spec_storage_dirty {
            tracing::info!(
                url = %repo.url,
                spec_storage_path = ?spec_storage_resolved.as_ref().map(|p| p.display().to_string()),
                "a34: spec_storage tree dirty AND workspace has no commits — spec-only iteration classified; spec-storage routing handled by the originating brownfield / scout / archive caller"
            );
        } else {
            tracing::info!(
                url = repo.url.as_str(),
                "polling pass produced no commits (all completed changes had empty diffs)"
            );
        }
        let _ = AlertState::clear(paths, workspace);
        return Ok(true);
    }
    if spec_storage_dirty {
        tracing::info!(
            url = %repo.url,
            spec_storage_path = ?spec_storage_resolved.as_ref().map(|p| p.display().to_string()),
            workspace_commit_count = commit_count,
            "a34: dual-tree iteration classified — workspace commits push as code-only PR; spec-storage routing handled by the originating brownfield / scout / archive caller"
        );
    }

    // a38: audit-only-PR suppression on iteration-pending state. When
    // any `.iteration-pending.json` marker is present in the workspace,
    // the agent-branch's commits-ahead-of-master include iteration_request
    // WIP that is explicitly not ready to ship (per a27a1). Opening a PR
    // on top of that WIP produces a "0 change(s)" PR that misleads the
    // operator AND, if merged, locks in half-done iteration work.
    // Suppress the push + PR steps for this iteration; audit-produced
    // commits (if any) remain on agent-q AND ship in the next iteration
    // after the iteration-pending change concludes via outcome_success,
    // outcome_spec_needs_revision, OR the a27a1 5-iteration cap.
    let pending_iteration_changes = {
        let basename = workspace
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        crate::iteration_pending::list_pending_changes(paths, basename)
    };
    if !pending_iteration_changes.is_empty() {
        tracing::info!(
            url = %repo.url,
            pending = %pending_iteration_changes.join(","),
            workspace_commit_count = commit_count,
            "a38: audit-only PR path suppressed: iteration-pending markers present for {}; deferring push + PR until iteration sequence concludes",
            pending_iteration_changes.join(", ")
        );
        let _ = AlertState::clear(paths, workspace);
        return Ok(true);
    }
    Ok(false)
}

/// Run the reviewer step (skip-decision, agentic/oneshot review, revision
/// partitioning) and return the review report, draft flag, and taken
/// reviewer-revision concerns. Extracted from `execute_one_pass` (a68 split).
async fn run_reviewer_step(
    workspace: &Path,
    repo: &RepositoryConfig,
    processed: &[String],
    reviewer: Option<&CodeReviewer>,
    chatops_ctx: Option<&ChatOpsContext>,
    revision_cap: u32,
    guard: &mut busy_marker::BusyGuard,
) -> Result<(Option<ReviewReport>, bool, Vec<ReviewConcern>)> {
    // Reviewer step (if configured) runs against the produced commits BEFORE
    // the push + PR. A failed reviewer is non-fatal: PR still ships with a
    // "(reviewer failed)" note in the body.
    //
    // When `reviewer.auto_revise` is enabled, the per-concern
    // `should_request_revision` records drive the reviewer-initiated
    // revision pipeline regardless of the verdict. Concerns are
    // partitioned against the per-PR cap budget here; the taken set is
    // queued to be posted as `<!-- reviewer-revision -->` PR comments
    // after the PR is created, and the dropped set is annotated into the
    // `## Code Review` PR-body section so the human sees what was skipped.
    // a34 §6: when `reviewer.skip_spec_only_prs: true` AND the PR's
    // diff lives entirely under `openspec/`, skip the reviewer call
    // (cost-optimization knob). The detection mirrors the iteration's
    // commit + push classification — a PR opened from a spec-only
    // iteration's classification is a spec-only PR; a code-only
    // iteration's PR (including dual-tree's code half) is NOT.
    let skip_reviewer_for_spec_only_pr = if let Some(r) = reviewer
        && r.skip_spec_only_prs()
    {
        let diff_paths = git::diff_files_changed(workspace, &repo.base_branch, &repo.agent_branch)
            .unwrap_or_default();
        let spec_only = crate::spec_storage_routing::diff_is_spec_only(&diff_paths);
        if spec_only {
            tracing::info!(
                url = %repo.url,
                "reviewer: skipping spec-only PR per skip_spec_only_prs config"
            );
        }
        spec_only
    } else {
        false
    };

    let (review_report, draft, reviewer_revision_concerns) = if processed.is_empty()
        || skip_reviewer_for_spec_only_pr
    {
        // Audit-only iteration: no implementer-touched files to evaluate.
        // The audit's own validation pass already gated each proposal, so
        // the reviewer would either error against an empty `processed`
        // list or produce a meaningless review of mechanical
        // proposal-writing. Skip the reviewer entirely.
        //
        // a34 §6 also skips here when `reviewer.skip_spec_only_prs` AND
        // the iteration's diff is entirely under `openspec/`.
        (None, false, Vec::new())
    } else {
        match reviewer {
            None => (None, false, Vec::new()),
            Some(r) => {
                let _ = guard.set_stage(busy_marker::Stage::Review);
                match r.kind() {
                    // a58: agentic transport — run the read-only CLI-wrapped
                    // session(s) and consume the schema-validated verdict. A
                    // session that records no valid submission DISCARDS the
                    // review (no verdict written, NOT an implicit Approve) AND
                    // posts the reviewer-failure operator alert.
                    crate::config::ReviewerKind::Agentic => {
                        let ctx = build_review_context(workspace, repo, processed, r.kind())?;
                        match crate::code_reviewer::run_agentic_review(r, &ctx, workspace).await {
                            Ok(crate::code_reviewer::AgenticReviewOutcome::Reviewed(result)) => {
                                let report = result.into_review_report();
                                let draft = matches!(report.verdict, ReviewVerdict::Block);
                                let taken =
                                    reviewer_revisions_for_review(r, &report, draft, revision_cap);
                                (Some(report), draft, taken)
                            }
                            Ok(crate::code_reviewer::AgenticReviewOutcome::Discarded {
                                reason,
                            }) => {
                                tracing::error!(url = %repo.url, "agentic reviewer discarded: {reason}");
                                post_reviewer_discarded_alert(chatops_ctx, repo, &reason).await;
                                (None, false, Vec::new())
                            }
                            Err(e) => {
                                tracing::error!(url = %repo.url, "agentic reviewer failed: {e:#}");
                                post_reviewer_discarded_alert(
                                    chatops_ctx,
                                    repo,
                                    &format!("agentic reviewer failed: {e}"),
                                )
                                .await;
                                (None, false, Vec::new())
                            }
                        }
                    }
                    crate::config::ReviewerKind::Oneshot => {
                        let outcome = match r.mode() {
                            crate::config::ReviewerMode::Bundled => {
                                let ctx =
                                    build_review_context(workspace, repo, processed, r.kind())?;
                                r.review(&ctx).await
                            }
                            crate::config::ReviewerMode::PerChange => {
                                let contexts =
                                    build_per_change_contexts(workspace, repo, processed)?;
                                r.review_per_change(&contexts).await.map(|per_change| {
                                    crate::code_reviewer::synthesize_per_change_report(per_change)
                                })
                            }
                        };
                        match outcome {
                            Ok(report) => {
                                let draft = matches!(report.verdict, ReviewVerdict::Block);
                                let taken =
                                    reviewer_revisions_for_review(r, &report, draft, revision_cap);
                                (Some(report), draft, taken)
                            }
                            Err(e) => {
                                tracing::error!("reviewer failed: {e:#}");
                                let synthetic = ReviewReport {
                                    verdict: ReviewVerdict::Concerns,
                                    markdown: format!("(reviewer failed: {e})"),
                                    concerns: Vec::new(),
                                    per_change_sections: Vec::new(),
                                    // Reviewer failed before producing a verdict;
                                    // no model output to attribute (a49).
                                    attribution: None,
                                };
                                (Some(synthetic), false, Vec::new())
                            }
                        }
                    }
                }
            }
        }
    };
    Ok((review_report, draft, reviewer_revision_concerns))
}

/// Run the `[out]` gate — code-implements-spec verification (a63) — and return
/// the advisory `## Spec Verification` PR-body section to splice in, or `None`
/// to omit it. The gate is a no-op (returns `None`) when the operator did not
/// opt in (`code_implements_spec::current()` is `None`) OR the iteration has no
/// implementer-processed changes to verify (audit-only). On an `implemented` or
/// `gaps_found` verdict it renders the section; a `gaps_found` verdict ALSO
/// posts an advisory chatops note. On a gate failure the module already WARNed
/// (labeled `[verifier:out]`); this returns `None` (omit the section). The gate
/// NEVER opens a revision AND NEVER blocks PR creation — the caller proceeds
/// with the PR regardless of the verdict.
pub(crate) async fn run_spec_verification_gate(
    workspace: &Path,
    repo: &RepositoryConfig,
    processed: &[String],
    chatops_ctx: Option<&ChatOpsContext>,
) -> Option<String> {
    let ctx = crate::code_implements_spec::current()?;
    if processed.is_empty() {
        // Audit-only iteration: no implementer-touched files to verify.
        return None;
    }
    let label = crate::verifier_gate::VerifierGate::Out.label();
    let diff = match git::diff_three_dot(workspace, &repo.base_branch, &repo.agent_branch) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(
                url = %repo.url,
                "{label} could not compute diff for the code-implements-spec gate; omitting the Spec Verification section (advisory, never blocks): {e:#}"
            );
            return None;
        }
    };
    let changed_files = git::diff_files_changed(workspace, &repo.base_branch, &repo.agent_branch)
        .unwrap_or_default();
    match crate::code_implements_spec::run_code_implements_spec_check(
        &ctx,
        workspace,
        processed,
        &diff,
        &changed_files,
    )
    .await
    {
        crate::code_implements_spec::SpecVerificationOutcome::Verified(verification) => {
            // Post the advisory chatops heads-up ONLY when gaps are found.
            if verification.has_gaps() {
                post_spec_verification_gaps_alert(chatops_ctx, repo, &verification).await;
            }
            Some(crate::code_implements_spec::render_spec_verification_section(&verification))
        }
        crate::code_implements_spec::SpecVerificationOutcome::FailedToRun { cause } => {
            // gatekeepers-fail-closed: the advisory gate fails to a VISIBLE
            // state, not silence — render an explicit FAILED TO RUN section so an
            // un-run gate is not mistaken for a clean pass. Still never blocks.
            Some(crate::code_implements_spec::render_spec_verification_failed_section(&cause))
        }
    }
}
