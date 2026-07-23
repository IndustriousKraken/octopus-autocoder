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
    let pr = create_pull_request_via_hook(
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
    use crate::forge::Forge;
    // a007: PR creation routes through the `Forge` trait. The provider is
    // GitHub; `with_api_base` threads the (test-injected) API base so the
    // mockito-driven tests exercise the trait path unchanged.
    #[cfg(test)]
    let forge = match test_hooks::github_api_base() {
        Some(api_base) => crate::forge::GithubForge::with_api_base(api_base),
        None => crate::forge::GithubForge::new(),
    };
    #[cfg(not(test))]
    let forge = crate::forge::GithubForge::new();
    forge
        .open_pr(
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
    gate_verdicts_section: Option<&str>,
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
        gate_verdicts_section,
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
        let branch_url = compose_branch_url(
            repo.forge.as_ref(),
            &repo.url,
            &owner,
            &repo_name,
            &repo.agent_branch,
        );
        let pr_base = repo
            .upstream
            .as_ref()
            .map(|u| u.branch.as_str())
            .unwrap_or(&repo.base_branch);
        let suggested = push_only_command(repo.forge.as_ref(), pr_base, &repo.agent_branch);
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
    gate_verdicts_section: Option<&str>,
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
    // verifier-gates-fail-closed §6: splice the `## Gate verdicts` ledger
    // section into the PR body as a compliance record — per gate (AND the
    // reviewer): identifier, model, verdict. A `PASS` is VISIBLE here rather
    // than inferred from the silent absence of an alert. Absent for audit-only
    // iterations (no implementer changes were gated).
    if let Some(section) = gate_verdicts_section
        && !section.trim().is_empty()
    {
        body.push_str("\n\n");
        body.push_str(section.trim_end());
    }
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

/// After this many consecutive query-failure (`Unknown`) skips for a
/// repository, the daemon raises the throttled operator alert. Three
/// consecutive failures (~30+ minutes at default cadence) separates a
/// transient blip from a sustained outage without new configuration.
pub(crate) const OPEN_PR_GATE_FAILURE_ALERT_THRESHOLD: u32 = 3;

/// Three-way outcome of the open-PR gate query. The gate fails CLOSED:
/// `Open` and `Unknown` both skip the iteration; only a confirmed empty list
/// (`NoPr`) proceeds. Collapsing `Unknown` into either boolean is the fail-open
/// bug this change removes (open-pr-gate-fails-closed).
#[derive(Debug, Clone)]
pub(crate) enum OpenPrGateOutcome {
    /// One or more open PRs exist for the agent branch → skip the iteration.
    Open,
    /// The query confirmed no open PR exists → proceed with the iteration.
    NoPr,
    /// The query could not deliver an answer (unparseable repo URL,
    /// token-resolution failure, transport error, or non-2xx). Skip the
    /// iteration exactly as if an open PR existed, because "cannot confirm no
    /// open PR" risks precisely the harms the gate exists to prevent. Carries
    /// a short cause description for the sustained-failure operator alert; the
    /// per-arm WARN is logged where the failure is detected.
    Unknown(String),
}

/// Query GitHub for an open PR on the configured agent branch and classify the
/// result three-way. On any failure to perform the check (parse, token,
/// transport, non-2xx) this logs a WARN and returns [`OpenPrGateOutcome::Unknown`]
/// so the caller fails CLOSED — a redundant Claude run, a force-push over a
/// reviewer's in-flight PR, and the 422 "PR already exists" loop are all worse
/// than parking one polling pass on an unconfirmed answer.
///
/// `api_base` is `github::DEFAULT_API_BASE` in production; tests pass a
/// mockito server URL instead.
pub(crate) async fn open_pr_exists_for_agent_branch_at(
    _paths: &DaemonPaths,
    api_base: &str,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
) -> OpenPrGateOutcome {
    let (upstream_owner, upstream_repo) = match github::parse_repo_url(&repo.url) {
        Ok(pair) => pair,
        Err(e) => {
            tracing::warn!(
                url = %repo.url,
                "open-PR check could not run: cannot parse repo URL: {e:#}; skipping iteration (fail closed)"
            );
            return OpenPrGateOutcome::Unknown(format!("cannot parse repo URL: {e:#}"));
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
                "open-PR check could not run: token resolution failed: {e:#}; skipping iteration (fail closed)"
            );
            return OpenPrGateOutcome::Unknown(format!("token resolution failed: {e:#}"));
        }
    };

    // a007: the open-PR check routes through the `Forge` trait. `api_base` is
    // `DEFAULT_API_BASE` in production and a mockito URL in tests; the GitHub
    // provider threads it via `with_api_base`.
    use crate::forge::Forge;
    let result = crate::forge::GithubForge::with_api_base(api_base)
        .list_open_prs(
            &upstream_owner,
            &upstream_repo,
            &head,
            &repo.base_branch,
            &token,
        )
        .await;

    match result {
        Ok(prs) if !prs.is_empty() => {
            let numbers: Vec<u64> = prs.iter().map(|p| p.number).collect();
            tracing::info!(
                url = %repo.url,
                pr_count = numbers.len(),
                prs = ?numbers,
                "open PR exists for agent branch; skipping iteration"
            );
            OpenPrGateOutcome::Open
        }
        Ok(_) => OpenPrGateOutcome::NoPr,
        Err(e) => {
            tracing::warn!(
                url = %repo.url,
                "open-PR check failed: {e:#}; skipping iteration (fail closed)"
            );
            OpenPrGateOutcome::Unknown(format!("open-PR query failed: {e:#}"))
        }
    }
}

pub(crate) async fn open_pr_exists_for_agent_branch(
    paths: &DaemonPaths,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
) -> OpenPrGateOutcome {
    #[cfg(test)]
    {
        if let Some(api_base) = test_hooks::github_api_base() {
            return open_pr_exists_for_agent_branch_at(paths, &api_base, repo, github_cfg).await;
        }
    }
    open_pr_exists_for_agent_branch_at(paths, github::DEFAULT_API_BASE, repo, github_cfg).await
}

/// Run the open-PR gate for one pass AND drive the per-repo consecutive-failure
/// counter (open-pr-gate-fails-closed §2). Returns `true` only on a confirmed
/// empty list (proceed); `false` on `Open` OR `Unknown` (skip, fail closed).
///
/// The `consecutive_failures` counter is the polling task's in-memory per-repo
/// state (a restart resetting it merely delays the alert): any successful query
/// (`Open` or `NoPr`) resets it to `0`; each `Unknown` increments it, and the
/// third consecutive `Unknown` posts the throttled operator alert naming the
/// gate, the repository, AND the most recent error via the existing
/// `handle_predictable_failure` throttle machinery. Subsequent consecutive
/// failures do not re-alert until the 24h throttle window elapses (or a
/// successful full pass clears the alert state).
pub(crate) async fn open_pr_gate_decision(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    consecutive_failures: &mut u32,
) -> bool {
    match open_pr_exists_for_agent_branch(paths, repo, github_cfg).await {
        // A successful query (either answer) resets the failure streak.
        OpenPrGateOutcome::Open => {
            *consecutive_failures = 0;
            false
        }
        OpenPrGateOutcome::NoPr => {
            *consecutive_failures = 0;
            true
        }
        OpenPrGateOutcome::Unknown(cause) => {
            *consecutive_failures = consecutive_failures.saturating_add(1);
            if *consecutive_failures >= OPEN_PR_GATE_FAILURE_ALERT_THRESHOLD {
                // Sustained failure: raise the throttled operator alert so a
                // repo silently idling behind a broken query is visible. The
                // 24h per-(repo, category) throttle inside
                // `handle_predictable_failure` suppresses re-alerts within the
                // window; a successful full pass clears the alert state.
                handle_predictable_failure(
                    paths,
                    workspace,
                    &repo.url,
                    chatops_ctx,
                    chatops_ctx
                        .map(|c| c.failure_alerts_enabled)
                        .unwrap_or(false),
                    AlertCategory::OpenPrGateFailure,
                    &anyhow!("{cause}"),
                )
                .await;
            }
            false
        }
    }
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

/// Open the confirmed-rollback PR, REUSING an existing agent-branch PR when one
/// is present instead of raw-creating (which 422s "a pull request already
/// exists"). The rollback's force-push of the agent branch already updated any
/// existing PR's head to the rolled-back state; this detects that PR — via the
/// SAME `list_open_prs` head/base query the polling loop's open-PR check uses —
/// and updates its title AND body to describe the rollback. A new PR is created
/// ONLY when none exists.
///
/// Returns the PR's HTML URL on either path.
pub(crate) async fn open_or_update_rollback_pull_request(
    _paths: &DaemonPaths,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    head_branch: &str,
    base_branch: &str,
    title: &str,
    body: &str,
) -> Result<String> {
    use crate::forge::Forge;
    let (owner, name) = github::parse_repo_url(&repo.url)
        .with_context(|| "rollback PR: parsing repo URL".to_string())?;
    let token = crate::github_credentials::resolve_token(github_cfg, &owner)?;
    let head_owner = github_cfg.fork_owner.as_deref().unwrap_or(&owner);
    let head = format!("{head_owner}:{head_branch}");

    let forge = rollback_forge();

    // Detect an existing open PR for the agent branch (same head/base query as
    // the polling loop's open-PR check). On a transport/parse error this is a
    // best-effort detection: fall through to create (the create either succeeds
    // or surfaces the real 422 to the operator).
    let existing = forge
        .list_open_prs(&owner, &name, &head, base_branch, &token)
        .await
        .ok()
        .and_then(|prs| prs.into_iter().next());

    if let Some(pr) = existing {
        tracing::info!(
            url = %repo.url,
            pr_number = pr.number,
            "rollback: reusing existing agent-branch PR (force-push updated its head); retitling to the rollback"
        );
        return forge
            .update_pr(&owner, &name, pr.number, title, body, &token)
            .await
            .with_context(|| format!("updating existing rollback PR #{}", pr.number));
    }

    let created = forge
        .open_pr(&owner, &name, &head, base_branch, title, body, &token, None, false)
        .await?;
    Ok(created.html_url)
}

/// Build the `Forge` provider for the rollback PR step, threading the
/// (test-injected) API base under `cfg(test)` so mockito-driven tests exercise
/// the same path as production.
fn rollback_forge() -> crate::forge::GithubForge {
    #[cfg(test)]
    {
        if let Some(api_base) = test_hooks::github_api_base() {
            return crate::forge::GithubForge::with_api_base(api_base);
        }
    }
    crate::forge::GithubForge::new()
}
