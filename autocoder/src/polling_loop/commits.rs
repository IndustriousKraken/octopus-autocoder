use super::*;

/// Run a polling pass up to and including any commits, but stop before push
/// and PR creation. Returns the names of changes archived during the pass.
/// The caller (production: `execute_one_pass`) is responsible for the
/// remote-side work; tests use this directly to verify commit-formation
/// behavior without needing a live GitHub endpoint or a writable remote.
#[allow(clippy::too_many_arguments)]
pub async fn run_pass_through_commits(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    executor: &dyn Executor,
    chatops_ctx: Option<&ChatOpsContext>,
    perma_stuck_threshold: u32,
    max_changes_per_pr: u32,
    audit_registry: &AuditRegistry,
    audits_cfg: Option<&AuditsConfig>,
    audit_settings: &HashMap<String, AuditSettings>,
    queued_audit_types: &std::collections::HashSet<String>,
) -> Result<(Vec<String>, bool)> {
    prepare_workspace_for_pass(paths, workspace, repo, github_cfg, chatops_ctx).await?;

    let pending_at_start = queue::list_pending(paths, workspace)?;
    let waiting_at_start = queue::list_waiting(workspace)?;
    tracing::info!(
        url = %repo.url,
        pending = pending_at_start.len(),
        waiting = waiting_at_start.len(),
        "polling pass starting"
    );

    // Pre-flight archive-collision filter on the pending list. Any change
    // whose dated archive path already exists on disk is excluded from the
    // queue walk entirely (a throttled chatops alert under
    // `AlertCategory::ArchiveCollision` is posted per excluded change) so
    // the executor is never invoked on a change that cannot land.
    let pending_filtered =
        apply_archive_collision_preflight(paths, workspace, repo, chatops_ctx, pending_at_start)
            .await;

    // Process waiting (escalated) changes BEFORE pending. Each resumes if
    // a human reply has arrived. Any change that comes back as Completed
    // with a diff goes into the `processed` list and will get pushed/PR'd
    // along with anything from the pending pass.
    let mut processed: Vec<String> = Vec::new();
    let mut includes_self_heal = false;
    if chatops_ctx.is_some() {
        let resumed = process_waiting_changes(
            paths,
            workspace,
            repo,
            executor,
            chatops_ctx,
            perma_stuck_threshold,
            max_changes_per_pr,
        )
        .await?;
        processed.extend(resumed);
    }

    // Same-repo block: if any change is STILL waiting after the resume
    // pass, skip the pending pass entirely for this iteration. Audits
    // still run after this gate — they are independent of queue state
    // and the operator-visible block is on the queue walk, not on
    // periodic maintenance.
    let still_waiting = queue::list_waiting(workspace)?;
    if !still_waiting.is_empty() {
        tracing::info!(
            url = repo.url.as_str(),
            "queue blocked for {}: {} change(s) still waiting on human reply: {}",
            repo.url,
            still_waiting.len(),
            still_waiting.join(", ")
        );
        run_due_audits_after_queue(
            paths,
            workspace,
            repo,
            audit_registry,
            audits_cfg,
            audit_settings,
            chatops_ctx,
            queued_audit_types,
        )
        .await;
        tracing::info!(
            url = %repo.url,
            committed = processed.len(),
            waiting = still_waiting.len(),
            "polling pass complete"
        );
        return Ok((processed, includes_self_heal));
    }

    // Same-repo block (a18): if any change carries an operator-action
    // marker (`.perma-stuck.json`, `.needs-spec-revision.json`, or
    // `.question.json` AskUser waiting) AND is NOT downgraded by a
    // companion `.ignore-for-queue.json`, halt the pending walk. The
    // operator opts a specific change out of blocking by stamping
    // `.ignore-for-queue.json` alongside the underlying marker.
    if handle_blocking_markers_gate(
        paths,
        workspace,
        repo,
        audit_registry,
        audits_cfg,
        audit_settings,
        chatops_ctx,
        queued_audit_types,
        processed.len(),
    )
    .await?
    {
        return Ok((processed, includes_self_heal));
    }

    let remaining = max_changes_per_pr.saturating_sub(processed.len() as u32);
    if remaining > 0 {
        let (pending_processed, pending_self_heal) = walk_queue(
            paths,
            workspace,
            repo,
            github_cfg,
            executor,
            chatops_ctx,
            perma_stuck_threshold,
            remaining,
            pending_filtered,
        )
        .await?;
        processed.extend(pending_processed);
        if pending_self_heal {
            includes_self_heal = true;
        }
    } else {
        tracing::info!(
            url = %repo.url,
            committed = processed.len(),
            cap = max_changes_per_pr,
            "resume step already filled the per-PR cap; skipping pending queue this iteration"
        );
    }

    // Periodic audits run AFTER the pending queue walk completes (was:
    // before list_pending). The reorder prevents an "audit storm" — many
    // audits becoming eligible at once after a HEAD change — from
    // monopolizing the daemon and starving pending changes. The
    // trade-off is that an audit's spec-writing outcome
    // (`AuditOutcome::SpecsWritten`) lands its new pending change
    // directories AFTER this iteration's queue walk has already finished;
    // those changes wait for the NEXT iteration's `list_pending`. The
    // audit's creation commit still ships in this iteration's PR.
    //
    // Iteration-level workspace-validity gate (see
    // `audits-require-valid-workspace`): the audit scheduler is only
    // reached when `ensure_initialized` returned Ok for this iteration.
    // The early `return Err(e)` on init failure above is the gate: if
    // the workspace can't be brought to a valid state at the start of
    // the iteration, this site is unreachable and `run_due_audits` is
    // never called, so audits cannot create broken-state side effects.
    // (Per-audit gates in each `Audit::run` catch the rarer case where
    // the workspace becomes invalid mid-iteration.)
    run_due_audits_after_queue(
        paths,
        workspace,
        repo,
        audit_registry,
        audits_cfg,
        audit_settings,
        chatops_ctx,
        queued_audit_types,
    )
    .await;

    let waiting_after = queue::list_waiting(workspace)?.len();
    tracing::info!(
        url = %repo.url,
        committed = processed.len(),
        waiting = waiting_after,
        "polling pass complete"
    );
    Ok((processed, includes_self_heal))
}

