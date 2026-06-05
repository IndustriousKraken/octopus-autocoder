use super::*;

/// Open the PR for a rebuild iteration. Returns the new PR's HTML URL on
/// success.
pub(crate) async fn open_rebuild_pull_request(
    _paths: &DaemonPaths,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    report: &crate::cli::sync_specs::RebuildReport,
) -> Result<String> {
    let (owner, repo_name) = github::parse_repo_url(&repo.url)?;
    let token = crate::github_credentials::resolve_token(github_cfg, &owner)?;
    let modified = report.modified_files();
    let title = format!("spec rebuild: {modified} capability(ies) rebuilt from archive history");
    let body = build_rebuild_pr_body(report);
    let head = match github_cfg.fork_owner.as_deref() {
        Some(fork_owner) => format!("{fork_owner}:{}", repo.agent_branch),
        None => repo.agent_branch.clone(),
    };
    let pr = github::create_pull_request(
        &owner,
        &repo_name,
        &head,
        &repo.base_branch,
        &title,
        &body,
        &token,
        None,
        false,
    )
    .await?;
    tracing::info!(
        url = repo.url.as_str(),
        pr = pr.html_url.as_str(),
        pr_number = pr.number,
        "opened rebuild PR"
    );
    Ok(pr.html_url)
}

/// PR-creation routing wrapper. In production this is a thin shim around
/// `github::create_pull_request` (targets the live GitHub API). Under
/// `cfg(test)`, when an override is installed via `test_hooks`, the call
/// is rerouted to `github::create_pull_request_at_for_test` against a
/// mockito server URL so the test can assert head/base/title/body.
#[allow(clippy::too_many_arguments)]
async fn create_pull_request_via_hook(
    owner: &str,
    repo: &str,
    head: &str,
    base: &str,
    title: &str,
    body: &str,
    token: &str,
    review_report: Option<&ReviewReport>,
    draft: bool,
) -> Result<github::CreatedPr> {
    #[cfg(test)]
    {
        if let Some(api_base) = test_hooks::github_api_base() {
            return github::create_pull_request_at_for_test(
                &api_base,
                owner,
                repo,
                head,
                base,
                title,
                body,
                token,
                review_report,
                draft,
            )
            .await;
        }
    }
    github::create_pull_request(
        owner,
        repo,
        head,
        base,
        title,
        body,
        token,
        review_report,
        draft,
    )
    .await
}

