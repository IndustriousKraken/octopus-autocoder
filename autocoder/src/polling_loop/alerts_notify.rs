use super::*;

/// Defensive no-op: remove `git status --porcelain` lines that
/// reference a workspace-root `.alert-state.json` file. Post-`a16` the
/// file lives in `<state_dir>/alert-state/<basename>.json`, so the
/// workspace should never contain it AND the helper returns its input
/// unchanged for normal operation. The helper stays in the polling-
/// loop code path to absorb transient workspace-root `.alert-state.json`
/// files (e.g., a fresh re-clone of a repo whose history transiently
/// committed it before the migration completes). A future spec can
/// remove the helper after a verification window.
pub(crate) fn filter_alert_state_lines(porcelain: &str) -> String {
    porcelain
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            // Status block is 1–2 chars + space + path; for the strict
            // match we look for the file basename at the start of the path
            // portion. Any line that names `.alert-state.json` as its only
            // path is autocoder bookkeeping.
            let path_start = trimmed.find(char::is_whitespace);
            let path = match path_start {
                Some(i) => trimmed[i..].trim_start(),
                None => trimmed,
            };
            path != ".alert-state.json"
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Post a `🚀 <repo>: starting work on <change> — <first-line-of-Why>`
/// notification when chatops is wired AND `start_work_enabled` is true.
/// Reads `proposal.md` only when the notification will actually be posted
/// so a disabled flag avoids the disk read entirely.
pub(crate) async fn maybe_post_start_of_work(
    workspace: &Path,
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    change: &str,
) {
    let Some(ctx) = chatops_ctx else { return };
    if !ctx.start_work_enabled {
        return;
    }
    let proposal_path = workspace
        .join("openspec/changes")
        .join(change)
        .join("proposal.md");
    let summary = match std::fs::read_to_string(&proposal_path) {
        Ok(raw) => first_line_of_section(&raw, "## Why").unwrap_or_default(),
        Err(e) => {
            tracing::warn!(
                url = %repo.url,
                change = %change,
                "could not read proposal.md for start-of-work summary: {e}; posting without summary"
            );
            String::new()
        }
    };
    let text = if summary.is_empty() {
        format!("🚀 `{}`: starting work on `{change}`", repo.url)
    } else {
        format!("🚀 `{}`: starting work on `{change}` — {summary}", repo.url)
    };
    if let Err(e) = ctx.chatops.post_notification(&ctx.channel, &text).await {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            "start-of-work notification failed; continuing: {e:#}"
        );
    }
}

/// Post the `🔀` rename-list notification. Best-effort: a failed post
/// logs at ERROR and does NOT block PR creation.
pub(crate) async fn maybe_post_rebuild_renames_notification(
    repo: &RepositoryConfig,
    report: &crate::cli::sync_specs::RebuildReport,
    chatops_ctx: Option<&ChatOpsContext>,
) {
    let Some(ctx) = chatops_ctx else { return };
    if report.prefix_renames.is_empty() {
        return;
    }
    let text = format_rebuild_renames_notification(&repo.url, &report.prefix_renames);
    if let Err(e) = ctx.chatops.post_notification(&ctx.channel, &text).await {
        tracing::error!(
            url = %repo.url,
            "rebuild-renames chatops notification failed; continuing: {e:#}"
        );
    }
}

/// Post the `❌` rebuild-aborted notification. Best-effort: a failed
/// post logs at ERROR and does not propagate.
pub(crate) async fn maybe_post_rebuild_abort_notification(
    repo: &RepositoryConfig,
    report: &crate::cli::sync_specs::RebuildReport,
    chatops_ctx: Option<&ChatOpsContext>,
) {
    let Some(ctx) = chatops_ctx else { return };
    let Some(reason) = report.abort_reason.as_ref() else {
        return;
    };
    let text = format_rebuild_abort_notification(&repo.url, reason);
    if let Err(e) = ctx.chatops.post_notification(&ctx.channel, &text).await {
        tracing::error!(
            url = %repo.url,
            "rebuild-abort chatops notification failed; continuing: {e:#}"
        );
    }
}

/// Post the end-of-rebuild chatops notification. Best-effort: a failed
/// post logs at WARN and never propagates. Unlike `maybe_post_pr_opened`,
/// this is NOT gated on `pr_opened_enabled` or `failure_alerts_enabled`
/// because it's a direct response to an operator-triggered command — the
/// operator wants the completion signal regardless of which notification
/// toggles they have set elsewhere.
pub(crate) async fn maybe_post_end_of_rebuild_notification(
    repo: &RepositoryConfig,
    report: &crate::cli::sync_specs::RebuildReport,
    pr_url: Option<&str>,
    chatops_ctx: Option<&ChatOpsContext>,
) {
    let Some(ctx) = chatops_ctx else { return };

    let modified = report.modified_files();
    let text = if report.failed == 0 {
        if let Some(url) = pr_url {
            format!(
                "✓ rebuild complete for `{}`: PR {url} opened — {modified} capability(ies) updated from {} archived change(s)",
                repo.url, report.successful
            )
        } else {
            format!(
                "✓ rebuild complete for `{}`: no drift detected, canonical specs already in sync",
                repo.url
            )
        }
    } else {
        let pr_segment = match pr_url {
            Some(u) => format!("PR {u}"),
            None => "(no PR — every change failed)".to_string(),
        };
        let slugs = report.failed_slugs();
        let listed: Vec<String> = slugs.iter().take(10).cloned().collect();
        let suffix = if slugs.len() > 10 {
            format!(" and {} more", slugs.len() - 10)
        } else {
            String::new()
        };
        let failed_list = format!("{}{suffix}", listed.join(", "));
        format!(
            "⚠️ rebuild for `{}` completed with {} failure(s); {pr_segment} opened with successful {} change(s).\nFailed: {failed_list}.\nSee journalctl -u autocoder for openspec stderr details.",
            repo.url, report.failed, report.successful
        )
    };

    if let Err(e) = ctx.chatops.post_notification(&ctx.channel, &text).await {
        tracing::warn!(
            url = %repo.url,
            "end-of-rebuild chatops notification failed; continuing: {e:#}"
        );
    }
}