/// Bring the workspace to a clean, initialized state for a pass: fork/refork,
/// ensure_initialized, stale-lock clearing, mid-iteration dirty recovery, and
/// the base-branch sync (fetch/checkout/pull/recreate + RAG init hook).
/// Extracted from `run_pass_through_commits` (a68 function-size split).
async fn prepare_workspace_for_pass(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    chatops_ctx: Option<&ChatOpsContext>,
) -> Result<()> {
    let fork_url = match github_cfg.fork_owner.as_deref() {
        Some(owner) => Some(crate::github::derive_fork_url(&repo.url, owner)?),
        None => None,
    };
    let did_clone = !workspace.exists();
    let mut did_refork = false;
    if did_clone && fork_url.is_some() && github_cfg.recreate_fork_on_reinit {
        match workspace::recreate_fork(github_cfg, repo).await {
            Ok(workspace::RecreateOutcome::Recreated) => {
                did_refork = true;
            }
            Ok(workspace::RecreateOutcome::Forbidden) => {
                // Helper already logged ERROR with scope guidance. Fall
                // through to the conservative ensure_initialized path so
                // the iteration still makes progress.
            }
            Err(e) => {
                tracing::error!(
                    url = %repo.url,
                    "recreate_fork failed: {e:#}; falling back to conservative ensure_initialized"
                );
            }
        }
    }
    let fork_arg = fork_url.as_deref().map(|u| (u, repo.agent_branch.as_str()));
    if let Err(e) = workspace::ensure_initialized(paths, workspace, &repo.url, fork_arg) {
        let class = classify_recovery_failure(&e);
        log_classified_recovery_failure(&repo.url, "workspace_init", class, &e);
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
        return Err(e);
    }
    if did_refork {
        maybe_post_refork_notification(repo, chatops_ctx).await;
    }
    let _cleared = queue::clear_stale_locks(workspace)?;

    let dirty = git::status_porcelain(workspace)?;
    // Post-`a16`, alert-state lives in `<state_dir>/alert-state/...`,
    // outside the workspace, so this filter is a defensive no-op for
    // normal operation. It still runs to catch transient `.alert-state.json`
    // files that linger before the first-startup migration completes
    // (e.g., a fresh re-clone of a repo whose history transiently
    // included it).
    let dirty_filtered = filter_alert_state_lines(&dirty);
    if !dirty_filtered.is_empty() {
        let dirty_count = dirty_filtered.lines().count();
        tracing::warn!(
            url = repo.url.as_str(),
            workspace = %workspace.display(),
            "workspace dirty mid-iteration ({dirty_count} entries); attempting recovery (git reset --hard origin/{} + git clean -fd)",
            repo.base_branch
        );
        match attempt_dirty_workspace_recovery(workspace, &repo.base_branch) {
            Ok(()) => {
                let recheck = git::status_porcelain(workspace)?;
                let recheck_filtered = filter_alert_state_lines(&recheck);
                if recheck_filtered.is_empty() {
                    tracing::info!(
                        url = repo.url.as_str(),
                        "workspace recovered mid-iteration; proceeding"
                    );
                } else {
                    let e = anyhow!(
                        "workspace {} still dirty after recovery; refusing to proceed:\n{recheck_filtered}",
                        workspace.display()
                    );
                    let class = classify_recovery_failure(&e);
                    log_classified_recovery_failure(&repo.url, "dirty_recheck", class, &e);
                    handle_classified_recovery_failure(
                        paths,
                        workspace,
                        &repo.url,
                        chatops_ctx,
                        chatops_ctx
                            .map(|c| c.failure_alerts_enabled)
                            .unwrap_or(false),
                        AlertCategory::WorkspaceDirtyMidIteration,
                        &e,
                        class,
                    )
                    .await;
                    return Err(e);
                }
            }
            Err(recovery_err) => {
                let e = anyhow!(
                    "dirty-workspace recovery failed: {recovery_err:#}; original dirty state:\n{dirty_filtered}"
                );
                let class = classify_recovery_failure(&e);
                log_classified_recovery_failure(&repo.url, "dirty_cleanup", class, &e);
                handle_classified_recovery_failure(
                    paths,
                    workspace,
                    &repo.url,
                    chatops_ctx,
                    chatops_ctx
                        .map(|c| c.failure_alerts_enabled)
                        .unwrap_or(false),
                    AlertCategory::WorkspaceDirtyMidIteration,
                    &e,
                    class,
                )
                .await;
                return Err(e);
            }
        }
    }

    if let Err(e) = git::fetch(workspace) {
        let class = classify_recovery_failure(&e);
        log_classified_recovery_failure(&repo.url, "git_fetch", class, &e);
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
        return Err(e);
    }
    // OSS-fork support (a26): opportunistic upstream fetch.
    // Best-effort — failures log a WARN but never block the iteration.
    opportunistic_upstream_fetch(workspace, repo);
    git::checkout(workspace, &repo.base_branch)?;
    git::pull_ff_only(workspace, &repo.base_branch)?;
    git::recreate_branch(workspace, &repo.agent_branch)?;

    // Canonical-spec RAG workspace-init hook (a21). Idempotent: only
    // builds + registers the store on the first iteration of a given
    // workspace (a previously-registered store is left alone). Fail-open
    // — any error logs WARN and the store is omitted from the registry.
    crate::rag::workspace_init_hook(workspace).await;
    Ok(())
}

