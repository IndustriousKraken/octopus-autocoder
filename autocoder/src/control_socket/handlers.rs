//! Control-socket action handlers behind the [`super::DISPATCH`] table:
//! repo-status assembly, marker clears, workspace wipe, spec rebuild, audit
//! triage/queueing, on-demand review, recent-commits / survival / provenance
//! reads, rollback recovery, defer/undefer, canonical-spec query, the
//! execution-scoped outcome/submission relays, AND config reload — together
//! with the private helpers those handlers share (`find_repo` and friends live
//! in the parent module). Split out of `control_socket.rs` as a behavior-
//! preserving decomposition; the per-action request/response JSON is unchanged.
//!
//! Handlers reached from the dispatch table (and the few helpers/types the
//! sibling test module exercises directly) are `pub(crate)` AND re-exported at
//! `super`'s path via `pub(crate) use handlers::*`, keeping every call site —
//! the table, the tests, and `dispatch_request` — compiling at the original
//! `crate::control_socket::*` paths.
use super::*;

pub(crate) async fn handle_repo_status(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(u) => u,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let workspace_path = workspace::resolve_path(&state.paths, &repo);
    let github_cfg = state.github.load_full();
    let stale_threshold = state
        .last_config
        .load_full()
        .executor
        .busy_marker_stale_threshold_secs();
    match build_repo_status(&state.paths, &workspace_path, &repo, &github_cfg, stale_threshold).await {
        Ok(resp) => match serde_json::to_value(&resp) {
            Ok(body) => json!({"ok": true, "status": body}),
            Err(e) => json!({"ok": false, "error": format!("serializing status: {e}")}),
        },
        Err(e) => json!({"ok": false, "error": format!("{e:#}")}),
    }
}

/// Aggregate `repo_status` for every repository currently in the live
/// `repo_tasks` registry — one round trip instead of N. Per-repo failures
/// are caught and recorded in the per-entry `ok` field rather than
/// failing the whole call; the bare-status menu always ships every repo
/// section even if one repo's workspace is mid-failure.
pub(crate) async fn handle_repo_status_all(state: &ControlState) -> Value {
    let repos: Vec<RepositoryConfig> = {
        // Snapshot URLs from the live task registry, then look up each
        // URL in the current config holder so the per-repo
        // RepositoryConfig is the one polling tasks see.
        let urls: Vec<String> = {
            let guard = state.repo_tasks.lock().unwrap();
            guard.keys().cloned().collect()
        };
        let cfg = state.last_config.load_full();
        urls.into_iter()
            .filter_map(|url| {
                cfg.repositories.iter().find(|r| r.url == url).cloned()
            })
            .collect()
    };
    let github_cfg = state.github.load_full();
    let stale_threshold = state
        .last_config
        .load_full()
        .executor
        .busy_marker_stale_threshold_secs();
    let mut results = Vec::with_capacity(repos.len());
    for repo in repos {
        let workspace_path = workspace::resolve_path(&state.paths, &repo);
        let url = repo.url.clone();
        let entry = match build_repo_status(&state.paths, &workspace_path, &repo, &github_cfg, stale_threshold).await {
            Ok(resp) => match serde_json::to_value(&resp) {
                Ok(body) => json!({"url": url, "ok": true, "status": body}),
                Err(e) => json!({
                    "url": url,
                    "ok": false,
                    "error": format!("serializing status: {e}"),
                }),
            },
            Err(e) => json!({
                "url": url,
                "ok": false,
                "error": format!("{e:#}"),
            }),
        };
        results.push(entry);
    }
    json!({"ok": true, "results": results})
}

/// Build the `RepoStatusResponse` for one repo by reading the workspace's
/// failure-state, alert-state, marker files, and queue state. Pure
/// filesystem reads + config snapshot, plus one outbound GitHub API call
/// for the "latest PR by the daemon" line. Does not interrogate the live
/// polling-task map for `last_iteration` (no central record exists yet);
/// the field is populated from the most recent failure-state timestamp
/// when available.
///
/// Per the status-enrichment spec, a GitHub or local-git failure is
/// log-and-degrade: the affected field becomes `None` and the reply
/// still ships every other section. An operator hitting `status <repo>`
/// during a GitHub incident still gets the local-state half.
async fn build_repo_status(
    paths: &crate::paths::DaemonPaths,
    workspace_path: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    stale_threshold_secs: u64,
) -> Result<RepoStatusResponse> {
    let mut resp = RepoStatusResponse {
        url: repo.url.clone(),
        base_branch: repo.base_branch.clone(),
        agent_branch: repo.agent_branch.clone(),
        ..RepoStatusResponse::default()
    };

    // Currently-busy peek is workspace-relative but does not require the
    // workspace dir to exist (the marker lives under the runtime dir),
    // so populate it before the early-return. The full marker contents
    // (stage, pid, audit-type-on-match) feed the new `currently:` line
    // branches in `format_status_reply`.
    resp.currently_busy = busy_marker::current(paths, workspace_path, stale_threshold_secs);

    // Workspace may not exist yet (e.g. a freshly added repo whose initial
    // clone hasn't run). Treat that as "everything empty for the
    // workspace-derived fields" — the URL header + branches + busy-marker
    // peek are still useful, and operators won't see a false error.
    if !workspace_path.is_dir() {
        // Try the GitHub PR call anyway — it does not depend on the local
        // workspace.
        resp.latest_pr = fetch_latest_pr(repo, github_cfg).await;
        return Ok(resp);
    }

    // Last-commit lines: best-effort. On error, log and keep the field
    // as None so the formatter renders `(none)`.
    match git::last_commit_summary(workspace_path, &repo.base_branch) {
        Ok(s) => resp.last_commit_base = s,
        Err(e) => {
            tracing::warn!(
                url = %repo.url,
                branch = %repo.base_branch,
                "status: last_commit_summary failed: {e:#}"
            );
        }
    }
    match git::last_commit_summary(workspace_path, &repo.agent_branch) {
        Ok(s) => resp.last_commit_agent = s,
        Err(e) => {
            tracing::warn!(
                url = %repo.url,
                branch = %repo.agent_branch,
                "status: last_commit_summary failed: {e:#}"
            );
        }
    }

    // Latest PR by the daemon (one outbound GitHub call). Failure is
    // log-and-degrade.
    resp.latest_pr = fetch_latest_pr(repo, github_cfg).await;

    // Marker-excluded changes — pull marked_at + detail from the marker
    // JSON files where possible. Each marker entry also carries a
    // `has_ignore_for_queue` flag (a18) so the formatter can annotate
    // the line when the operator has stamped `.ignore-for-queue.json`
    // alongside the blocking marker.
    let (perma_changes, revision_changes) = queue::list_marker_excluded(workspace_path)?;
    for change in perma_changes {
        let marker_path = workspace_path
            .join("openspec/changes")
            .join(&change)
            .join(".perma-stuck.json");
        let (marked_at, detail) = read_perma_marker(&marker_path);
        let has_ignore_for_queue = queue::is_ignore_for_queue_marked(workspace_path, &change);
        resp.perma_stuck_changes.push(MarkerEntry {
            change,
            marked_at,
            detail,
            has_ignore_for_queue,
        });
    }
    for change in revision_changes {
        let marker_path = workspace_path
            .join("openspec/changes")
            .join(&change)
            .join(".needs-spec-revision.json");
        let marked_at = read_revision_marker(&marker_path);
        let has_ignore_for_queue = queue::is_ignore_for_queue_marked(workspace_path, &change);
        resp.revision_marked_changes.push(MarkerEntry {
            change,
            marked_at,
            detail: String::new(),
            has_ignore_for_queue,
        });
    }

    // Throttled alerts (category-level + per-change perma-stuck +
    // per-change spec-revision).
    let alert_state = AlertState::load_or_default(paths, workspace_path);
    for (category, entry) in &alert_state.alerts {
        resp.throttled_alerts.push(ThrottledAlertEntry {
            label: category.label().to_string(),
            last_fired_at: entry.last_alerted_at,
            throttle_window_hours: 24,
        });
    }
    for (change, entry) in &alert_state.perma_stuck_alerts {
        resp.throttled_alerts.push(ThrottledAlertEntry {
            label: format!("perma_stuck:{change}"),
            last_fired_at: entry.last_alerted_at,
            throttle_window_hours: 24,
        });
    }
    for (change, entry) in &alert_state.spec_revision_alerts {
        resp.throttled_alerts.push(ThrottledAlertEntry {
            label: format!("spec_revision:{change}"),
            last_fired_at: entry.last_alerted_at,
            throttle_window_hours: 24,
        });
    }

    // Queue snapshot.
    resp.pending_changes = queue::list_pending(paths, workspace_path).unwrap_or_default();
    resp.waiting_changes = queue::list_waiting(workspace_path).unwrap_or_default();

    // Best-effort last-iteration: failure-state's most recent entry
    // gives us a timestamp for "something happened recently"; without a
    // central iteration log there's no archive-vs-failure outcome to
    // report. Skip when there are no failure-state entries (a healthy
    // workspace).
    if let Ok(state) = failure_state::load(paths, workspace_path) {
        if let Some(latest_entry) = state
            .entries
            .values()
            .max_by_key(|e| e.last_failed_at)
        {
            resp.last_iteration = Some(LastIteration {
                finished_at: latest_entry.last_failed_at,
                outcome_summary: format!(
                    "last failure: {}",
                    truncate(&latest_entry.last_reason, 80)
                ),
                next_iteration_estimate: Some(
                    latest_entry.last_failed_at
                        + chrono::Duration::seconds(repo.poll_interval_sec as i64),
                ),
                poll_interval_sec: repo.poll_interval_sec,
            });
        }
    }

    Ok(resp)
}

/// Resolve owner / repo / token for `repo` and call
/// `github::latest_pr_for_head`. Any failure (parse, token-resolve, HTTP)
/// is logged at WARN and converted to `None`. Per spec: the status reply
/// MUST NOT fail because GitHub is rate-limited or briefly down.
async fn fetch_latest_pr(
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
) -> Option<crate::chatops::operator_commands::PrSummary> {
    fetch_latest_pr_at(github::DEFAULT_API_BASE, repo, github_cfg).await
}

/// Test-instrumentable variant of `fetch_latest_pr`. Production calls
/// the no-arg helper above which forwards to `DEFAULT_API_BASE`; tests
/// pass a mockito URL. Same head_owner-from-fork_owner semantics per
/// `a20a4`.
pub(crate) async fn fetch_latest_pr_at(
    api_base: &str,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
) -> Option<crate::chatops::operator_commands::PrSummary> {
    let (owner, repo_name) = match github::parse_repo_url(&repo.url) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(url = %repo.url, "status: parse_repo_url failed: {e:#}");
            return None;
        }
    };
    let token = match github_credentials::resolve_token(github_cfg, &owner) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(url = %repo.url, "status: github token resolve failed: {e:#}");
            return None;
        }
    };
    // Per a20a4: head qualifier owner is fork_owner in fork-PR mode,
    // upstream owner otherwise. Pre-fix code passed only the upstream
    // owner; the GitHub query never matched a fork-headed PR, so every
    // fork-PR-mode status reply showed `latest PR: (none)` regardless
    // of whether one was open.
    let head_owner = github_cfg.fork_owner.as_deref().unwrap_or(&owner);
    match github::latest_pr_for_head(
        api_base,
        &token,
        &owner,
        &repo_name,
        head_owner,
        &repo.agent_branch,
    )
    .await
    {
        Ok(pr) => pr,
        Err(e) => {
            tracing::warn!(url = %repo.url, "status: latest_pr_for_head failed: {e:#}");
            None
        }
    }
}

fn read_perma_marker(path: &Path) -> (DateTime<Utc>, String) {
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    let marked_at = parsed
        .get("marked_stuck_at")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
        .unwrap_or_else(Utc::now);
    let count = parsed
        .get("consecutive_failures")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let detail = if count > 0 {
        format!("consecutive_failures: {count}")
    } else {
        String::new()
    };
    (marked_at, detail)
}