/// Post a one-line ChatOps notification announcing a freshly-opened PR.
/// Suppressed when chatops is not configured OR when `pr_opened_enabled` is
/// false. Best-effort: a failed post logs at WARN and never propagates.
pub(crate) async fn maybe_post_pr_opened(
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    pr_url: &str,
    change_count: usize,
) {
    let Some(ctx) = chatops_ctx else { return };
    if !ctx.pr_opened_enabled {
        return;
    }
    let text = format!(
        "🎉 `{}`: opened PR {pr_url} with {change_count} change(s)",
        repo.url
    );
    if let Err(e) = ctx.chatops.post_notification(&ctx.channel, &text).await {
        tracing::warn!(
            url = %repo.url,
            pr = %pr_url,
            "pr-opened notification failed; continuing: {e:#}"
        );
    }
}

/// OSS-fork support (a26): post the `📦 Branch pushed` notification
/// when `auto_submit_pr: false` skipped PR creation. Carries the
/// branch URL AND the templated `gh pr create` command the operator
/// can run manually after local review. Gated by the same
/// `pr_opened_enabled` flag as `maybe_post_pr_opened`.
pub(crate) async fn maybe_post_branch_pushed_no_pr(
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    branch_url: &str,
    suggested_command: &str,
    change_count: usize,
) {
    let Some(ctx) = chatops_ctx else { return };
    if !ctx.pr_opened_enabled {
        return;
    }
    let text = format!(
        "📦 `{url}`: branch pushed with {change_count} change(s): {branch_url}\nRun: {suggested_command}",
        url = repo.url,
    );
    if let Err(e) = ctx.chatops.post_notification(&ctx.channel, &text).await {
        tracing::warn!(
            url = %repo.url,
            branch_url = %branch_url,
            "branch-pushed-no-pr notification failed; continuing: {e:#}"
        );
    }
}

/// OSS-fork support (a26): compose the push-only branch hint URL for the
/// `auto_submit_pr: false` path so the chatops notification links to the
/// pushed branch the operator can review locally.
///
/// a008: the hint is forge-specific — GitHub produces a branch tree URL
/// (`https://github.com/<owner>/<repo>/tree/<branch>`) while GitLab produces
/// an MR-create web URL. The provider is selected via the repo's `forge:`
/// block (see `crate::forge::resolve_forge`), falling back to GitHub's shape
/// when the forge cannot be resolved.
pub(crate) fn compose_branch_url(
    forge: Option<&crate::config::ForgeConfig>,
    url: &str,
    owner: &str,
    repo: &str,
    branch: &str,
) -> String {
    use crate::forge::Forge;
    match crate::forge::resolve_forge(forge, url) {
        Ok(f) => f.branch_url(owner, repo, branch),
        Err(_) => crate::forge::GithubForge::new().branch_url(owner, repo, branch),
    }
}

/// a008: the push-only manual command hint, forge-specific: `gh pr create`
/// for GitHub (the default) versus `glab mr create` for GitLab.
pub(crate) fn push_only_command(
    forge: Option<&crate::config::ForgeConfig>,
    base: &str,
    branch: &str,
) -> String {
    use crate::config::ForgeKind;
    match forge.map(|f| f.kind) {
        Some(ForgeKind::Gitlab) => {
            format!("glab mr create --target-branch {base} --source-branch {branch}")
        }
        _ => format!("gh pr create --base {base} --head {branch}"),
    }
}

/// Post a one-line ChatOps notification announcing a fork recreation.
/// Re-forking is destructive: any open PRs from the deleted fork are
/// closed by GitHub when the head ref disappears, so operators should
/// see this immediately. Gated by `failure_alerts_enabled` (re-fork is
/// a recovery action; if the operator opted out of failure alerts, they
/// have opted out of this too).
pub(crate) async fn maybe_post_refork_notification(
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
) {
    let Some(ctx) = chatops_ctx else { return };
    if !ctx.failure_alerts_enabled {
        return;
    }
    let text = format!(
        ":warning: `{}`: re-forked at workspace reinitialization \
         (previous fork deleted; any open PRs from this fork are now closed)",
        repo.url
    );
    if let Err(e) = ctx.chatops.post_notification(&ctx.channel, &text).await {
        tracing::warn!(
            url = %repo.url,
            "re-fork notification failed; continuing: {e:#}"
        );
    }
}