/// Operator-action-marker queue gate (a18). When a blocking marker is present
/// and not downgraded, run due audits and signal the caller to stop the
/// pending walk. Returns true when the queue is blocked. Extracted from
/// `run_pass_through_commits` (a68 split).
#[allow(clippy::too_many_arguments)]
async fn handle_blocking_markers_gate(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    audit_registry: &AuditRegistry,
    audits_cfg: Option<&AuditsConfig>,
    audit_settings: &HashMap<String, AuditSettings>,
    chatops_ctx: Option<&ChatOpsContext>,
    queued_audit_types: &std::collections::HashSet<String>,
    committed_count: usize,
) -> Result<bool> {
    let blocking_markers = queue::find_queue_blocking_markers(workspace)?;
    if !blocking_markers.is_empty() {
        for bm in &blocking_markers {
            let marker_path = workspace
                .join("openspec/changes")
                .join(&bm.change)
                .join(&bm.marker);
            tracing::info!(
                url = repo.url.as_str(),
                change = %bm.change,
                marker = %bm.marker,
                path = %marker_path.display(),
                "queue blocked: change `{}` has `{}` (not downgraded by .ignore-for-queue.json)",
                bm.change,
                bm.marker
            );
        }
        run_due_audits_after_queue(
            paths,
            workspace,
            repo,
            audit_registry,
            audits_cfg,
            audit_settings,
            chatops_ctx,
            queued_audit_types,
        )
        .await;
        tracing::info!(
            url = %repo.url,
            committed = committed_count,
            blocked = blocking_markers.len(),
            "polling pass complete (queue blocked by operator-action markers)"
        );
        return Ok(true);
    }
    Ok(false)
}

/// Log a mid-iteration recovery failure with its classification (transient
/// vs. permanent). Transient → WARN (network blips are noisy but
/// self-recovering); Permanent → ERROR (operator must inspect). The
/// `site` field names the call site (`workspace_init`, `git_fetch`,
/// `dirty_cleanup`, `dirty_recheck`) so journalctl filters can scope to
/// a specific stage.
fn log_classified_recovery_failure(
    repo_url: &str,
    site: &'static str,
    class: RecoveryFailureClass,
    err: &anyhow::Error,
) {
    match class {
        RecoveryFailureClass::Transient => tracing::warn!(
            url = repo_url,
            site,
            class = class.log_tag(),
            "mid-iteration recovery failed (will retry next iteration): {err:#}"
        ),
        RecoveryFailureClass::Permanent => tracing::error!(
            url = repo_url,
            site,
            class = class.log_tag(),
            "mid-iteration recovery failed (operator inspection required): {err:#}"
        ),
    }
}

/// Attempt to recover a workspace whose pre-pass dirty check tripped.
/// Mirrors the startup recovery in `cli/run.rs::repo_passes_startup_check`:
/// best-effort `git checkout <base>` (might fail if uncommitted
/// modifications would be overwritten — that's fine, the next step forces
/// the issue), then `git reset --hard origin/<base>`, then `git clean -fd`.
///
/// Safe in the per-iteration position because the agent branch is rebuilt
/// from base each iteration via `recreate_branch`; wholesale wiping does
/// not lose recoverable work. The caller is responsible for re-checking
/// `git status --porcelain` after this returns.
pub(crate) fn attempt_dirty_workspace_recovery(workspace: &Path, base_branch: &str) -> Result<()> {
    let _ = git::checkout(workspace, base_branch);
    git::reset_hard_to_remote(workspace, base_branch)
        .with_context(|| format!("git reset --hard origin/{base_branch}"))?;
    git::clean_force(workspace).with_context(|| "git clean -fd".to_string())?;
    Ok(())
}
