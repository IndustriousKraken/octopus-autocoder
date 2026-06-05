use super::*;

/// Run a single rebuild iteration: acquire the busy marker, ensure the
/// workspace is on a clean agent branch, run the rebuild, commit + push,
/// open a PR if drift was found, and post the end-of-rebuild chatops
/// notification.
///
/// Failures from individual archived changes are accumulated in the
/// `RebuildReport` and do NOT abort the iteration. A failure to push or
/// open the PR is propagated as the iteration's Err — the chatops
/// notification still fires (best-effort, separate code path).
pub(crate) async fn execute_rebuild_iteration(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    stuck_threshold_secs: u64,
) -> Result<()> {
    let mut guard =
        match busy_marker::try_acquire(paths, workspace, &repo.url, stuck_threshold_secs) {
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
                    "rebuild iteration: busy marker held by another pass; will retry next iteration"
                );
                return Ok(());
            }
            Ok(busy_marker::AcquireOutcome::SkipAmbiguous(m)) => {
                tracing::error!(
                    url = %repo.url,
                    pid = m.pid,
                    "rebuild iteration: ambiguous busy-marker state; skipping"
                );
                post_stuck_alert(chatops_ctx, repo, &m, true).await;
                return Ok(());
            }
            Err(e) => return Err(e),
        };

    tracing::info!(
        url = %repo.url,
        "iteration: running spec rebuild instead of queue walk"
    );

    // Make sure the workspace is initialized + on a clean agent branch
    // before we mutate openspec/specs/. We reuse the existing setup that
    // run_pass_through_commits performs to keep behavior identical.
    let fork_url = match github_cfg.fork_owner.as_deref() {
        Some(owner) => Some(crate::github::derive_fork_url(&repo.url, owner)?),
        None => None,
    };
    let fork_arg = fork_url.as_deref().map(|u| (u, repo.agent_branch.as_str()));
    workspace::ensure_initialized(paths, workspace, &repo.url, fork_arg)?;

    // If the workspace is dirty (e.g. a SIGTERMed iteration left state),
    // try to recover. Failure to recover is fatal for this iteration.
    let dirty = git::status_porcelain(workspace)?;
    let dirty_filtered = filter_alert_state_lines(&dirty);
    if !dirty_filtered.is_empty() {
        tracing::warn!(
            url = %repo.url,
            "rebuild iteration: workspace dirty; attempting recovery"
        );
        attempt_dirty_workspace_recovery(workspace, &repo.base_branch)?;
    }
    git::fetch(workspace)?;
    git::checkout(workspace, &repo.base_branch)?;
    git::pull_ff_only(workspace, &repo.base_branch)?;
    git::recreate_branch(workspace, &repo.agent_branch)?;

    let _ = guard.set_stage(busy_marker::Stage::Commit);
    let report = crate::cli::sync_specs::rebuild_canonical(workspace).await?;
    tracing::info!(
        url = %repo.url,
        processed = report.processed,
        successful = report.successful,
        failed = report.failed,
        modified_files = report.modified_files(),
        prefix_renames = report.prefix_renames.len(),
        aborted = report.abort_reason.is_some(),
        "rebuild_canonical finished"
    );

    // If the dependency pre-pass aborted the rebuild, there is no PR to
    // open and no canonical-spec drift to push. Post the `❌` chatops
    // notification and exit early.
    if report.abort_reason.is_some() {
        maybe_post_rebuild_abort_notification(repo, &report, chatops_ctx).await;
        return Ok(());
    }

    // If the pre-pass applied prefix renames, post the `🔀` chatops
    // notification BEFORE staging/pushing/PR so operators see the
    // renames first. Best-effort: a failed post does not block PR
    // creation.
    if !report.prefix_renames.is_empty() {
        maybe_post_rebuild_renames_notification(repo, &report, chatops_ctx).await;
    }

    // Stage everything: openspec/specs/ changes AND any archive directory
    // moves (the in-place rename shouldn't produce a net diff but we
    // stage defensively).
    git::add_all(workspace)?;

    let porcelain = git::status_porcelain(workspace)?;
    let staged = filter_alert_state_lines(&porcelain);
    let mut pr_url: Option<String> = None;

    if staged.is_empty() {
        tracing::info!(
            url = %repo.url,
            "rebuild iteration: no drift detected — skipping commit/push/PR"
        );
    } else {
        let modified = report.modified_files();
        let subject = format!(
            "spec rebuild: {modified} capability(ies) rebuilt from {} archived change(s)",
            report.successful
        );
        git::commit(workspace, &subject)?;
        let push_remote = if github_cfg.fork_owner.is_some() {
            "fork"
        } else {
            "origin"
        };
        let _ = guard.set_stage(busy_marker::Stage::Push);
        git::push_force_with_lease(workspace, &repo.agent_branch, push_remote)?;

        let _ = guard.set_stage(busy_marker::Stage::Pr);
        match open_rebuild_pull_request(paths, repo, github_cfg, &report).await {
            Ok(url) => {
                pr_url = Some(url);
            }
            Err(e) => {
                tracing::error!(
                    url = %repo.url,
                    "rebuild iteration: PR creation failed: {e:#}"
                );
                // We still want to send the chatops notification so the
                // operator knows the rebuild happened (and that the PR
                // step failed). Propagate err after the notification.
                maybe_post_end_of_rebuild_notification(repo, &report, None, chatops_ctx).await;
                return Err(e);
            }
        }
    }

    maybe_post_end_of_rebuild_notification(repo, &report, pr_url.as_deref(), chatops_ctx).await;
    Ok(())
}