fn read_revision_marker(path: &Path) -> DateTime<Utc> {
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    parsed
        .get("marked_at")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
        .unwrap_or_else(Utc::now)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

/// The literal wildcard target accepted by the marker-clear actions. A
/// `change` value of `*` selects a bulk sweep; a `url` value of `*` makes
/// the sweep fleet-wide (every configured repository). The sweep path is
/// intercepted BEFORE `resolve_change_prefix` is ever called, because a
/// literal `*` matches no directory prefix (it would resolve to `NoMatch`
/// and silently clear nothing).
const CLEAR_WILDCARD: &str = "*";

/// Which marker file a wildcard sweep removes.
#[derive(Debug, Clone, Copy)]
enum SweepMarkerKind {
    /// `.perma-stuck.json` (also removes any companion `.ignore-for-queue.json`).
    PermaStuck,
    /// `.needs-spec-revision.json`.
    Revision,
}

impl SweepMarkerKind {
    /// Enumerate the changes in one workspace that carry this kind's marker.
    /// Uses the same per-kind enumeration `queue::list_marker_excluded`
    /// reports for status, so the sweep names exactly the changes status
    /// surfaces.
    fn list_marked(self, workspace: &Path) -> Result<Vec<String>> {
        let (perma, revision) = queue::list_marker_excluded(workspace)?;
        Ok(match self {
            SweepMarkerKind::PermaStuck => perma,
            SweepMarkerKind::Revision => revision,
        })
    }

    /// Remove this kind's marker from one change. For `PermaStuck`, this
    /// also removes any companion `.ignore-for-queue.json` (matching the
    /// exact-form behavior). Returns `true` when an accompanying
    /// ignore-for-queue marker was also removed.
    fn remove_from(self, workspace: &Path, change: &str) -> Result<bool> {
        match self {
            SweepMarkerKind::PermaStuck => {
                queue::remove_perma_stuck_marker(workspace, change)?;
                queue::remove_ignore_for_queue_marker_idempotent(workspace, change)
            }
            SweepMarkerKind::Revision => {
                queue::remove_revision_marker(workspace, change)?;
                Ok(false)
            }
        }
    }
}

/// Execute a wildcard marker-clear sweep. `url` is either the wildcard
/// sentinel (`*` → every configured repository) or a single concrete URL
/// (one repository). For each repository, every change carrying `kind`'s
/// marker is removed; a per-repository read/remove failure is collected
/// and reported alongside the successes WITHOUT aborting the sweep. The
/// response carries a `results` array (one entry per repository) so the
/// chatops formatter can enumerate what was cleared, report an empty repo
/// as "nothing to clear", AND surface per-repo failures fail-loud.
fn sweep_marker_clear(url: &str, kind: SweepMarkerKind, state: &ControlState) -> Value {
    // Resolve the repository set. A `*` url enumerates every configured
    // repository; a concrete url resolves the one repository (an unknown
    // url is an error, mirroring the single-target path).
    let repos: Vec<RepositoryConfig> = if url == CLEAR_WILDCARD {
        state.last_config.load_full().repositories.clone()
    } else {
        match find_repo(state, url) {
            Ok(r) => vec![r],
            Err(e) => return json!({"ok": false, "error": e}),
        }
    };

    let mut results: Vec<Value> = Vec::with_capacity(repos.len());
    for repo in &repos {
        let workspace_path = workspace::resolve_path(&state.paths, repo);
        // Enumerate the changes carrying this marker kind. A read failure
        // is collected per-repo, not fatal: the sweep continues.
        let marked = match kind.list_marked(&workspace_path) {
            Ok(m) => m,
            Err(e) => {
                results.push(json!({
                    "url": repo.url,
                    "error": format!("could not read markers: {e:#}"),
                }));
                continue;
            }
        };
        let mut cleared: Vec<String> = Vec::new();
        let mut removed_ignore = false;
        let mut errors: Vec<String> = Vec::new();
        for change in marked {
            match kind.remove_from(&workspace_path, &change) {
                Ok(also_ignore) => {
                    removed_ignore |= also_ignore;
                    cleared.push(change);
                }
                Err(e) => errors.push(format!("{change}: {e:#}")),
            }
        }
        cleared.sort();
        if errors.is_empty() {
            results.push(json!({
                "url": repo.url,
                "cleared": cleared,
                "removed_ignore_for_queue": removed_ignore,
            }));
        } else {
            // Some markers cleared, some failed: report both — the cleared
            // set AND the per-change failures — without aborting the sweep.
            results.push(json!({
                "url": repo.url,
                "cleared": cleared,
                "removed_ignore_for_queue": removed_ignore,
                "error": errors.join("; "),
            }));
        }
    }
    json!({"ok": true, "results": results})
}

pub(crate) fn handle_clear_perma_stuck(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(u) => u,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let change = match require_str(parsed, "change") {
        Ok(c) => c,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    // Wildcard sweep MUST be intercepted before `resolve_change_prefix`: a
    // literal `*` matches no directory prefix and would resolve to NoMatch
    // (a silent "nothing to clear"). The sweep enumerates the marker
    // directories directly instead.
    if change == CLEAR_WILDCARD {
        return sweep_marker_clear(&url, SweepMarkerKind::PermaStuck, state);
    }
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let workspace_path = workspace::resolve_path(&state.paths, &repo);
    // a40: resolve a leading-prefix slug to its canonical change-directory
    // name, scoped to the action's relevant marker file. Exact matches are
    // a passthrough; the operator-supplied prefix is replaced with the
    // canonical slug everywhere downstream (marker removal AND response
    // JSON), so chatops scrollback names the change that was cleared.
    let change = match queue::resolve_change_prefix(
        &workspace_path,
        &change,
        queue::ChangePrefixMarkerScope::PermaStuck,
    ) {
        Ok(canonical) => {
            if canonical != change {
                tracing::info!(
                    "control_socket: resolved partial change '{change}' → '{canonical}' for action clear_perma_stuck_marker"
                );
            }
            canonical
        }
        Err(e) => {
            tracing::info!(
                "control_socket: clear_perma_stuck_marker prefix '{change}' did not resolve"
            );
            return json!({"ok": false, "error": e.to_operator_message(&change)});
        }
    };
    if let Err(e) = queue::remove_perma_stuck_marker(&workspace_path, &change) {
        return json!({"ok": false, "error": format!("{e:#}")});
    }
    // Per a18: removing `.perma-stuck.json` also removes any companion
    // `.ignore-for-queue.json` (full resolution — the operator is going
    // to retry the change, so the downgrade marker becomes vestigial).
    let removed_ignore =
        match queue::remove_ignore_for_queue_marker_idempotent(&workspace_path, &change) {
            Ok(removed) => removed,
            Err(e) => {
                return json!({
                    "ok": false,
                    "error": format!("removed perma-stuck but failed to remove .ignore-for-queue.json: {e:#}"),
                });
            }
        };
    json!({
        "ok": true,
        "change": change,
        "url": url,
        "removed_ignore_for_queue": removed_ignore,
    })
}

pub(crate) fn handle_clear_revision(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(u) => u,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let change = match require_str(parsed, "change") {
        Ok(c) => c,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    // Wildcard sweep — see `handle_clear_perma_stuck`. Intercepted before
    // `resolve_change_prefix` so a literal `*` never resolves to NoMatch.
    if change == CLEAR_WILDCARD {
        return sweep_marker_clear(&url, SweepMarkerKind::Revision, state);
    }
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let workspace_path = workspace::resolve_path(&state.paths, &repo);
    // a40: leading-prefix → canonical slug, scoped to .needs-spec-revision.json.
    let change = match queue::resolve_change_prefix(
        &workspace_path,
        &change,
        queue::ChangePrefixMarkerScope::NeedsRevision,
    ) {
        Ok(canonical) => {
            if canonical != change {
                tracing::info!(
                    "control_socket: resolved partial change '{change}' → '{canonical}' for action clear_revision_marker"
                );
            }
            canonical
        }
        Err(e) => {
            tracing::info!(
                "control_socket: clear_revision_marker prefix '{change}' did not resolve"
            );
            return json!({"ok": false, "error": e.to_operator_message(&change)});
        }
    };
    match queue::remove_revision_marker(&workspace_path, &change) {
        Ok(()) => json!({"ok": true, "change": change, "url": url}),
        Err(e) => json!({"ok": false, "error": format!("{e:#}")}),
    }
}

/// Stamp `<workspace>/openspec/changes/<change>/.ignore-for-queue.json`
/// for the operator's "skip this broken change AND let siblings proceed"
/// signal. Refuses with a polite error when the change has NEITHER
/// `.perma-stuck.json` NOR `.needs-spec-revision.json` (ignoring a
/// not-broken change is a confusing no-op). On success, stages + commits
/// + pushes the new file on the daemon's agent branch.
pub(crate) fn handle_ignore_for_queue(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(u) => u,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let change = match require_str(parsed, "change") {
        Ok(c) => c,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let marked_by = parsed
        .get("marked_by")
        .and_then(|v| v.as_str())
        .unwrap_or("operator")
        .to_string();
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let workspace_path = workspace::resolve_path(&state.paths, &repo);

    // a40: leading-prefix → canonical slug, scoped to the EitherBlocking
    // pair (.perma-stuck.json OR .needs-spec-revision.json). The resolver
    // guarantees a blocking marker exists on the resolved change, so the
    // previous "no operator-action marker" refusal happens AT resolution
    // time and is no longer duplicated here.
    let change = match queue::resolve_change_prefix(
        &workspace_path,
        &change,
        queue::ChangePrefixMarkerScope::EitherBlocking,
    ) {
        Ok(canonical) => {
            if canonical != change {
                tracing::info!(
                    "control_socket: resolved partial change '{change}' → '{canonical}' for action ignore_for_queue_marker"
                );
            }
            canonical
        }
        Err(e) => {
            tracing::info!(
                "control_socket: ignore_for_queue_marker prefix '{change}' did not resolve"
            );
            return json!({"ok": false, "error": e.to_operator_message(&change)});
        }
    };
    // Refuse if the marker is already present so the operator gets a
    // clear "no change" signal rather than a stealth no-op commit.
    let spec_root = crate::spec_root::SpecRoot::for_repo(&repo, &workspace_path);
    if crate::ignore_for_queue::marker_exists(&spec_root, &change) {
        return json!({
            "ok": false,
            "error": format!(
                "{change} already has .ignore-for-queue.json. No change."
            ),
        });
    }

    // Write the marker file.
    if let Err(e) = crate::ignore_for_queue::write_marker(&spec_root, &change, &marked_by) {
        return json!({"ok": false, "error": format!("writing marker: {e:#}")});
    }

    // Commit + push using the daemon's normal git path. Failure to
    // commit/push is surfaced to the operator and leaves the marker on
    // disk so the next iteration's queue gate honors the operator's
    // intent even if the push didn't land.
    let github_cfg = state.github.load_full();
    let push_remote = if github_cfg.fork_owner.is_some() {
        "fork"
    } else {
        "origin"
    };
    let subject = format!("chore: ignore-for-queue on {change} (operator {marked_by})");
    if let Err(e) = git::add_all(&workspace_path) {
        return json!({
            "ok": false,
            "error": format!("git add failed after writing marker: {e:#}"),
        });
    }
    if let Err(e) = git::commit(&workspace_path, &subject) {
        return json!({
            "ok": false,
            "error": format!("git commit failed after writing marker: {e:#}"),
        });
    }
    if let Err(e) = git::push_force_with_lease(&workspace_path, &repo.agent_branch, push_remote) {
        return json!({
            "ok": false,
            "error": format!("git push failed after commit: {e:#}"),
        });
    }
    json!({"ok": true, "change": change, "url": url})
}

/// Remove `.ignore-for-queue.json` for the named change AND commit +
/// push the removal. Refuses with a polite error when the marker is
/// absent (`@<bot> clear-ignore` against a clean change is a no-op the
/// operator should know about).
pub(crate) fn handle_clear_ignore_for_queue(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(u) => u,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let change = match require_str(parsed, "change") {
        Ok(c) => c,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let workspace_path = workspace::resolve_path(&state.paths, &repo);

    // a40: leading-prefix → canonical slug, scoped to .ignore-for-queue.json.
    let change = match queue::resolve_change_prefix(
        &workspace_path,
        &change,
        queue::ChangePrefixMarkerScope::IgnoreForQueue,
    ) {
        Ok(canonical) => {
            if canonical != change {
                tracing::info!(
                    "control_socket: resolved partial change '{change}' → '{canonical}' for action clear_ignore_for_queue_marker"
                );
            }
            canonical
        }
        Err(e) => {
            tracing::info!(
                "control_socket: clear_ignore_for_queue_marker prefix '{change}' did not resolve"
            );
            return json!({"ok": false, "error": e.to_operator_message(&change)});
        }
    };

    // Remove the marker — propagate the absent-marker error.
    if let Err(e) = queue::remove_ignore_for_queue_marker(&workspace_path, &change) {
        return json!({"ok": false, "error": format!("{e:#}")});
    }

    // Surface which underlying marker the queue resumes blocking on, so
    // the chatops reply can name it.
    let underlying_marker = if queue::is_perma_stuck(&workspace_path, &change) {
        ".perma-stuck.json".to_string()
    } else if queue::is_needs_spec_revision_marked(&workspace_path, &change) {
        ".needs-spec-revision.json".to_string()
    } else {
        "the underlying marker".to_string()
    };

    let github_cfg = state.github.load_full();
    let push_remote = if github_cfg.fork_owner.is_some() {
        "fork"
    } else {
        "origin"
    };
    let subject = format!("chore: clear ignore-for-queue on {change}");
    if let Err(e) = git::add_all(&workspace_path) {
        return json!({
            "ok": false,
            "error": format!("git add failed after removing marker: {e:#}"),
        });
    }
    if let Err(e) = git::commit(&workspace_path, &subject) {
        return json!({
            "ok": false,
            "error": format!("git commit failed after removing marker: {e:#}"),
        });
    }
    if let Err(e) = git::push_force_with_lease(&workspace_path, &repo.agent_branch, push_remote) {
        return json!({
            "ok": false,
            "error": format!("git push failed after commit: {e:#}"),
        });
    }
    json!({
        "ok": true,
        "change": change,
        "url": url,
        "underlying_marker": underlying_marker,
    })
}

/// Idempotent — a missing workspace directory is success (the user wanted
/// it gone, it's gone). Returns the path that was (or would have been)
/// removed in the success response so the chatops reply names a concrete
/// thing.
///
/// Before deleting, the handler signals the per-repo polling task's
/// per-iteration cancel token (when set) and awaits the `iteration_drained`
/// `Notify` so the in-flight executor subprocess exits cleanly instead of
/// losing its CWD to a `remove_dir_all`. The wait is capped by
/// `executor.wipe_drain_timeout_secs`; the deletion runs regardless of
/// whether the drain completed, since the directory is going to be gone
/// either way.
pub(crate) async fn handle_wipe_workspace(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(u) => u,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let workspace_path = workspace::resolve_path(&state.paths, &repo);
    let display = workspace_path.display().to_string();

    // Look up the per-repo handle's iteration_cancel handle + drained
    // Notify under the briefest possible lock so the lookup never blocks
    // the chatops listener for longer than a hashmap probe + Arc clone.
    let (iter_token, drained_notify): (Option<CancellationToken>, Option<Arc<Notify>>) = {
        let guard = state.repo_tasks.lock().unwrap();
        match guard.get(&url) {
            Some(h) => {
                let token = h.iteration_cancel.lock().unwrap().clone();
                (token, Some(h.iteration_drained.clone()))
            }
            None => (None, None),
        }
    };

    // Drain coordination. The four-outcome decision tree (per the
    // wipe-workspace spec): drained-cleanly / drain-timeout / no-iteration /
    // already-absent. The first two require an in-flight iteration; the
    // third is the "between iterations, just delete" short-circuit; the
    // fourth is the idempotent no-op.
    let drain_timeout_secs = state
        .last_config
        .load_full()
        .executor
        .wipe_drain_timeout_secs_clamped();
    let drain_outcome: String = if let (Some(token), Some(notify)) = (iter_token, drained_notify) {
        let start = std::time::Instant::now();
        // Register interest in the Notify BEFORE firing the cancel so we
        // don't miss the wake. `Notify::notified()` only observes events
        // that fire after the future is created.
        let notified = notify.notified();
        tokio::pin!(notified);
        token.cancel();
        if drain_timeout_secs == 0 {
            // Special case: skip the await entirely. The wipe runs
            // immediately whether the iteration responded or not. Treat
            // as a drain-timeout outcome so the operator's chatops reply
            // still signals "we did not wait" rather than misleadingly
            // claiming a clean drain.
            "drain timeout — iteration may have been stuck".to_string()
        } else {
            match tokio::time::timeout(
                std::time::Duration::from_secs(drain_timeout_secs),
                notified.as_mut(),
            )
            .await
            {
                Ok(()) => {
                    let elapsed = start.elapsed().as_secs_f64();
                    format!("drained cleanly in {elapsed:.1}s")
                }
                Err(_) => {
                    tracing::warn!(
                        url = %url,
                        timeout_secs = drain_timeout_secs,
                        "wipe-workspace drain timeout: the in-flight iteration for `{url}` did not exit \
                         within {drain_timeout_secs}s of the per-iteration cancel signal; \
                         proceeding with the workspace deletion regardless"
                    );
                    "drain timeout — iteration may have been stuck".to_string()
                }
            }
        }
    } else {
        "no iteration in flight".to_string()
    };

    if !workspace_path.exists() {
        // Existing already-absent shape preserved (no behaviour change for
        // operators who scripted against the prior `ok=true,
        // already_absent=true` payload). The drain_outcome is appended.
        return json!({
            "ok": true,
            "path": display,
            "url": url,
            "already_absent": true,
            "drain_outcome": drain_outcome,
        });
    }
    match std::fs::remove_dir_all(&workspace_path) {
        Ok(()) => json!({
            "ok": true,
            "path": display,
            "url": url,
            "already_absent": false,
            "drain_outcome": drain_outcome,
        }),
        Err(e) => json!({
            "ok": false,
            "error": format!("removing {display}: {e}"),
        }),
    }
}

/// Rebuild canonical specs from archive history. Two modes:
///   - `immediate: true`: SIGTERM the running executor (via the busy
///     marker's subprocess sidecar), wait up to 30s, then run the
///     rebuild synchronously and return the report in the response.
///   - `immediate: false`: set `pending_rebuild = true` on the named
///     repo's polling task state and return immediately. The next
///     polling iteration picks up the flag and runs the rebuild
///     instead of the normal queue walk.
pub(crate) async fn handle_rebuild_specs(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(u) => u,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let immediate = parsed
        .get("immediate")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let workspace = workspace::resolve_path(&state.paths, &repo);

    if immediate {
        if let Err(e) =
            crate::cli::sync_specs::coordinate_with_daemon(&workspace, true).await
        {
            return json!({
                "ok": false,
                "error": format!("--immediate coordination failed: {e:#}"),
            });
        }
        match crate::cli::sync_specs::rebuild_canonical(&workspace).await {
            Ok(report) => {
                let report_val = serde_json::to_value(&report).unwrap_or(Value::Null);
                json!({
                    "ok": true,
                    "url": url,
                    "immediate": true,
                    "report": report_val,
                })
            }
            Err(e) => json!({
                "ok": false,
                "error": format!("rebuild failed: {e:#}"),
            }),
        }
    } else {
        // Set the per-repo task's pending_rebuild flag.
        let flag = {
            let guard = state.repo_tasks.lock().unwrap();
            guard.get(&url).map(|h| h.pending_rebuild.clone())
        };
        match flag {
            Some(f) => {
                f.store(true, std::sync::atomic::Ordering::SeqCst);
                json!({
                    "ok": true,
                    "url": url,
                    "immediate": false,
                    "scheduled": true,
                    "poll_interval_sec": repo.poll_interval_sec,
                })
            }
            None => json!({
                "ok": false,
                "error": format!(
                    "no live polling task for `{url}` (daemon may not have spawned it yet)"
                ),
            }),
        }
    }
}

/// Queue an audit-triage run for the repo whose audit produced the
/// thread named by `thread_ts`. Reads the audit-thread state to resolve
/// repo URL + audit type, pushes the `thread_ts` onto the matching
/// `RepoTaskHandle::pending_triages`, and returns the repo's poll
/// interval so the chatops reply can name an ETA.
pub(crate) async fn handle_trigger_audit_action(parsed: &Value, state: &ControlState) -> Value {
    let thread_ts = match require_str(parsed, "thread_ts") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let state_root = crate::audits::threads::default_state_root(&state.paths);
    let audit_state = match crate::audits::threads::read_state(&state_root, &thread_ts) {
        Ok(Some(s)) => s,
        Ok(None) => {
            return json!({
                "ok": false,
                "error": format!(
                    "no audit-thread state for thread_ts `{thread_ts}` (the chatops dispatcher should have caught this earlier)"
                ),
            });
        }
        Err(e) => {
            return json!({"ok": false, "error": format!("reading audit-thread state: {e:#}")});
        }
    };

    let repo = match find_repo(state, &audit_state.repo_url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };

    let queue_slot = {
        let guard = state.repo_tasks.lock().unwrap();
        guard.get(&audit_state.repo_url).map(|h| h.pending_triages.clone())
    };
    let queue = match queue_slot {
        Some(q) => q,
        None => {
            return json!({
                "ok": false,
                "error": format!(
                    "no live polling task for `{}` (daemon may not have spawned it yet)",
                    audit_state.repo_url
                ),
            });
        }
    };
    {
        let mut g = queue.lock().unwrap();
        // De-dup: if the same thread_ts is already queued (e.g. the
        // operator double-clicked `send it`), keep just the one entry.
        if !g.iter().any(|t| t == &thread_ts) {
            g.push(thread_ts.clone());
        }
    }

    json!({
        "ok": true,
        "thread_ts": thread_ts,
        "url": audit_state.repo_url,
        "audit_type": audit_state.audit_type,
        "poll_interval_sec": repo.poll_interval_sec,
    })
}

/// Append `audit_type` to the named repo's `pending_audit_runs` queue
/// so the next polling iteration's audit phase runs it unconditionally
/// (bypassing cadence). De-duplicated: appending a value already in the
/// queue is a no-op (the response still reports success). The request
/// identifies the repo by `url` (chatops verb path) OR by `workspace`
/// (CLI `audit run` path — the daemon does the workspace-to-URL
/// resolution against its configured repo list). The response echoes
/// the canonical `audit_type` and resolved `url` so the chatops/CLI
/// caller can build an ack with the daemon's authoritative names;
/// `poll_interval_sec` lets the caller compute the ETA clause.
pub(crate) fn handle_queue_audit(parsed: &Value, state: &ControlState) -> Value {
    let audit_type = match require_str(parsed, "audit_type") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    // Resolve target URL: explicit `url` wins; otherwise look up by
    // `workspace` path (matched against each configured repo's
    // `workspace::resolve_path`).
    let url = if let Some(u) = parsed.get("url").and_then(|v| v.as_str()) {
        u.to_string()
    } else if let Some(ws) = parsed.get("workspace").and_then(|v| v.as_str()) {
        match find_repo_by_workspace(state, std::path::Path::new(ws)) {
            Some(u) => u,
            None => {
                return json!({
                    "ok": false,
                    "error": format!(
                        "no managed repository found for workspace path `{ws}`; the daemon is managing: {}",
                        managed_repo_list_for_error(state)
                    ),
                });
            }
        }
    } else {
        return json!({"ok": false, "error": "missing `url` or `workspace` field"});
    };
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let queue_slot = {
        let guard = state.repo_tasks.lock().unwrap();
        guard.get(&url).map(|h| h.pending_audit_runs.clone())
    };
    let queue = match queue_slot {
        Some(q) => q,
        None => {
            return json!({
                "ok": false,
                "error": format!(
                    "no live polling task for `{url}` (daemon may not have spawned it yet)"
                ),
            });
        }
    };
    // Carry the originating chat context (when present) so the scheduler can
    // post the terminal completion notification back to the operator's thread.
    let origin = parsed
        .get("channel")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|channel| crate::polling_loop::ChatOrigin {
            channel: channel.to_string(),
            thread_ts: parsed
                .get("thread_ts")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
        });
    // Resolve the repo's workspace so the mutated queue can be mirrored to
    // its durable `pending-audit-runs/<basename>.json` file the instant the
    // enqueue is acknowledged below — closing the enqueue→restart window
    // (persist-on-demand-audit-queue).
    let workspace = workspace::resolve_path(&state.paths, &repo);
    {
        let mut g = queue.lock().unwrap();
        if !g.iter().any(|a| a.audit_type == audit_type) {
            g.push(crate::polling_loop::QueuedAudit {
                audit_type: audit_type.clone(),
                origin,
            });
        }
        // Persist on every mutation. Best-effort: a write failure is logged
        // and never fails the enqueue (the in-memory queue stays
        // authoritative for the live process).
        if let Err(e) =
            crate::polling_loop::save_pending_audit_runs(&state.paths, &workspace, g.as_slice())
        {
            tracing::warn!(
                url = %url,
                "queue_audit: failed to persist pending-audit-runs queue (in-memory queue remains authoritative): {e:#}"
            );
        }
    }
    json!({
        "ok": true,
        "url": url,
        "audit_type": audit_type,
        "poll_interval_sec": repo.poll_interval_sec,
    })
}


/// Handle the `promote_issue_candidate` action (a010). Synchronously
/// promotes a posted issue-lane candidate the chatops `send it` dispatcher
/// matched to a maintainer's in-thread reply: resolves the repo AND its
/// workspace, loads the candidate, AND — when it is still `Posted` — writes
/// `issues/<slug>/` (its `issue.md` + `tasks.md`, plus the quarantined
/// `report-body.md` for a public-origin candidate) AND flips the candidate's
/// status to `Promoted`. Writing the unit IS the queue (the issues-lane
/// walker picks up any ready `issues/<slug>/`). Idempotent: an already-
/// `Promoted` candidate writes nothing further AND reports `already_promoted`
/// so the dispatcher can word its reply without re-writing. Mirrors
/// [`handle_queue_clear_survey`]'s synchronous-filesystem-work shape.
///
/// `channel` AND `thread_ts` are required so the request identifies the
/// originating thread (matching the survey/audit action contract); the
/// promotion itself is a pure filesystem write that needs neither.
pub(crate) fn handle_promote_issue_candidate(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let candidate_id = match require_str(parsed, "candidate_id") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    if let Err(e) = require_str(parsed, "channel") {
        return json!({"ok": false, "error": e});
    }
    if let Err(e) = require_str(parsed, "thread_ts") {
        return json!({"ok": false, "error": e});
    }
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let workspace = crate::workspace::resolve_path(&state.paths, &repo);
    let candidate =
        match crate::lanes::ingestion::read_candidate(&state.paths.state, &candidate_id) {
            Ok(Some(c)) => c,
            Ok(None) => {
                return json!({
                    "ok": false,
                    "error": format!("no issue candidate `{candidate_id}` recorded"),
                });
            }
            Err(e) => {
                return json!({"ok": false, "error": format!("reading issue candidate: {e:#}")});
            }
        };
    match candidate.status {
        crate::lanes::ingestion::CandidateStatus::Promoted => json!({
            "ok": true,
            "already_promoted": true,
            "slug": candidate.slug,
        }),
        crate::lanes::ingestion::CandidateStatus::Posted => {
            match crate::lanes::ingestion::promote_candidate(
                &workspace,
                &state.paths.state,
                &candidate,
            ) {
                Ok(dir) => json!({
                    "ok": true,
                    "slug": candidate.slug,
                    "path": dir.display().to_string(),
                }),
                Err(e) => json!({"ok": false, "error": format!("{e:#}")}),
            }
        }
    }
}

/// Handle the `review_target` action (a59): run an on-demand code review of
/// a PR, commit, or target against the repository's local clone, then report
/// the verdict + concerns back. The review reuses the existing agentic
/// reviewer (sandbox, `submit_review`, reads-on-demand) AND is advisory +
/// read-only — it opens NO revision, modifies NO code, changes NO marker.
///
/// A session that fails to produce a valid verdict SURFACES the failure
/// (gatekeepers-fail-closed) instead of reporting a clean pass: the response
/// carries `ok: false` with the discard reason, so the chatops reply / CLI
/// exit code shows the failure rather than a fabricated approval.
///
/// For a `pr` target the verdict is ALSO posted as a PR comment (best
/// effort) when a github token resolves; a comment-post failure logs a WARN
/// and never changes the chat verdict.
pub(crate) async fn handle_review_target(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let target_tokens: Vec<String> = match parsed.get("target").and_then(|v| v.as_array()) {
        Some(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        None => {
            return json!({"ok": false, "error": "missing `target` field (array of tokens)"});
        }
    };
    let spec = match crate::code_reviewer::ReviewTargetSpec::parse(&target_tokens) {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let repo = match find_repo_by_substring(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let workspace = crate::workspace::resolve_path(&state.paths, &repo);
    if !workspace.is_dir() {
        return json!({
            "ok": false,
            "error": format!(
                "workspace {} does not exist yet; the daemon must clone the repo before an \
                 on-demand review can run",
                workspace.display()
            ),
        });
    }

    // The on-demand review reuses the AGENTIC reviewer machinery; it has no
    // oneshot-HTTP equivalent (the read-on-demand sandbox + submit_review is
    // the whole point). A disabled or oneshot reviewer surfaces a clear error
    // rather than silently doing nothing.
    let reviewer = match state.reviewer.load_full().as_ref() {
        Some(r) => r.clone(),
        None => {
            return json!({
                "ok": false,
                "error": "reviewer is not configured/enabled; on-demand review requires an \
                          agentic reviewer (set reviewer.enabled and reviewer.kind: agentic)",
            });
        }
    };
    if reviewer.kind() != crate::config::ReviewerKind::Agentic {
        return json!({
            "ok": false,
            "error": "on-demand review requires the agentic reviewer transport, but the \
                      reviewer resolved to `oneshot` (its CLI may be unavailable on this host). \
                      Install the reviewer CLI or set reviewer.kind: agentic.",
        });
    }

    // Resolve the target into a review SURFACE against the local clone. The
    // remote PR refs live on `origin`.
    let surface = match crate::code_reviewer::resolve_review_surface(
        &spec,
        &workspace,
        &repo.base_branch,
        "origin",
    ) {
        Ok(s) => s,
        Err(e) => {
            return json!({"ok": false, "error": format!("resolving review target: {e:#}")});
        }
    };

    // Run the review. No archived-change briefs are loaded for an on-demand
    // review — the surface (diff or target) is the reviewer's whole context.
    let outcome = match crate::code_reviewer::run_on_demand_review(
        reviewer.as_ref(),
        &surface,
        Vec::new(),
        &workspace,
    )
    .await
    {
        Ok(o) => o,
        Err(e) => {
            return json!({"ok": false, "error": format!("on-demand review failed: {e:#}")});
        }
    };

    let report = match outcome {
        crate::code_reviewer::OnDemandReviewOutcome::Reviewed(r) => r,
        crate::code_reviewer::OnDemandReviewOutcome::Discarded { reason } => {
            // Gatekeepers-fail-closed: a no-verdict session surfaces the
            // failure; it does NOT report a clean pass.
            return json!({
                "ok": false,
                "discarded": true,
                "error": reason,
            });
        }
    };

    // Optional PR comment for a `pr` target (best effort; never alters the
    // chat verdict).
    if let crate::code_reviewer::ReviewTargetSpec::Pr { number } = &spec {
        post_on_demand_pr_comment(state, &repo, *number, &report).await;
    }

    json!({
        "ok": true,
        "verdict": report.verdict.label(),
        "body": report.markdown,
        "sessions": report.sessions,
        "chunks": report.chunk_labels,
        "attribution": report.attribution,
    })
}

/// Best-effort PR-comment post for an on-demand `pr` review (a59). Resolves
/// the github token AND posts the verdict + body as a PR comment. Every
/// failure (token resolve, URL parse, HTTP) logs a WARN and returns — the
/// chat verdict already carries the result, so a comment-post failure must
/// not fail the review.
async fn post_on_demand_pr_comment(
    state: &ControlState,
    repo: &RepositoryConfig,
    pr_number: u64,
    report: &crate::code_reviewer::OnDemandReviewReport,
) {
    let github_cfg = state.github.load_full();
    let (owner, repo_name) = match github::parse_repo_url(&repo.url) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(url = %repo.url, "on-demand review: parse_repo_url failed: {e:#}");
            return;
        }
    };
    let token = match github_credentials::resolve_token(&github_cfg, &owner) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(url = %repo.url, "on-demand review: github token resolve failed: {e:#}");
            return;
        }
    };
    let body = format!(
        "## On-demand code review\n\n**Verdict: {}**\n\n{}",
        report.verdict.label(),
        report.markdown
    );
    use crate::forge::Forge;
    if let Err(e) = crate::forge::GithubForge::with_api_base(github::DEFAULT_API_BASE)
        .post_comment(&token, &owner, &repo_name, pr_number, &body)
        .await
    {
        tracing::warn!(
            url = %repo.url,
            pr_number,
            "on-demand review: PR comment post failed: {e:#}"
        );
    }
}

/// Handle the `recent_commits_log` action (code-rollback-recovery's
/// read-only `log` command). Resolves the repo by substring (the same way
/// `review` does), reads the base branch's most recent `count` commits
/// (newest-first), AND returns them. Modifies NOTHING — no branch,
/// workspace, or marker is touched.
pub(crate) fn handle_recent_commits_log(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    // Optional count; default to a small page (20).
    let count = parsed
        .get("count")
        .and_then(|v| v.as_u64())
        .map(|n| n.max(1) as usize)
        .unwrap_or(20);
    let repo = match find_repo_by_substring(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let workspace = workspace::resolve_path(&state.paths, &repo);
    if !workspace.is_dir() {
        return json!({
            "ok": false,
            "error": format!(
                "workspace {} does not exist yet; the daemon must clone the repo before its log \
                 can be read",
                workspace.display()
            ),
        });
    }
    match git::recent_commits(&workspace, &repo.base_branch, count) {
        Ok(entries) => {
            let commits: Vec<Value> = entries
                .iter()
                .map(|e| {
                    json!({
                        "short_sha": e.short_sha,
                        "subject": e.subject,
                        "date": e.date,
                    })
                })
                .collect();
            json!({
                "ok": true,
                "url": repo.url,
                "base_branch": repo.base_branch,
                "commits": commits,
            })
        }
        Err(e) => json!({"ok": false, "error": format!("reading commit log: {e:#}")}),
    }
}

/// Handle the `survival_analysis` action (review-survival-provenance's
/// `survives` command). Resolves the repo by substring (same as `review` /
/// `log`), then reports which of a past PR's OR commit's changes still
/// survive verbatim at `HEAD` (per-file pre-filter + line-level blame).
/// The request carries exactly one of `pr` (number) / `commit` (sha). The
/// optional `detect_moves` flag enables `git blame -M -C`. Read-only:
/// touches no branch, workspace, or marker.
pub(crate) fn handle_survival_analysis(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let target = match parsed.get("pr").and_then(|v| v.as_u64()) {
        Some(n) => crate::survival::SurvivalTarget::Pr { number: n },
        None => match parsed.get("commit").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => {
                crate::survival::SurvivalTarget::Commit { sha: s.to_string() }
            }
            _ => {
                return json!({
                    "ok": false,
                    "error": "missing target: provide exactly one of `pr` (number) or `commit` (sha)",
                });
            }
        },
    };
    let detect_moves = parsed
        .get("detect_moves")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let repo = match find_repo_by_substring(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let workspace = workspace::resolve_path(&state.paths, &repo);
    if !workspace.is_dir() {
        return json!({
            "ok": false,
            "error": format!(
                "workspace {} does not exist yet; the daemon must clone the repo before survival \
                 analysis can run",
                workspace.display()
            ),
        });
    }
    match crate::survival::analyze_survival(
        &workspace,
        &repo.base_branch,
        "origin",
        &target,
        detect_moves,
    ) {
        Ok(report) => json!({
            "ok": true,
            "url": repo.url,
            "report": report.render(),
            "review_focus_paths": report.review_focus_paths(),
            "surviving_lines": report.total_surviving_lines(),
        }),
        Err(e) => json!({"ok": false, "error": format!("survival analysis failed: {e:#}")}),
    }
}

/// Handle the `provenance_lookup` action (review-survival-provenance's
/// `blame` command). Resolves the repo by substring, then runs `git blame`
/// at `HEAD` for the requested `path` line range (`start`..=`end`),
/// reporting each line's introducing commit AND the PR when the commit's
/// subject names one (no fabricated PR). Read-only.
pub(crate) fn handle_provenance_lookup(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let path = match require_str(parsed, "path") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let start = parsed.get("start").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let end = parsed
        .get("end")
        .and_then(|v| v.as_u64())
        .unwrap_or(start as u64) as usize;
    if start == 0 || end == 0 || start > end {
        return json!({
            "ok": false,
            "error": "line range invalid: start/end must be >= 1 and start <= end",
        });
    }
    let detect_moves = parsed
        .get("detect_moves")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let repo = match find_repo_by_substring(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let workspace = workspace::resolve_path(&state.paths, &repo);
    if !workspace.is_dir() {
        return json!({
            "ok": false,
            "error": format!(
                "workspace {} does not exist yet; the daemon must clone the repo before \
                 provenance can be read",
                workspace.display()
            ),
        });
    }
    match crate::survival::analyze_provenance(&workspace, &path, start, end, detect_moves) {
        Ok(report) => json!({
            "ok": true,
            "url": repo.url,
            "report": report.render(),
        }),
        Err(e) => json!({"ok": false, "error": format!("provenance lookup failed: {e:#}")}),
    }
}

/// Parse the rollback depth (count OR sha) from a request. Exactly one of
/// `count` / `sha` must be present.
fn parse_rollback_depth(parsed: &Value) -> std::result::Result<crate::rollback::RollbackDepth, String> {
    let count = parsed.get("count").and_then(|v| v.as_u64());
    let sha = parsed.get("sha").and_then(|v| v.as_str()).map(|s| s.to_string());
    match (count, sha) {
        (Some(_), Some(_)) => {
            Err("provide EITHER `count` OR `sha`, not both".to_string())
        }
        (Some(n), None) => Ok(crate::rollback::RollbackDepth::Count(n as usize)),
        (None, Some(s)) => Ok(crate::rollback::RollbackDepth::Sha(s)),
        (None, None) => Err("missing rollback depth: provide `count` (last N commits) OR `sha` (target commit)".to_string()),
    }
}

/// Handle the `rollback_recovery` action (code-rollback-recovery). Rolls the
/// repo's CODE back by a count OR to a SHA WHILE unarchiving the
/// changes/issues archived in the range, riding the normal push + PR flow
/// (honoring `auto_submit_pr`). A `dry_run: true` request (the CLI default)
/// returns the preview — exactly what WOULD be rolled back AND unarchived —
/// without changing any branch, workspace, archive, or canon.
pub(crate) async fn handle_rollback_recovery(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let depth = match parse_rollback_depth(parsed) {
        Ok(d) => d,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let dry_run = parsed.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(true);

    let repo = match find_repo_by_substring(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let github_cfg = state.github.load_full();
    let workspace = workspace::resolve_path(&state.paths, &repo);

    let fork_url = match github_cfg.fork_owner.as_deref() {
        Some(owner) => match crate::github::derive_fork_url(&repo.url, owner) {
            Ok(u) => Some(u),
            Err(e) => return json!({"ok": false, "error": format!("deriving fork url: {e:#}")}),
        },
        None => None,
    };
    let fork_arg = fork_url.as_deref().map(|u| (u, repo.agent_branch.as_str()));

    // DRY-RUN PATH: genuinely READ-ONLY — it inspects history to report what
    // WOULD happen and mutates NOTHING, so it neither preempts an in-flight
    // pass NOR acquires the busy marker, AND it cannot clobber a concurrent
    // agentic session that has the same workspace bind-mounted writable.
    //
    // Read-only resolution: ensure the repo exists (clone if absent; on an
    // existing workspace `ensure_initialized` only does `git fetch origin`,
    // which updates the remote-tracking refs WITHOUT touching the working
    // tree or the checked-out branch) and then resolve the plan against
    // `origin/<base>` — the remote-tracking ref the fetch refreshed. No
    // `git checkout`, no `git reset --hard`, no other working-tree mutation.
    // The collision check reads active dirs only, so it stays.
    if dry_run {
        if let Err(e) =
            workspace::ensure_initialized(&state.paths, &workspace, &repo.url, fork_arg)
        {
            return json!({"ok": false, "error": format!("workspace init failed: {e:#}")});
        }
        // The base branch always lives on `origin` (the live path resets to
        // `origin/<base>` regardless of any fork remote, which is only the
        // push target). Refresh just that tracking ref read-only.
        if let Err(e) = git::fetch_remote_branch(&workspace, "origin", &repo.base_branch) {
            tracing::warn!(
                url = %repo.url,
                "rollback dry-run: `git fetch origin {}` failed (resolving against the local \
                 tracking ref as-is): {e:#}",
                repo.base_branch
            );
        }
        let resolve_ref = format!("origin/{}", repo.base_branch);
        let plan = match crate::rollback::resolve_plan(
            &workspace,
            &repo.base_branch,
            &resolve_ref,
            &depth,
        ) {
            Ok(p) => p,
            Err(e) => return json!({"ok": false, "error": format!("{e:#}")}),
        };
        let preview = crate::rollback::format_preview(&workspace, &plan);
        let collisions = crate::rollback::detect_collisions(&workspace, &plan);
        return json!({
            "ok": true,
            "dry_run": true,
            "url": repo.url,
            "preview": preview,
            "code_only": plan.is_code_only(),
            "commit_count": plan.commit_count(),
            "changes": plan.changes.iter().map(|u| u.slug.clone()).collect::<Vec<_>>(),
            "issues": plan.issues.iter().map(|u| u.slug.clone()).collect::<Vec<_>>(),
            "has_collisions": !collisions.is_empty(),
        });
    }

    // LIVE PATH: this mutates the workspace tree AND branch, so it conforms
    // to the workspace-mutating control-socket invariant — preempt any
    // in-flight pass, acquire the per-repo busy marker, and hold it across
    // the WHOLE operation (clean-base preamble → recreate → prepare → push
    // → PR). The `_busy_guard` binding keeps the marker held until the
    // handler returns; its `Drop` releases on EVERY path (success OR error),
    // so no new pass can start mid-op.
    // CONFIRMED rollback is the operator's emergency override: it uses the
    // FORCEFUL reclaim (escalate past a stuck/ambiguous marker instead of
    // failing `Busy`), so it always ends up holding the marker. It therefore
    // never returns a "still busy" error — only an Internal filesystem error.
    let (_busy_guard, preempted_change) =
        match preempt_and_force_acquire_busy_marker(state, &repo, &workspace).await {
            Ok(outcome) => (outcome.guard, outcome.preempted_change),
            Err(PreemptAcquireError::Busy(msg)) => {
                // Unreachable for the forceful path (it escalates rather than
                // returning Busy); surfaced defensively rather than panicking.
                return json!({"ok": false, "error": msg});
            }
            Err(PreemptAcquireError::Internal(msg)) => {
                return json!({"ok": false, "error": format!("rollback preempt failed: {msg}")});
            }
        };

    // Initialize the workspace (clone if needed; sync the base branch) so the
    // plan is resolved against the live base-branch tip.
    if let Err(e) =
        workspace::ensure_initialized(&state.paths, &workspace, &repo.url, fork_arg)
    {
        return json!({"ok": false, "error": format!("workspace init failed: {e:#}")});
    }

    // Resolve the plan against a clean base branch. The base must reflect the
    // remote tip; reset to it so a stale local base does not skew the range.
    if let Err(e) = git::checkout(&workspace, &repo.base_branch) {
        return json!({"ok": false, "error": format!("checkout base branch failed: {e:#}")});
    }
    if let Err(e) = git::reset_hard_to_remote(&workspace, &repo.base_branch) {
        // Non-fatal in a fresh clone that already matches; log + proceed.
        tracing::warn!(
            url = %repo.url,
            "rollback: reset --hard to origin/{} failed (proceeding with local base): {e:#}",
            repo.base_branch
        );
    }

    // The live path resolves against the checked-out, freshly-reset clean
    // base branch (its working tree is the base tip), so the resolve ref IS
    // the base branch name.
    let plan = match crate::rollback::resolve_plan(
        &workspace,
        &repo.base_branch,
        &repo.base_branch,
        &depth,
    ) {
        Ok(p) => p,
        Err(e) => return json!({"ok": false, "error": format!("{e:#}")}),
    };
    let preview = crate::rollback::format_preview(&workspace, &plan);

    // A CONFIRMED rollback does NOT abort on collisions: it RECONCILES to the
    // target state (active-exactly-once, redundant archive entry removed) in
    // `prepare_rolled_back_tree` below. The dry-run path above still reports
    // `has_collisions` informationally; the confirmed path resolves them.

    // Prepare the rolled-back state on a FRESH agent branch at the base tip,
    // so the rollback rides the normal push + PR flow rather than
    // force-pushing the base branch.
    if let Err(e) = git::recreate_branch(&workspace, &repo.agent_branch) {
        return json!({"ok": false, "error": format!("recreating agent branch failed: {e:#}")});
    }
    if let Err(e) = crate::rollback::prepare_rolled_back_tree(&workspace, &plan) {
        return json!({"ok": false, "error": format!("preparing rolled-back tree: {e:#}")});
    }

    let push_remote = if github_cfg.fork_owner.is_some() {
        "fork"
    } else {
        "origin"
    };
    if let Err(e) = git::push_force_with_lease(&workspace, &repo.agent_branch, push_remote) {
        return json!({"ok": false, "error": format!("pushing rolled-back branch failed: {e:#}")});
    }

    let pr_body = crate::rollback::build_pr_body(&workspace, &plan);
    let title = format!(
        "rollback: restore code ({} commit(s)){}",
        plan.commit_count(),
        if plan.is_code_only() {
            " — code-only".to_string()
        } else {
            format!(
                " + unarchive {} change(s) / {} issue(s)",
                plan.changes.len(),
                plan.issues.len()
            )
        }
    );

    // Honor `auto_submit_pr`: open a PR by default, OR surface the pushed
    // branch with no PR (`BranchPushedNoPr`) when an install set it false.
    if !repo.auto_submit_pr {
        let (owner, repo_name) = match crate::github::parse_repo_url(&repo.url) {
            Ok(p) => p,
            Err(e) => return json!({"ok": false, "error": format!("parsing repo url: {e:#}")}),
        };
        let branch_url = crate::polling_loop::compose_branch_url(
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
        let suggested =
            crate::polling_loop::push_only_command(repo.forge.as_ref(), pr_base, &repo.agent_branch);
        return json!({
            "ok": true,
            "dry_run": false,
            "outcome": "branch_pushed_no_pr",
            "url": repo.url,
            "branch_url": branch_url,
            "suggested_command": suggested,
            "code_only": plan.is_code_only(),
            "commit_count": plan.commit_count(),
            "changes": plan.changes.iter().map(|u| u.slug.clone()).collect::<Vec<_>>(),
            "issues": plan.issues.iter().map(|u| u.slug.clone()).collect::<Vec<_>>(),
            "preview": preview,
            "preempted_change": preempted_change,
        });
    }

    // Reuse an existing agent-branch PR (the force-push already updated its
    // head) instead of raw-creating, which 422s when a PR already exists; the
    // existing PR's title + body are updated to the rollback. Create only when
    // none exists.
    match crate::polling_loop::open_or_update_rollback_pull_request(
        &state.paths,
        &repo,
        &github_cfg,
        &repo.agent_branch,
        &repo.base_branch,
        &title,
        &pr_body,
    )
    .await
    {
        Ok(pr_url) => json!({
            "ok": true,
            "dry_run": false,
            "outcome": "pr_opened",
            "url": repo.url,
            "pr_url": pr_url,
            "code_only": plan.is_code_only(),
            "commit_count": plan.commit_count(),
            "changes": plan.changes.iter().map(|u| u.slug.clone()).collect::<Vec<_>>(),
            "issues": plan.issues.iter().map(|u| u.slug.clone()).collect::<Vec<_>>(),
            "preempted_change": preempted_change,
        }),
        Err(e) => json!({"ok": false, "error": format!("opening rollback PR: {e:#}")}),
    }
}

// =====================================================================
// Defer / undefer (a02): set a work unit aside out of both lanes and
// resume it, riding the agent-branch + push + PR flow.
// =====================================================================

/// The on-disk kind + concrete path of a unit a `defer`/`undefer` acts
/// on. A change is always a directory; an issue is EITHER a single `.md`
/// file OR a directory — `DeferKind` records which so the move preserves
/// the form exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeferKind {
    /// `openspec/changes/<slug>/` ↔ `deferred-changes/<slug>/`.
    Change,
    /// `issues/<slug>.md` ↔ `deferred-issues/<slug>.md`.
    IssueFile,
    /// `issues/<slug>/` ↔ `deferred-issues/<slug>/`.
    IssueDir,
}

/// Relative paths a change/issue unit occupies in its lane and in the
/// deferred area, derived from the slug + kind.
struct DeferPaths {
    /// Path inside a lane (`openspec/changes/<slug>` or `issues/<slug>(.md)`).
    lane: PathBuf,
    /// Path inside the deferred area (`deferred-changes/<slug>` or
    /// `deferred-issues/<slug>(.md)`).
    deferred: PathBuf,
}

impl DeferKind {
    /// Compute the absolute lane + deferred paths for `slug` under
    /// `workspace`.
    fn paths(&self, workspace: &Path, slug: &str) -> DeferPaths {
        match self {
            DeferKind::Change => DeferPaths {
                lane: workspace.join("openspec/changes").join(slug),
                deferred: workspace.join("deferred-changes").join(slug),
            },
            DeferKind::IssueFile => DeferPaths {
                lane: workspace.join("issues").join(format!("{slug}.md")),
                deferred: workspace.join("deferred-issues").join(format!("{slug}.md")),
            },
            DeferKind::IssueDir => DeferPaths {
                lane: workspace.join("issues").join(slug),
                deferred: workspace.join("deferred-issues").join(slug),
            },
        }
    }

    fn label(&self) -> &'static str {
        match self {
            DeferKind::Change => "change",
            DeferKind::IssueFile | DeferKind::IssueDir => "issue",
        }
    }
}

/// Result of locating a unit for a defer/undefer operation.
pub(crate) enum DeferLocate {
    /// The unit's lane location holds it; a move is required.
    NeedsMove(DeferKind),
    /// The unit is already at its destination (deferred location for
    /// defer, lane location for undefer) AND absent from the source —
    /// idempotent no-op.
    AlreadyDone(DeferKind),
    /// No unit by that slug exists at the relevant source OR destination.
    NotFound,
    /// The slug names more than one candidate (e.g. both a change AND an
    /// issue) — ambiguous; the message names the candidates.
    Ambiguous(String),
}

/// For each of the three forms, classify a slug's presence at a `source`
/// (lane for defer; deferred for undefer) vs. `dest`. Returns the kinds
/// present at source and the kinds present at dest, so the caller can map
/// to a [`DeferLocate`].
///
/// `at_source` / `at_dest` are predicates that, given a [`DeferKind`],
/// report whether that form exists at the source / destination location
/// respectively.
fn classify_defer(
    at_source: impl Fn(&DeferKind) -> bool,
    at_dest: impl Fn(&DeferKind) -> bool,
    candidate_paths: impl Fn(&DeferKind) -> (PathBuf, PathBuf),
) -> DeferLocate {
    let forms = [DeferKind::Change, DeferKind::IssueFile, DeferKind::IssueDir];
    let present_source: Vec<DeferKind> =
        forms.iter().filter(|k| at_source(k)).cloned().collect();

    if present_source.len() > 1 {
        // Ambiguous: a change AND an issue (or both issue forms) share the
        // slug at the source. Name both candidate source paths.
        let names: Vec<String> = present_source
            .iter()
            .map(|k| {
                let (src, _dst) = candidate_paths(k);
                format!("{} ({})", src.display(), k.label())
            })
            .collect();
        return DeferLocate::Ambiguous(names.join(" AND "));
    }

    if let Some(kind) = present_source.into_iter().next() {
        return DeferLocate::NeedsMove(kind);
    }

    // Nothing at the source — check the destination for an idempotent
    // already-done. (Both issue forms checked; a change is unambiguous.)
    let present_dest: Vec<DeferKind> = forms.iter().filter(|k| at_dest(k)).cloned().collect();
    if present_dest.len() > 1 {
        let names: Vec<String> = present_dest
            .iter()
            .map(|k| {
                let (_src, dst) = candidate_paths(k);
                format!("{} ({})", dst.display(), k.label())
            })
            .collect();
        return DeferLocate::Ambiguous(names.join(" AND "));
    }
    if let Some(kind) = present_dest.into_iter().next() {
        return DeferLocate::AlreadyDone(kind);
    }
    DeferLocate::NotFound
}

/// Locate a unit named `slug` for a DEFER: it must live in a lane
/// (`openspec/changes/<slug>/` or `issues/<slug>(.md|/)`). The deferred
/// area is the destination (checked only for the idempotent
/// already-deferred no-op).
pub(crate) fn locate_for_defer(workspace: &Path, slug: &str) -> DeferLocate {
    classify_defer(
        |k| k.paths(workspace, slug).lane.exists(),
        |k| k.paths(workspace, slug).deferred.exists(),
        |k| {
            let p = k.paths(workspace, slug);
            (p.lane, p.deferred)
        },
    )
}

/// Locate a unit named `slug` for an UNDEFER: it must live in the
/// deferred area; the lane is the destination (checked only for the
/// idempotent already-active no-op).
pub(crate) fn locate_for_undefer(workspace: &Path, slug: &str) -> DeferLocate {
    classify_defer(
        |k| k.paths(workspace, slug).deferred.exists(),
        |k| k.paths(workspace, slug).lane.exists(),
        |k| {
            let p = k.paths(workspace, slug);
            // For undefer, the "source" is the deferred location and the
            // "dest" is the lane; report them in that order.
            (p.deferred, p.lane)
        },
    )
}

/// Filesystem-rename a unit from `from` to `to`, creating the
/// destination's parent directory if absent. Uses `std::fs::rename` — NOT
/// `git mv` — so the WHOLE unit (including any gitignored markers like
/// `.perma-stuck.json`, which `git mv` would orphan because it only moves
/// tracked files) travels intact. Works for both a directory unit and a
/// single-file issue.
pub(crate) fn fs_move_unit(from: &Path, to: &Path) -> std::result::Result<(), String> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating destination parent {}: {e}", parent.display()))?;
    }
    std::fs::rename(from, to).map_err(|e| {
        format!(
            "moving {} → {}: {e}",
            from.display(),
            to.display()
        )
    })
}

pub(crate) async fn handle_defer_unit(parsed: &Value, state: &ControlState) -> Value {
    handle_defer_or_undefer(parsed, state, /* defer = */ true).await
}

pub(crate) async fn handle_undefer_unit(parsed: &Value, state: &ControlState) -> Value {
    handle_defer_or_undefer(parsed, state, /* defer = */ false).await
}

/// Shared body for `defer_unit` / `undefer_unit`. `defer == true` moves a
/// unit out of its lane into the deferred area; `false` is the exact
/// inverse. Both are workspace-mutating control-socket ops, so they
/// preempt any in-flight pass AND hold the per-repo busy marker for the
/// whole move (per a01's invariant), then perform the move on a freshly
/// recreated agent branch and ride the push + PR flow honoring
/// `auto_submit_pr`. Never commits to the base branch directly.
pub(crate) async fn handle_defer_or_undefer(parsed: &Value, state: &ControlState, defer: bool) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let slug = match require_str(parsed, "slug") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let github_cfg = state.github.load_full();
    let workspace = workspace::resolve_path(&state.paths, &repo);

    let fork_url = match github_cfg.fork_owner.as_deref() {
        Some(owner) => match crate::github::derive_fork_url(&repo.url, owner) {
            Ok(u) => Some(u),
            Err(e) => return json!({"ok": false, "error": format!("deriving fork url: {e:#}")}),
        },
        None => None,
    };
    let fork_arg = fork_url.as_deref().map(|u| (u, repo.agent_branch.as_str()));

    let verb = if defer { "defer" } else { "undefer" };

    // READ-ONLY no-op detection FIRST, BEFORE any preempt or lock. An
    // already-deferred `defer` (or already-active `undefer`) is a no-op:
    // detecting it without preempting means a typo'd re-request does NOT
    // cancel an in-flight pass nor acquire the busy marker for nothing
    // (the "detect-before-preempt" invariant). We only `ensure_initialized`
    // (clone-if-absent) — NO checkout/reset/preempt/lock — and inspect the
    // current tree. An already-done request returns its no-op success here
    // with no preempt. Every other outcome (NeedsMove / NotFound /
    // Ambiguous) falls through to the live preempt + reset + re-detect path
    // below, which classifies against the freshly-reset clean base tree.
    if let Err(e) =
        workspace::ensure_initialized(&state.paths, &workspace, &repo.url, fork_arg)
    {
        return json!({"ok": false, "error": format!("workspace init failed: {e:#}")});
    }
    let preflight = if defer {
        locate_for_defer(&workspace, &slug)
    } else {
        locate_for_undefer(&workspace, &slug)
    };
    if let DeferLocate::AlreadyDone(_) = preflight {
        // Idempotent no-op success: no commit, no PR, AND — per the
        // detect-before-preempt invariant — NO preempt of any in-flight
        // pass and NO busy-marker acquire.
        let outcome = if defer { "already_deferred" } else { "already_active" };
        return json!({
            "ok": true,
            "outcome": outcome,
            "slug": slug,
            "url": repo.url,
            "preempted_change": serde_json::Value::Null,
        });
    }

    // Workspace-mutating op: preempt any in-flight pass + acquire the
    // per-repo busy marker BEFORE inspecting/mutating the tree, and hold
    // the guard for the whole op (RAII-released on every return path). On
    // `Busy`, surface a clear error and do NOT touch the workspace.
    let (_busy_guard, preempted_change) =
        match preempt_and_acquire_busy_marker(state, &repo, &workspace).await {
            Ok(outcome) => (outcome.guard, outcome.preempted_change),
            Err(PreemptAcquireError::Busy(msg)) => {
                return json!({"ok": false, "error": msg});
            }
            Err(PreemptAcquireError::Internal(msg)) => {
                return json!({"ok": false, "error": format!("{verb} preempt failed: {msg}")});
            }
        };

    // Re-initialize against the now-held marker (clone if needed; sync the
    // base branch) so the move lands on a clean base tip.
    if let Err(e) =
        workspace::ensure_initialized(&state.paths, &workspace, &repo.url, fork_arg)
    {
        return json!({"ok": false, "error": format!("workspace init failed: {e:#}")});
    }
    if let Err(e) = git::checkout(&workspace, &repo.base_branch) {
        return json!({"ok": false, "error": format!("checkout base branch failed: {e:#}")});
    }
    if let Err(e) = git::reset_hard_to_remote(&workspace, &repo.base_branch) {
        // Non-fatal in a fresh clone that already matches; log + proceed.
        tracing::warn!(
            url = %repo.url,
            "{verb}: reset --hard to origin/{} failed (proceeding with local base): {e:#}",
            repo.base_branch
        );
    }

    // Locate the unit against the now-clean base tree. (The slug is
    // resolved AFTER the reset so detection reflects the committed base
    // state, not a stale or dirty working tree.)
    let located = if defer {
        locate_for_defer(&workspace, &slug)
    } else {
        locate_for_undefer(&workspace, &slug)
    };
    let kind = match located {
        DeferLocate::NeedsMove(k) => k,
        DeferLocate::AlreadyDone(_) => {
            // Idempotent no-op success: no commit, no PR.
            let outcome = if defer { "already_deferred" } else { "already_active" };
            return json!({
                "ok": true,
                "outcome": outcome,
                "slug": slug,
                "url": repo.url,
                "preempted_change": preempted_change,
            });
        }
        DeferLocate::NotFound => {
            let msg = if defer {
                format!("no change or issue `{slug}` on {}", repo.url)
            } else {
                format!("no deferred change or issue `{slug}` on {}", repo.url)
            };
            return json!({"ok": false, "error": msg});
        }
        DeferLocate::Ambiguous(candidates) => {
            return json!({
                "ok": false,
                "error": format!(
                    "ambiguous slug `{slug}` on {}: it names more than one unit ({candidates}); \
                     resolve the collision before {verb}ring",
                    repo.url
                ),
            });
        }
    };

    let paths = kind.paths(&workspace, &slug);
    // Defer moves lane → deferred; undefer moves deferred → lane.
    let (from, to) = if defer {
        (paths.lane.clone(), paths.deferred.clone())
    } else {
        (paths.deferred.clone(), paths.lane.clone())
    };

    // Perform the move on a FRESH agent branch at the base tip, so the
    // operation rides the normal push + PR flow rather than committing to
    // the base branch directly.
    if let Err(e) = git::recreate_branch(&workspace, &repo.agent_branch) {
        return json!({"ok": false, "error": format!("recreating agent branch failed: {e:#}")});
    }
    if let Err(e) = fs_move_unit(&from, &to) {
        return json!({"ok": false, "error": e});
    }
    if let Err(e) = git::add_all(&workspace) {
        return json!({"ok": false, "error": format!("staging the move failed: {e:#}")});
    }
    let commit_msg = if defer {
        format!("chore: defer {slug}")
    } else {
        format!("chore: resume {slug}")
    };
    if let Err(e) = git::commit(&workspace, &commit_msg) {
        return json!({"ok": false, "error": format!("committing the move failed: {e:#}")});
    }

    let push_remote = if github_cfg.fork_owner.is_some() {
        "fork"
    } else {
        "origin"
    };
    if let Err(e) = git::push_force_with_lease(&workspace, &repo.agent_branch, push_remote) {
        return json!({"ok": false, "error": format!("pushing {verb} branch failed: {e:#}")});
    }

    let from_rel = from.strip_prefix(&workspace).unwrap_or(&from);
    let to_rel = to.strip_prefix(&workspace).unwrap_or(&to);
    let action_word = if defer { "Deferred" } else { "Resumed" };
    let title = if defer {
        format!("defer: set aside {} `{slug}`", kind.label())
    } else {
        format!("resume: reactivate {} `{slug}`", kind.label())
    };
    let pr_body = format!(
        "{action_word} {} `{slug}`.\n\nMoved `{}` → `{}`.\n\n\
         This rides the agent-branch + PR flow (no base-branch commit). \
         The unit is preserved byte-for-byte, including any markers; \
         {} is the exact inverse.",
        kind.label(),
        from_rel.display(),
        to_rel.display(),
        if defer { "`undefer`" } else { "`defer`" },
    );
    let outcome_word = if defer { "deferred" } else { "resumed" };

    // Honor `auto_submit_pr`: open a PR by default, OR surface the pushed
    // branch with no PR (`BranchPushedNoPr`) when an install set it false.
    if !repo.auto_submit_pr {
        let (owner, repo_name) = match crate::github::parse_repo_url(&repo.url) {
            Ok(p) => p,
            Err(e) => return json!({"ok": false, "error": format!("parsing repo url: {e:#}")}),
        };
        let branch_url = crate::polling_loop::compose_branch_url(
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
        let suggested =
            crate::polling_loop::push_only_command(repo.forge.as_ref(), pr_base, &repo.agent_branch);
        return json!({
            "ok": true,
            "outcome": outcome_word,
            "mechanism": "branch_pushed_no_pr",
            "slug": slug,
            "kind": kind.label(),
            "url": repo.url,
            "branch": repo.agent_branch,
            "branch_url": branch_url,
            "suggested_command": suggested,
            "preempted_change": preempted_change,
        });
    }

    match crate::polling_loop::open_triage_pull_request(
        &state.paths,
        &repo,
        &github_cfg,
        &repo.agent_branch,
        &repo.base_branch,
        &title,
        &pr_body,
    )
    .await
    {
        Ok(pr_url) => json!({
            "ok": true,
            "outcome": outcome_word,
            "mechanism": "pr_opened",
            "slug": slug,
            "kind": kind.label(),
            "url": repo.url,
            "branch": repo.agent_branch,
            "pr_url": pr_url,
            "preempted_change": preempted_change,
        }),
        Err(e) => json!({"ok": false, "error": format!("opening {verb} PR: {e:#}")}),
    }
}

/// Handle the `query_canonical_specs` action (a21). Looks up the
/// workspace's `CanonicalRagStore` in the daemon's registry; on hit,
/// runs the query and returns ranked chunks. Every error path is
/// fail-open: an `ok: true` response with an empty `hits` array and a
/// structured `error_hint`. Protocol-level violations (missing
/// `workspace_basename` or `query`) return `ok: false` per the canonical
/// request-protocol scenario.
pub(crate) async fn handle_query_canonical_specs(parsed: &Value, state: &ControlState) -> Value {
    let workspace_basename = match require_str(parsed, "workspace_basename") {
        Ok(s) => s,
        Err(e) => {
            return json!({"ok": false, "error": format!("missing required field: workspace_basename ({e})")});
        }
    };
    let query = match require_str(parsed, "query") {
        Ok(s) => s,
        Err(e) => {
            return json!({"ok": false, "error": format!("missing required field: query ({e})")});
        }
    };
    let top_k = parsed
        .get("top_k")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    let cfg = state.last_config.load_full();
    if cfg
        .canonical_rag
        .as_ref()
        .map(|r| !r.is_active())
        .unwrap_or(true)
    {
        return json!({
            "ok": true,
            "hits": [],
            "error_hint": "rag disabled in config",
        });
    }

    let store = match state
        .canonical_rag_registry
        .get(&workspace_basename)
        .await
    {
        Some(s) => s,
        None => {
            // Distinguish the two empty-registry cases: a known
            // workspace whose init failed (config has the block) vs.
            // a basename the daemon doesn't manage.
            let cfg_active = cfg
                .canonical_rag
                .as_ref()
                .map(|r| r.is_active())
                .unwrap_or(false);
            let hint = if cfg_active {
                "rag init failed; see daemon log"
            } else {
                "no workspace registered for that basename"
            };
            return json!({
                "ok": true,
                "hits": [],
                "error_hint": hint,
            });
        }
    };

    match store.query(&query, top_k).await {
        Ok(hits) => json!({
            "ok": true,
            "hits": hits,
        }),
        Err(e) => {
            tracing::warn!(
                workspace_basename = %workspace_basename,
                "canonical RAG query failed: {e:#}"
            );
            json!({
                "ok": true,
                "hits": [],
                "error_hint": format!("query failed: {e}"),
            })
        }
    }
}

/// Handle the `record_outcome` action (a27a0). Trusts the relayed
/// payload (the per-execution MCP child has already validated it) and
/// writes it into the daemon's outcome store. Returns `{"ok":true}` on
/// success; returns `{"ok":false,"error":...}` on a malformed payload
/// shape (missing key fields, unknown variant tag, wrong types). The
/// failure case exists to surface programmer error during new-client
/// development, NOT to enforce business rules — schema validation is
/// the MCP layer's responsibility.
pub(crate) fn handle_record_outcome(parsed: &Value, state: &ControlState) -> Value {
    let workspace_basename = match require_str(parsed, "workspace_basename") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let change = match require_str(parsed, "change") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let outcome_val = match parsed.get("outcome") {
        Some(v) => v.clone(),
        None => return json!({"ok": false, "error": "missing `outcome` field"}),
    };
    // Probe the variant tag for a clearer error than serde's default
    // "unknown variant" message.
    let variant_tag = outcome_val
        .get("type")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    match variant_tag.as_deref() {
        Some("success") | Some("spec_needs_revision") | Some("iteration_request") => {}
        Some(other) => {
            return json!({
                "ok": false,
                "error": format!("unknown outcome variant tag `{other}`"),
            });
        }
        None => {
            return json!({
                "ok": false,
                "error": "outcome is missing required string `type` discriminator",
            });
        }
    }
    let recorded: crate::outcome_store::RecordedOutcome =
        match serde_json::from_value(outcome_val) {
            Ok(r) => r,
            Err(e) => {
                return json!({
                    "ok": false,
                    "error": format!("malformed outcome payload: {e}"),
                });
            }
        };
    tracing::info!(
        workspace_basename = %workspace_basename,
        change = %change,
        variant = ?variant_tag,
        "outcome recorded via outcome tool"
    );
    state
        .outcome_store
        .record(workspace_basename, change, recorded);
    json!({"ok": true})
}

/// Handle the `consume_outcome` action (a27a0). Atomically reads AND
/// removes the entry for `(workspace_basename, change)`. Returns
/// `{"ok":true,"outcome":null}` when no entry exists.
pub(crate) fn handle_consume_outcome(parsed: &Value, state: &ControlState) -> Value {
    let workspace_basename = match require_str(parsed, "workspace_basename") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let change = match require_str(parsed, "change") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let outcome = state.outcome_store.consume(&workspace_basename, &change);
    let outcome_value = match outcome {
        Some(o) => serde_json::to_value(&o).unwrap_or(Value::Null),
        None => Value::Null,
    };
    json!({"ok": true, "outcome": outcome_value})
}

/// Handle the `record_submission` action (a56). Accepts a
/// `workspace_basename` routing key, a `change`/execution key, a `role`
/// name, AND a `payload`; validates the payload against the role's
/// registered schema (a no-op until a later change registers one) AND
/// stores it keyed by execution. Returns `{"ok":true}` on success OR
/// `{"ok":false,"error":<reason>}` on a schema/validation failure (which
/// the MCP relay surfaces to the agent as a correctable tool error).
/// Parallels [`handle_record_outcome`].
pub(crate) fn handle_record_submission(parsed: &Value, state: &ControlState) -> Value {
    let workspace_basename = match require_str(parsed, "workspace_basename") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let change = match require_str(parsed, "change") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let role = match require_str(parsed, "role") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let payload = match parsed.get("payload") {
        Some(v) => v.clone(),
        None => return json!({"ok": false, "error": "missing `payload` field"}),
    };
    match state
        .submission_store
        .record(workspace_basename.clone(), change.clone(), &role, payload)
    {
        Ok(()) => {
            tracing::info!(
                workspace_basename = %workspace_basename,
                change = %change,
                role = %role,
                "submission recorded via submit tool"
            );
            json!({"ok": true})
        }
        Err(reason) => json!({"ok": false, "error": reason}),
    }
}

/// Handle the `consume_submission` action (a56). Atomically reads AND
/// removes the stored submission for `(workspace_basename, change)`.
/// Returns `{"ok":true,"submission":null}` when no entry exists (absence
/// is "the role did not submit", not an error). Parallels
/// [`handle_consume_outcome`].
pub(crate) fn handle_consume_submission(parsed: &Value, state: &ControlState) -> Value {
    let workspace_basename = match require_str(parsed, "workspace_basename") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let change = match require_str(parsed, "change") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let submission = state
        .submission_store
        .consume(&workspace_basename, &change)
        .unwrap_or(Value::Null);
    json!({"ok": true, "submission": submission})
}

/// Read the daemon's config path, parse + validate, diff against the
/// last-applied snapshot, hot-apply safe sections, and return the result.
pub async fn handle_reload(state: &ControlState) -> Value {
    let path = &state.config_path;
    let new_cfg = match Config::load_from(path) {
        Ok(c) => c,
        Err(e) => {
            return json!({
                "ok": false,
                "error": format!("config file {}: {e:#}", path.display()),
            });
        }
    };
    if let Err(e) = crate::workspace::detect_collisions(&state.paths, &new_cfg.repositories) {
        return json!({"ok": false, "error": format!("{e:#}")});
    }
    if let Err(e) = crate::cli::run::validate_github_token_routes(
        &new_cfg.github,
        &new_cfg.repositories,
    ) {
        return json!({"ok": false, "error": format!("{e:#}")});
    }

    let current = state.last_config.load_full();

    let mut applied: Vec<String> = Vec::new();
    let mut unchanged: Vec<String> = Vec::new();
    let mut requires_restart: Vec<String> = Vec::new();
    let mut section_errors: Vec<(String, String)> = Vec::new();

    // --- github ---
    if yaml_repr(&current.github) != yaml_repr(&new_cfg.github) {
        state.github.store(Arc::new(new_cfg.github.clone()));
        applied.push("github".to_string());
    } else {
        unchanged.push("github".to_string());
    }

    // --- reviewer ---
    if yaml_repr(&current.reviewer) != yaml_repr(&new_cfg.reviewer) {
        match build_reviewer(
            new_cfg.reviewer.as_ref(),
            new_cfg.executor.agentic_session_timeout(),
        ) {
            Ok(slot) => {
                state.reviewer.store(Arc::new(slot));
                applied.push("reviewer".to_string());
            }
            Err(e) => {
                tracing::error!("reload: reviewer reconstruction failed: {e:#}");
                section_errors.push(("reviewer".to_string(), format!("{e:#}")));
            }
        }
    } else {
        unchanged.push("reviewer".to_string());
    }

    // --- chatops ---
    if yaml_repr(&current.chatops) != yaml_repr(&new_cfg.chatops) {
        match build_chatops_slot(new_cfg.chatops.as_ref()).await {
            Ok(slot) => {
                state.chatops.store(Arc::new(slot));
                applied.push("chatops".to_string());
            }
            Err(e) => {
                tracing::error!("reload: chatops reconstruction failed: {e:#}");
                section_errors.push(("chatops".to_string(), format!("{e:#}")));
            }
        }
    } else {
        unchanged.push("chatops".to_string());
    }

    // --- cache (hot-applied) ---
    // a65: swap the workspace-cache config so the new `workspaces_max_gb`
    // cap takes effect at the next iteration of every polling task.
    if yaml_repr(&current.cache) != yaml_repr(&new_cfg.cache) {
        state.cache.store(Arc::new(new_cfg.cache.clone()));
        applied.push("cache".to_string());
    } else {
        unchanged.push("cache".to_string());
    }

    // --- repositories (hot-applied) ---
    // Diff by URL: added/removed are computed from the URL sets; for URLs
    // present in both, compare the full RepositoryConfig (URLs already
    // match, so any difference is in another field).
    let delta = apply_repository_changes(state, &new_cfg.repositories);
    if delta.added.is_empty() && delta.removed.is_empty() && delta.changed.is_empty() {
        unchanged.push("repositories".to_string());
    } else {
        applied.push("repositories".to_string());
    }

    // --- executor (restart-required) ---
    if yaml_repr(&current.executor) != yaml_repr(&new_cfg.executor) {
        requires_restart.push("executor".to_string());
    } else {
        unchanged.push("executor".to_string());
    }

    // Persist the new config snapshot so the next reload diffs against
    // current state.
    state.last_config.store(Arc::new(new_cfg));

    let mut resp = json!({
        "ok": true,
        "applied": applied,
        "requires_restart": requires_restart,
        "unchanged": unchanged,
        "repositories_delta": {
            "added": delta.added,
            "removed": delta.removed,
            "changed": delta.changed,
        },
    });
    if !section_errors.is_empty() {
        let mut errors = serde_json::Map::new();
        for (section, msg) in section_errors {
            errors.insert(section, Value::String(msg));
        }
        resp.as_object_mut()
            .unwrap()
            .insert("section_errors".to_string(), Value::Object(errors));
    }
    resp
}

#[derive(Default, Debug, Clone)]
struct RepositoriesDelta {
    added: Vec<String>,
    removed: Vec<String>,
    changed: Vec<String>,
}

/// Apply the repositories-section diff against the live task map.
/// Returns the set of URLs added, removed, and changed-in-place.
///
/// Semantics:
///   - URL in current but not new → `removed`: cancel the per-repo
///     token. The task's exit path removes the map entry.
///   - URL in new but not current → `added`: spawn a fresh polling task.
///     If the URL is somehow still in the map (transient state from a
///     recently-cancelled task that hasn't exited), log WARN and treat
///     as `unchanged` for the response — the next reload (after the
///     in-flight iteration completes) will pick it up cleanly.
///   - URL in both → compare configs (URLs already match, so any
///     difference is in another field). If different AND the existing
///     handle's token is NOT already cancelled, swap the live config
///     holder via `store(Arc::new(new))` so the next iteration picks up
///     the new values. If the existing handle's token IS already
///     cancelled (transient mid-shutdown state), log WARN and skip —
///     report as `unchanged`.
fn apply_repository_changes(
    state: &ControlState,
    new_repos: &[RepositoryConfig],
) -> RepositoriesDelta {
    let mut delta = RepositoriesDelta::default();
    let new_by_url: HashMap<String, &RepositoryConfig> =
        new_repos.iter().map(|r| (r.url.clone(), r)).collect();
    let new_urls: HashSet<String> = new_by_url.keys().cloned().collect();

    // Snapshot current URLs (+ a structural fingerprint per URL for the
    // change-in-place diff). We do this under the lock, then drop the
    // lock before any spawn calls so the spawn closure can re-take it.
    let current_state: Vec<(String, bool, Arc<RepositoryConfig>)> = {
        let guard = state.repo_tasks.lock().unwrap();
        guard
            .iter()
            .map(|(url, handle)| {
                (
                    url.clone(),
                    handle.cancel.is_cancelled(),
                    handle.config.load_full(),
                )
            })
            .collect()
    };
    let current_urls: HashSet<String> =
        current_state.iter().map(|(u, _, _)| u.clone()).collect();

    // 1. Removed: cancel the existing per-repo token. The task exit
    //    path removes the map entry; we do NOT remove it here.
    let mut removed_sorted: Vec<&String> = current_urls.difference(&new_urls).collect();
    removed_sorted.sort();
    for url in removed_sorted {
        let cancel_token = {
            let guard = state.repo_tasks.lock().unwrap();
            guard.get(url).map(|h| h.cancel.clone())
        };
        if let Some(token) = cancel_token {
            tracing::info!(url = %url, "reload: cancelling polling task for removed repository");
            token.cancel();
            delta.removed.push(url.clone());
        }
    }

    // 2. Changed in place: URL still present, other fields differ.
    //    Skip with a WARN if the existing handle's token is already
    //    cancelled (transient mid-shutdown state).
    let mut existing_sorted: Vec<&String> = new_urls.intersection(&current_urls).collect();
    existing_sorted.sort();
    for url in existing_sorted {
        let (_, was_cancelled, current_cfg) = current_state
            .iter()
            .find(|(u, _, _)| *u == **url)
            .cloned()
            .expect("URL came from current_urls intersected with new_urls");
        let new_cfg = new_by_url
            .get(url)
            .copied()
            .expect("URL came from new_urls intersection");
        if yaml_repr(current_cfg.as_ref()) == yaml_repr(new_cfg) {
            // No structural difference. Nothing to do.
            continue;
        }
        if was_cancelled {
            tracing::warn!(
                url = %url,
                "reload: repository is still in the task map but its per-repo cancellation token is set; \
                 in-flight iteration is shutting down — skipping hot-swap on this reload, \
                 retry after the task exits"
            );
            continue;
        }
        // Take the lock just long enough to issue the store. If the
        // task exited between our snapshot and the store, the swap is
        // harmless (the holder's Arc strong references will drop with
        // the task).
        let guard = state.repo_tasks.lock().unwrap();
        if let Some(handle) = guard.get(url) {
            handle.config.store(Arc::new(new_cfg.clone()));
            tracing::info!(url = %url, "reload: hot-swapped repository config");
            delta.changed.push(url.clone());
        }
    }

    // 3. Added: spawn a new task per URL. If the URL is already in the
    //    map (e.g. mid-shutdown of a previously-cancelled task), log
    //    WARN and skip — count as unchanged in the response.
    let mut added_sorted: Vec<&String> = new_urls.difference(&current_urls).collect();
    added_sorted.sort();
    for url in added_sorted {
        let new_cfg = new_by_url
            .get(url)
            .copied()
            .expect("URL came from new_urls difference")
            .clone();
        match (state.spawn_repo)(new_cfg) {
            SpawnOutcome::Spawned => {
                tracing::info!(url = %url, "reload: spawned polling task for added repository");
                delta.added.push(url.clone());
            }
            SpawnOutcome::AlreadyPresent => {
                tracing::warn!(
                    url = %url,
                    "reload: repository is in the new config but already present in the task map; \
                     skipping spawn — likely a transient mid-shutdown state, retry after the prior task exits"
                );
            }
            SpawnOutcome::StartupCheckFailed => {
                tracing::error!(
                    url = %url,
                    "reload: repository startup check failed; not spawning a polling task — \
                     edit YAML and reload again after fixing the workspace"
                );
            }
        }
    }

    delta
}

fn build_reviewer(
    cfg: Option<&ReviewerConfig>,
    agentic_session_timeout: std::time::Duration,
) -> Result<Option<Arc<CodeReviewer>>> {
    match cfg {
        Some(rcfg) if rcfg.enabled => {
            let r = CodeReviewer::from_config(rcfg)
                .context("initializing code reviewer from new config")?
                // Reviewer shares the ONE auxiliary-session timeout with the
                // verifier gates AND the revision sessions; the reviewer block
                // does not carry the executor field, so it is threaded here.
                .with_agentic_session_timeout(agentic_session_timeout);
            // a64: re-evaluate agentic-CLI availability on reload via the
            // existing `reviewer:` hot-reload path; an unavailable CLI
            // degrades to `oneshot` for this boot with one loud WARN.
            let r = crate::code_reviewer::apply_startup_cli_fallback(r);
            Ok(Some(Arc::new(r)))
        }
        _ => Ok(None),
    }
}

async fn build_chatops_slot(cfg: Option<&ChatOpsConfig>) -> Result<Option<ChatOpsSlot>> {
    let Some(co) = cfg else { return Ok(None) };
    let backend = crate::chatops::from_config(co)
        .await
        .context("initializing chatops backend from new config")?;
    Ok(Some(ChatOpsSlot {
        backend,
        default_channel_id: co.default_channel_id.clone(),
        start_work_enabled: NotificationsConfig::start_work_enabled(Some(co)),
        failure_alerts_enabled: NotificationsConfig::failure_alerts_enabled(Some(co)),
        pr_opened_enabled: NotificationsConfig::pr_opened_enabled(Some(co)),
    }))
}

/// Structural-equality diff via YAML serialization. Catches changes to
/// nested values (e.g. `SecretSource`) that raw equality would miss.
fn yaml_repr<T: serde::Serialize>(value: &T) -> String {
    serde_yml::to_string(value).unwrap_or_default()
}