/// Build the initial per-PR `RevisionState` written at PR-open time when the
/// original automatic review ran (a33 §7.2 baseline + the per-PR caps).
///
/// The caps are SOURCED — never hardcoded — so this init agrees with the
/// revision dispatcher's own state init in `revisions::process_one_pr`:
/// - `revision_cap` is the resolved `executor.max_auto_revisions_per_pr`
///   (already clamped at config load) — bounds AUTOMATIC revisions only.
/// - `code_review_cap` is `reviewer.max_code_reviews_per_pr()`, where `None`
///   means UNLIMITED (the a47 default). Hardcoding `Some(5)` here would
///   silently re-cap re-reviews on every daemon-opened PR even when the
///   operator set no cap, defeating a47's default-unlimited re-reviews.
pub(crate) fn initial_revision_state_at_pr_open(
    pr_number: u64,
    agent_branch: String,
    now: chrono::DateTime<chrono::Utc>,
    revision_cap: u32,
    reviewer: Option<&CodeReviewer>,
    head_sha: String,
) -> crate::revisions::RevisionState {
    crate::revisions::RevisionState {
        pr_number,
        agent_branch,
        last_seen_comment_at: now,
        auto_revisions_applied: 0,
        revision_cap,
        cap_decline_posted: false,
        human_revise_count: 0,
        human_revise_cap_decline_posted: false,
        code_reviews_applied: 0,
        code_review_cap: reviewer.and_then(|r| r.max_code_reviews_per_pr()),
        cap_decline_posted_for_code_review: false,
        last_suggested_rereview_at_revisions_count: None,
        original_review_head_sha: Some(head_sha),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn open_pull_request(
    paths: &DaemonPaths,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    changes: &[String],
    includes_self_heal: bool,
    review_report: Option<&ReviewReport>,
    reviewer: Option<&CodeReviewer>,
    revision_cap: u32,
    draft: bool,
    reviewer_revision_concerns: &[ReviewConcern],
    chatops_ctx: Option<&ChatOpsContext>,
    workspace: &Path,
    spec_verification_section: Option<&str>,
) -> Result<()> {
    let (owner, repo_name) = github::parse_repo_url(&repo.url)?;
    // PAT routing uses the UPSTREAM owner, not the fork owner — the PR is
    // posted to upstream's /pulls endpoint regardless of fork-PR mode, so
    // the credential authorizing that call must have access to upstream.
    let token = crate::github_credentials::resolve_token(github_cfg, &owner)?;
    // Audit-only iterations have no implementer-processed changes; the
    // agent branch carries only the audit's `audit: <type> proposals
    // (N change(s))` commits. Build the PR title + body from those
    // commit subjects so reviewers see which audits fired.
    let (title, body) = build_open_pr_title_body(
        repo,
        changes,
        includes_self_heal,
        workspace,
        spec_verification_section,
    );

    // In fork-PR mode the `head` is namespaced `<fork-owner>:<branch>` for
    // GitHub to recognize the cross-repo PR. Direct-push mode uses the bare
    // branch name (same-repo PR).
    let head = match github_cfg.fork_owner.as_deref() {
        Some(fork_owner) => format!("{fork_owner}:{}", repo.agent_branch),
        None => repo.agent_branch.clone(),
    };

    // OSS-fork support (a26): when `auto_submit_pr: false`, skip the
    // PR-creation API call. The branch has already been pushed to its
    // remote by the caller; we surface the branch URL AND a
    // templated `gh pr create` command to chatops so the operator can
    // open the PR manually after local review.
    if !repo.auto_submit_pr {
        let branch_url = compose_branch_url(&owner, &repo_name, &repo.agent_branch);
        let pr_base = repo
            .upstream
            .as_ref()
            .map(|u| u.branch.as_str())
            .unwrap_or(&repo.base_branch);
        let suggested = format!("gh pr create --base {pr_base} --head {}", repo.agent_branch);
        maybe_post_branch_pushed_no_pr(repo, chatops_ctx, &branch_url, &suggested, changes.len())
            .await;
        tracing::info!(
            url = %repo.url,
            branch_url = %branch_url,
            "auto_submit_pr: false — skipped PR creation; surfaced branch URL to chatops"
        );
        // Best-effort: post implementer-summary comments only when a PR
        // exists. Without a PR we have no number to attach them to —
        // skip and rely on chatops surfacing.
        return Ok(());
    }

    let pr = match create_pull_request_via_hook(
        &owner,
        &repo_name,
        &head,
        &repo.base_branch,
        &title,
        &body,
        &token,
        review_report,
        draft,
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            handle_predictable_failure(
                paths,
                workspace,
                &repo.url,
                chatops_ctx,
                chatops_ctx
                    .map(|c| c.failure_alerts_enabled)
                    .unwrap_or(false),
                AlertCategory::PrCreationFailure,
                &e,
            )
            .await;
            return Err(e);
        }
    };
    tracing::info!(
        url = repo.url.as_str(),
        pr = pr.html_url.as_str(),
        pr_number = pr.number,
        "opened PR"
    );

    record_original_review_head_sha(
        paths,
        workspace,
        repo,
        pr.number,
        revision_cap,
        reviewer,
        review_report,
    );

    // Best-effort: post a one-line ChatOps notification with a link to
    // the new PR. PR creation already succeeded; never propagate a
    // failure from this step.
    maybe_post_pr_opened(repo, chatops_ctx, &pr.html_url, changes.len()).await;

    // Best-effort: post a follow-up comment with each change's implementer
    // stdout. PR creation already succeeded; never propagate a failure
    // from this step.
    post_implementer_summary_comment(
        paths,
        github::DEFAULT_API_BASE,
        workspace,
        &owner,
        &repo_name,
        pr.number,
        changes,
        &token,
    )
    .await;

    // Best-effort: post one `<!-- reviewer-revision -->` comment per
    // taken reviewer concern, so the revision dispatcher (running on the
    // next polling iteration) picks them up and forwards them to the
    // implementer agent. PR creation already succeeded; per-concern post
    // failures are logged at WARN but never propagated.
    if !reviewer_revision_concerns.is_empty() {
        post_reviewer_revision_comments(
            github::DEFAULT_API_BASE,
            &owner,
            &repo_name,
            pr.number,
            reviewer_revision_concerns,
            &token,
        )
        .await;
    }

    Ok(())
}

/// Build the `(title, body)` for an opened PR: audit-only vs. implementer
/// shape, plus the advisory `## Spec Verification` splice. Extracted from
/// `open_pull_request` (a68 function-size split).
fn build_open_pr_title_body(
    repo: &RepositoryConfig,
    changes: &[String],
    includes_self_heal: bool,
    workspace: &Path,
    spec_verification_section: Option<&str>,
) -> (String, String) {
    let (title, mut body) = if changes.is_empty() {
        let range = format!("{}..{}", repo.base_branch, repo.agent_branch);
        let subjects = git::log_subjects(workspace, &range).unwrap_or_default();
        (
            build_audit_only_pr_title(&subjects),
            build_audit_only_pr_body(&subjects),
        )
    } else {
        (
            build_pr_title(changes),
            build_pr_body(workspace, changes, includes_self_heal),
        )
    };
    // a63: splice the advisory `## Spec Verification` section (the `[out]`
    // gate's verdict) into the PR body, parallel to the reviewer's
    // `## Code Review` block (which is appended downstream from
    // `review_report`). Absent when the gate is disabled, produced no verdict
    // (advisory failure), OR the iteration is audit-only.
    if let Some(section) = spec_verification_section
        && !section.trim().is_empty()
    {
        body.push_str("\n\n");
        body.push_str(section.trim_end());
    }
    (title, body)
}

/// Persist the agent-branch head SHA captured at PR-open time so the
/// diff-overlap revision path has a baseline. Best-effort. Extracted from
/// `open_pull_request` (a68 function-size split).
fn record_original_review_head_sha(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    pr_number: u64,
    revision_cap: u32,
    reviewer: Option<&CodeReviewer>,
    review_report: Option<&ReviewReport>,
) {
    // a33 task 7.2: record the agent-branch head SHA at the time the
    // original automatic review completed, so the diff-overlap suggestion
    // path has a baseline. Best-effort — failures here do NOT abort PR
    // opening. Only fires when a review_report is present (i.e. a
    // reviewer ran on this iteration).
    if review_report.is_some()
        && let Ok(head_sha) = git::rev_parse(workspace, &repo.agent_branch)
    {
        {
            let now = chrono::Utc::now();
            let existing = crate::revisions::read_state(paths, workspace, pr_number)
                .ok()
                .flatten();
            let state = match existing {
                Some(mut s) => {
                    s.original_review_head_sha = Some(head_sha);
                    s
                }
                None => initial_revision_state_at_pr_open(
                    pr_number,
                    repo.agent_branch.clone(),
                    now,
                    revision_cap,
                    reviewer,
                    head_sha,
                ),
            };
            if let Err(e) = crate::revisions::write_state(paths, workspace, &state) {
                tracing::warn!(
                    url = %repo.url,
                    pr_number = pr_number,
                    "failed to persist original_review_head_sha: {e:#}"
                );
            }
        }
    }
}

/// Return `true` if any open PR exists on GitHub for the configured agent
/// branch, in which case the caller should skip this iteration. On any
/// failure to perform the check (parse, token, transport, non-2xx) this
/// logs a WARN and returns `false` so a transient GitHub problem does not
/// block normal iterations — the cost of a redundant Claude run is lower
/// than the cost of an entire repo grinding to a halt on a flaky API.
///
/// `api_base` is `github::DEFAULT_API_BASE` in production; tests pass a
/// mockito server URL instead.
pub(crate) async fn open_pr_exists_for_agent_branch_at(
    _paths: &DaemonPaths,
    api_base: &str,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
) -> bool {
    let (upstream_owner, upstream_repo) = match github::parse_repo_url(&repo.url) {
        Ok(pair) => pair,
        Err(e) => {
            tracing::warn!(
                url = %repo.url,
                "open-PR check skipped: cannot parse repo URL: {e:#}"
            );
            return false;
        }
    };
    // In fork-PR mode, the head qualifier is `<fork_owner>:<branch>`; in
    // direct mode it's the upstream owner. Either way the QUERY targets
    // the upstream repo's `/pulls` because that's where PRs are created.
    let head_owner = github_cfg.fork_owner.as_deref().unwrap_or(&upstream_owner);
    let head = format!("{}:{}", head_owner, repo.agent_branch);

    let token = match crate::github_credentials::resolve_token(github_cfg, &upstream_owner) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(
                url = %repo.url,
                "open-PR check skipped: token resolution failed: {e:#}"
            );
            return false;
        }
    };

    let result = if api_base == github::DEFAULT_API_BASE {
        github::list_open_prs(
            &upstream_owner,
            &upstream_repo,
            &head,
            &repo.base_branch,
            &token,
        )
        .await
    } else {
        // Test path: explicit base.
        #[cfg(test)]
        {
            github::list_open_prs_at_for_test(
                api_base,
                &upstream_owner,
                &upstream_repo,
                &head,
                &repo.base_branch,
                &token,
            )
            .await
        }
        #[cfg(not(test))]
        {
            unreachable!("non-default api_base is test-only");
        }
    };

    match result {
        Ok(prs) if !prs.is_empty() => {
            let numbers: Vec<u64> = prs.iter().map(|p| p.number).collect();
            tracing::info!(
                url = %repo.url,
                pr_count = numbers.len(),
                prs = ?numbers,
                "open PR exists for agent branch; skipping iteration"
            );
            true
        }
        Ok(_) => false,
        Err(e) => {
            tracing::warn!(
                url = %repo.url,
                "open-PR check failed: {e:#}; proceeding with iteration"
            );
            false
        }
    }
}