pub(crate) fn truncate_one_line(s: &str, n: usize) -> String {
    let one = s.lines().next().unwrap_or("");
    if one.chars().count() <= n {
        one.to_string()
    } else {
        one.chars().take(n).collect::<String>() + "…"
    }
}

/// Render a list of `RenameRecord`s grouped by day, in the format shared
/// between the `🔀` chatops notification and the PR body's renames
/// section. Each entry: `<from> → <to>` followed by an indented
/// parenthetical `(<dependency_summary>)`.
pub(crate) fn render_prefix_renames_markdown(
    renames: &[crate::cli::sync_specs_deps::RenameRecord],
) -> String {
    use std::collections::BTreeMap;
    let mut grouped: BTreeMap<&str, Vec<&crate::cli::sync_specs_deps::RenameRecord>> =
        BTreeMap::new();
    for r in renames {
        grouped.entry(r.day.as_str()).or_default().push(r);
    }
    let mut out = String::new();
    for (day, group) in grouped {
        out.push_str(&format!("  {day}:\n"));
        for r in group {
            out.push_str(&format!("    {} → {}\n", r.from, r.to));
            if !r.dependency_summary.is_empty() {
                out.push_str(&format!("      ({})\n", r.dependency_summary));
            }
        }
    }
    out
}

/// Count how many distinct days appear in a list of `RenameRecord`s.
fn count_distinct_days(renames: &[crate::cli::sync_specs_deps::RenameRecord]) -> usize {
    use std::collections::BTreeSet;
    renames
        .iter()
        .map(|r| r.day.as_str())
        .collect::<BTreeSet<_>>()
        .len()
}

/// Format the `🔀` chatops notification text announcing applied
/// dependency-prefix renames. Pure function for snapshot-testing.
pub(crate) fn format_rebuild_renames_notification(
    repo_url: &str,
    renames: &[crate::cli::sync_specs_deps::RenameRecord],
) -> String {
    let n_days = count_distinct_days(renames);
    let mut out = format!(
        "🔀 `{repo_url}`: rebuild applied dependency-prefix renames in {n_days} day-group(s)\n"
    );
    out.push_str(&render_prefix_renames_markdown(renames));
    // Trim trailing newline for a cleaner one-message look.
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Format the `❌` chatops notification text when the rebuild's
/// dependency pre-pass aborted (cycle, cross-day backward dep, scan
/// failure). Pure function for snapshot-testing.
pub(crate) fn format_rebuild_abort_notification(
    repo_url: &str,
    reason: &crate::cli::sync_specs_deps::RebuildAbortReason,
) -> String {
    format!(
        "❌ `{repo_url}`: rebuild aborted — {}. No archives were renamed; no canonical specs were modified. Operator action required.",
        reason.summary()
    )
}