pub(crate) async fn open_pr_exists_for_agent_branch(
    paths: &DaemonPaths,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
) -> bool {
    #[cfg(test)]
    {
        if let Some(api_base) = test_hooks::github_api_base() {
            return open_pr_exists_for_agent_branch_at(paths, &api_base, repo, github_cfg).await;
        }
    }
    open_pr_exists_for_agent_branch_at(paths, github::DEFAULT_API_BASE, repo, github_cfg).await
}

/// Open the audit-triage / chat-triage spec PR. Mirrors the shape of
/// `polling_loop::open_pull_request` but is purpose-built for the
/// spec-only triage flow (no reviewer step, no change-list body). Routes
/// through `create_pull_request_via_hook` so tests can assert against a
/// mockito server.
pub(crate) async fn open_triage_pull_request(
    _paths: &DaemonPaths,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    head_branch: &str,
    base_branch: &str,
    title: &str,
    body: &str,
) -> Result<String> {
    let (owner, name) = github::parse_repo_url(&repo.url)
        .with_context(|| "audit-triage: parsing repo URL".to_string())?;
    let token = crate::github_credentials::resolve_token(github_cfg, &owner)?;
    let head = if let Some(fork_owner) = github_cfg.fork_owner.as_deref() {
        format!("{fork_owner}:{head_branch}")
    } else {
        head_branch.to_string()
    };
    let pr = create_pull_request_via_hook(
        &owner,
        &name,
        &head,
        base_branch,
        title,
        body,
        &token,
        None,
        false,
    )
    .await?;
    Ok(pr.html_url)
}
