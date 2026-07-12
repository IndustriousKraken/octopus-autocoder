//! The dedicated, always-on discuss handler
//! (discuss-verb-conversational-propose).
//!
//! Unlike the per-repo polling tasks, this is a single daemon-global task fed
//! by an `mpsc` channel from the control-socket dispatcher, so a `discuss`
//! message is processed immediately — no polling-loop sleep. It:
//!
//!   - `Start`: runs a read-only agentic session with the discuss-mode prompt,
//!     posts the reply to the thread, and persists the session id so
//!     continuation can resume it.
//!   - `Continue`: resumes the session with the new message; posts the reply.
//!   - `SendIt`: waits for any running per-repo executor to finish (per-repo
//!     sequential), resumes the session in write mode, commits the artifact,
//!     opens a PR on the agent branch, and posts the PR URL.
//!
//! It also runs a periodic sweep: a once-per-stale-discussion 7-day idle
//! reminder for deferred discussions, and a 14-day prune of `DiscussionState`
//! files.

use crate::config::{GithubConfig, RepositoryConfig};
use crate::control_socket::{
    ChatOpsHolder, DiscussContinue, DiscussEvent, DiscussSendIt, DiscussStart, GithubHolder,
    RepoTaskMap,
};
use crate::discussion_state::{self, DiscussionState, DiscussionStatus};
use crate::executor::{DiscussContext, Executor};
use crate::paths::DaemonPaths;
use crate::prompts::{PromptId, PromptLoader, render_template};
use chrono::Utc;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

/// Everything the discuss handler needs, captured once at daemon startup.
#[derive(Clone)]
pub struct DiscussHandlerDeps {
    pub paths: Arc<DaemonPaths>,
    pub executor: Arc<dyn Executor>,
    pub chatops: ChatOpsHolder,
    pub github: GithubHolder,
    /// Live per-repo task registry, used to resolve the current
    /// `RepositoryConfig` (workspace, agent branch) for a URL.
    pub repo_tasks: RepoTaskMap,
}

/// How often the idle-reminder + prune sweep runs. Hourly is ample for a
/// 7-day / 14-day cadence and cheap.
const SWEEP_INTERVAL: Duration = Duration::from_secs(3_600);

/// The main handler loop. Runs until `cancel` fires.
pub async fn run_discuss_handler(
    mut rx: UnboundedReceiver<DiscussEvent>,
    deps: DiscussHandlerDeps,
    cancel: CancellationToken,
) {
    tracing::info!("discuss handler task started");
    let mut sweep = tokio::time::interval(SWEEP_INTERVAL);
    // Skip the immediate first tick so startup doesn't post reminders before
    // any discussion exists.
    sweep.tick().await;
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                tracing::info!("discuss handler task stopping (cancelled)");
                return;
            }
            maybe = rx.recv() => {
                match maybe {
                    Some(event) => process_event(&deps, event).await,
                    None => {
                        tracing::info!("discuss handler channel closed; stopping");
                        return;
                    }
                }
            }
            _ = sweep.tick() => {
                run_sweep(&deps).await;
            }
        }
    }
}

async fn process_event(deps: &DiscussHandlerDeps, event: DiscussEvent) {
    let result = match event {
        DiscussEvent::Start(s) => process_start(deps, s).await,
        DiscussEvent::Continue(c) => process_continue(deps, c).await,
        DiscussEvent::SendIt(si) => process_send_it(deps, si).await,
    };
    if let Err(e) = result {
        tracing::warn!("discuss handler: event processing failed: {e:#}");
    }
}

/// Resolve the live `RepositoryConfig` for `url` from the task registry.
fn resolve_repo(deps: &DiscussHandlerDeps, url: &str) -> Option<RepositoryConfig> {
    let guard = deps.repo_tasks.lock().unwrap();
    guard.get(url).map(|h| (*h.config.load_full()).clone())
}

/// Post a threaded reply best-effort; a missing/failed backend is logged.
async fn post_reply(deps: &DiscussHandlerDeps, channel: &str, thread_ts: &str, body: &str) {
    let slot = deps.chatops.load_full();
    let Some(slot) = slot.as_ref() else {
        tracing::warn!("discuss handler: no chatops backend; dropping reply");
        return;
    };
    if let Err(e) = slot
        .backend
        .post_threaded_reply(channel, thread_ts, body)
        .await
    {
        tracing::warn!("discuss handler: post_threaded_reply failed: {e:#}");
    }
}

/// Render the discuss-mode prompt for one turn.
fn render_prompt(repo_url: &str, message: &str, write_mode: bool) -> String {
    let template = PromptLoader::load(PromptId::DiscussMode, None, None, None);
    let mode = if write_mode { "send-it" } else { "discuss" };
    render_template(
        &template,
        &[("repo_url", repo_url), ("mode", mode), ("message", message)],
    )
}

async fn process_start(deps: &DiscussHandlerDeps, start: DiscussStart) -> anyhow::Result<()> {
    let repo = match resolve_repo(deps, &start.repo_url) {
        Some(r) => r,
        None => {
            post_reply(
                deps,
                &start.channel,
                &start.thread_ts,
                &format!("✗ discuss: no live polling task for `{}`.", start.repo_url),
            )
            .await;
            return Ok(());
        }
    };
    // Ensure the workspace exists (clone-if-absent). We do NOT reset or switch
    // branches: discussion is read-only and MUST NOT disturb a concurrently
    // running executor's working tree.
    let workspace = crate::workspace::resolve_path(&deps.paths, &repo);
    ensure_workspace(deps, &repo, &workspace);

    let prompt = render_prompt(&start.repo_url, &start.initial_text, false);
    let ctx = DiscussContext {
        rendered_prompt: prompt,
        resume_session_id: None,
        write_mode: false,
    };
    let turn = match deps.executor.run_discuss(&workspace, &ctx).await {
        Ok(t) => t,
        Err(e) => {
            post_reply(
                deps,
                &start.channel,
                &start.thread_ts,
                &format!("✗ discuss: the session failed to start: {e}"),
            )
            .await;
            return Ok(());
        }
    };

    // Update state: session id + activity, and handle any deferral signal.
    let state_root = discussion_state::default_state_root(&deps.paths);
    let mut state = discussion_state::read_state(&state_root, &start.thread_ts)
        .ok()
        .flatten()
        .unwrap_or_else(|| new_state_from_start(&start));
    state.session_id = turn.session_id.clone();
    state.last_activity_at = Utc::now();
    state.reminded_at = None;

    let (clean_reply, deferred) = extract_defer_signal(&turn.reply);
    if let Some(slug) = deferred {
        maybe_auto_defer(deps, &repo, &workspace, &mut state, &start.channel, &start.thread_ts, &slug)
            .await;
    }
    let _ = discussion_state::write_state(&state_root, &state);
    post_reply(deps, &start.channel, &start.thread_ts, &clean_reply).await;
    Ok(())
}

async fn process_continue(deps: &DiscussHandlerDeps, cont: DiscussContinue) -> anyhow::Result<()> {
    tracing::debug!(
        request_id = %cont.request_id,
        thread_ts = %cont.thread_ts,
        "discuss handler: continuation turn"
    );
    let state_root = discussion_state::default_state_root(&deps.paths);
    let mut state = match discussion_state::read_state(&state_root, &cont.thread_ts).ok().flatten() {
        Some(s) if s.status == DiscussionStatus::Active => s,
        _ => {
            post_reply(
                deps,
                &cont.channel,
                &cont.thread_ts,
                "✗ This discussion is no longer active. Start a new one with @<bot> discuss <repo> <text>.",
            )
            .await;
            return Ok(());
        }
    };
    let repo = match resolve_repo(deps, &cont.repo_url) {
        Some(r) => r,
        None => {
            post_reply(
                deps,
                &cont.channel,
                &cont.thread_ts,
                &format!("✗ discuss: no live polling task for `{}`.", cont.repo_url),
            )
            .await;
            return Ok(());
        }
    };
    let workspace = crate::workspace::resolve_path(&deps.paths, &repo);
    ensure_workspace(deps, &repo, &workspace);

    let prompt = render_prompt(&cont.repo_url, &cont.text, false);
    let ctx = DiscussContext {
        rendered_prompt: prompt,
        resume_session_id: state.session_id.clone(),
        write_mode: false,
    };
    let turn = match deps.executor.run_discuss(&workspace, &ctx).await {
        Ok(t) => t,
        Err(e) => {
            post_reply(
                deps,
                &cont.channel,
                &cont.thread_ts,
                &format!("✗ discuss: the session failed to resume: {e}"),
            )
            .await;
            return Ok(());
        }
    };
    if turn.session_id.is_some() {
        state.session_id = turn.session_id.clone();
    }
    state.last_activity_at = Utc::now();
    state.reminded_at = None;
    let (clean_reply, deferred) = extract_defer_signal(&turn.reply);
    if let Some(slug) = deferred
        && state.deferred_slug.is_none()
    {
        maybe_auto_defer(deps, &repo, &workspace, &mut state, &cont.channel, &cont.thread_ts, &slug)
            .await;
    }
    let _ = discussion_state::write_state(&state_root, &state);
    post_reply(deps, &cont.channel, &cont.thread_ts, &clean_reply).await;
    Ok(())
}

async fn process_send_it(deps: &DiscussHandlerDeps, si: DiscussSendIt) -> anyhow::Result<()> {
    let state_root = discussion_state::default_state_root(&deps.paths);
    // Defense-in-depth: only an Active discussion can be sent. The dispatcher
    // already refuses non-Active `send it`, but a second SendIt event (retry,
    // manual control-socket call, future dispatcher bug) must NOT re-run the
    // agent and open a duplicate PR. Mirrors the guard in `process_continue`.
    let mut state = match discussion_state::read_state(&state_root, &si.thread_ts).ok().flatten() {
        Some(s) if s.status == DiscussionStatus::Active => s,
        _ => {
            post_reply(
                deps,
                &si.channel,
                &si.thread_ts,
                "✗ This discussion is no longer active. Start a new one with @<bot> discuss <repo> <text>.",
            )
            .await;
            return Ok(());
        }
    };
    let repo = match resolve_repo(deps, &si.repo_url) {
        Some(r) => r,
        None => {
            post_reply(
                deps,
                &si.channel,
                &si.thread_ts,
                &format!("✗ discuss: no live polling task for `{}`.", si.repo_url),
            )
            .await;
            return Ok(());
        }
    };
    // Transition to Executing so the send-it is not double-processed.
    state.status = DiscussionStatus::Executing;
    state.last_activity_at = Utc::now();
    let _ = discussion_state::write_state(&state_root, &state);

    let workspace = crate::workspace::resolve_path(&deps.paths, &repo);
    ensure_workspace(deps, &repo, &workspace);

    // Sequential with the per-repo implementation executor: acquire the busy
    // marker so no executor pass runs while we create + commit the artifact on
    // the shared agent branch. Wait (with a cap) for a running pass to finish.
    let _busy = match acquire_busy_when_free(deps, &repo, &workspace).await {
        Some(g) => g,
        None => {
            post_reply(
                deps,
                &si.channel,
                &si.thread_ts,
                "✗ discuss: timed out waiting for the running executor to finish; try `send it` again.",
            )
            .await;
            state.status = DiscussionStatus::Active;
            let _ = discussion_state::write_state(&state_root, &state);
            return Ok(());
        }
    };

    // Prepare the agent branch (we now hold the busy marker, so this is safe).
    if let Err(e) = prepare_agent_branch(&repo, &workspace) {
        tracing::warn!("discuss send-it: agent-branch prep failed: {e:#}");
    }

    let mut message = state.initial_text.clone();
    if let Some(fc) = si.final_context.as_deref() {
        message.push_str("\n\nFinal context from the operator:\n");
        message.push_str(fc);
    }
    let prompt = render_prompt(&si.repo_url, &message, true);
    let ctx = DiscussContext {
        rendered_prompt: prompt,
        resume_session_id: state.session_id.clone(),
        write_mode: true,
    };
    let turn = match deps.executor.run_discuss(&workspace, &ctx).await {
        Ok(t) => t,
        Err(e) => {
            post_reply(
                deps,
                &si.channel,
                &si.thread_ts,
                &format!("✗ discuss: artifact creation failed: {e}"),
            )
            .await;
            state.status = DiscussionStatus::Active;
            let _ = discussion_state::write_state(&state_root, &state);
            return Ok(());
        }
    };

    // Commit any artifact the agent produced AND open a PR.
    let github = (*deps.github.load_full()).clone();
    match commit_and_open_pr(&repo, &github, &workspace, &si.request_id).await {
        Ok(Some(pr_url)) => {
            // Clear any auto-defer now that the change has landed as a PR.
            if let Some(slug) = state.deferred_slug.take() {
                clear_auto_defer(&repo, &workspace, &slug);
            }
            state.status = DiscussionStatus::Completed;
            state.last_activity_at = Utc::now();
            let _ = discussion_state::write_state(&state_root, &state);
            post_reply(
                deps,
                &si.channel,
                &si.thread_ts,
                &format!("✅ Artifact PR opened: {pr_url}\nReview + merge it to apply the change."),
            )
            .await;
        }
        Ok(None) => {
            state.status = DiscussionStatus::Active;
            let _ = discussion_state::write_state(&state_root, &state);
            let note = turn.reply.trim();
            let body = if note.is_empty() {
                "ℹ️ The discuss agent produced no artifact to commit. Nothing was opened.".to_string()
            } else {
                format!("ℹ️ The discuss agent produced no artifact to commit:\n\n{note}")
            };
            post_reply(deps, &si.channel, &si.thread_ts, &body).await;
        }
        Err(e) => {
            state.status = DiscussionStatus::Active;
            let _ = discussion_state::write_state(&state_root, &state);
            post_reply(
                deps,
                &si.channel,
                &si.thread_ts,
                &format!("✗ discuss: could not open the artifact PR: {e}"),
            )
            .await;
        }
    }
    Ok(())
}

fn new_state_from_start(start: &DiscussStart) -> DiscussionState {
    let now = Utc::now();
    DiscussionState {
        thread_ts: start.thread_ts.clone(),
        channel: start.channel.clone(),
        repo_url: start.repo_url.clone(),
        request_id: start.request_id.clone(),
        operator_user: start.operator_user.clone(),
        initial_text: start.initial_text.clone(),
        status: DiscussionStatus::Active,
        session_id: None,
        deferred_slug: None,
        reminded_at: None,
        created_at: now,
        last_activity_at: now,
    }
}

/// Ensure the workspace directory exists (clone-if-absent). Best-effort — a
/// failure is logged; the session will simply see whatever is on disk.
fn ensure_workspace(deps: &DiscussHandlerDeps, repo: &RepositoryConfig, workspace: &Path) {
    if workspace.join(".git").is_dir() {
        return;
    }
    let github = (*deps.github.load_full()).clone();
    let fork_arg = github
        .fork_owner
        .as_deref()
        .and_then(|owner| crate::github::derive_fork_url(&repo.url, owner).ok())
        .map(|u| (u, repo.agent_branch.clone()));
    let fork_ref = fork_arg.as_ref().map(|(u, b)| (u.as_str(), b.as_str()));
    if let Err(e) = crate::workspace::ensure_initialized(&deps.paths, workspace, &repo.url, fork_ref)
    {
        tracing::warn!("discuss handler: workspace ensure_initialized failed: {e:#}");
    }
}

/// Extract the first `DISCUSS-DEFER: <slug>` signal line from an agent reply,
/// returning the reply with that line removed AND the slug (if any). The slug
/// must match the change/issue slug shape.
fn extract_defer_signal(reply: &str) -> (String, Option<String>) {
    let mut slug = None;
    let mut kept = Vec::new();
    for line in reply.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("DISCUSS-DEFER:") {
            let candidate = rest.trim().trim_matches('`').trim();
            if slug.is_none() && is_valid_slug(candidate) {
                slug = Some(candidate.to_string());
                continue; // drop the signal line from the posted reply
            }
        }
        kept.push(line);
    }
    (kept.join("\n").trim().to_string(), slug)
}

fn is_valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 120
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && s.starts_with(|c: char| c.is_ascii_lowercase())
}

/// Auto-defer an existing spec/change under active discussion: move it out of
/// its lane on the base branch, commit, and post the undefer reminder. Records
/// `deferred_slug` on the state. Best-effort AND guarded so it never disturbs a
/// running executor (it acquires the busy marker; if the repo is busy it skips
/// the move but still records the intent + posts the reminder).
///
// ponytail: base-branch commit guarded by the busy marker; a full preempt (like
// the `undefer` chatops verb) is heavier than warranted for a discussion nudge.
async fn maybe_auto_defer(
    deps: &DiscussHandlerDeps,
    repo: &RepositoryConfig,
    workspace: &Path,
    state: &mut DiscussionState,
    channel: &str,
    thread_ts: &str,
    slug: &str,
) {
    if state.deferred_slug.as_deref() == Some(slug) {
        return; // already deferred this unit
    }
    match crate::busy_marker::try_acquire(&deps.paths, workspace, &repo.url, 3_600) {
        Ok(crate::busy_marker::AcquireOutcome::Acquired(_guard)) => {
            if let Err(e) = defer_unit_on_base(repo, workspace, slug, true) {
                tracing::warn!("discuss auto-defer: move failed: {e:#}");
            }
        }
        _ => {
            tracing::info!(
                slug = slug,
                "discuss auto-defer: repo busy; recording deferral intent without the move"
            );
        }
    }
    state.deferred_slug = Some(slug.to_string());
    let body = format!(
        "I've deferred `{slug}` while we discuss. If you decide not to follow \
         through, clear it with @<bot> undefer {repo} {slug}. I'll clear it \
         automatically when a PR lands.",
        repo = repo.url,
    );
    post_reply(deps, channel, thread_ts, &body).await;
}

/// Clear an auto-defer by moving the unit back into its lane on the base
/// branch and committing. Best-effort.
fn clear_auto_defer(repo: &RepositoryConfig, workspace: &Path, slug: &str) {
    if let Err(e) = defer_unit_on_base(repo, workspace, slug, false) {
        tracing::warn!("discuss clear-defer: move-back failed: {e:#}");
    }
}

/// Move a change/issue between its lane and `deferred-*/`, then commit on the
/// base branch. `defer == true` moves lane → deferred; `false` moves back.
fn defer_unit_on_base(
    repo: &RepositoryConfig,
    workspace: &Path,
    slug: &str,
    defer: bool,
) -> anyhow::Result<()> {
    let change_lane = workspace.join("openspec/changes").join(slug);
    let change_deferred = workspace.join("deferred-changes").join(slug);
    let (from, to) = if defer {
        (change_lane, change_deferred)
    } else {
        (change_deferred, change_lane)
    };
    if !from.exists() {
        // Nothing to move (already in the target state, or a spec-requirement
        // deferral we don't model as a dir). Not an error.
        return Ok(());
    }
    let _ = crate::git::checkout(workspace, &repo.base_branch);
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&from, &to)?;
    crate::git::add_all(workspace)?;
    let subject = if defer {
        format!("chore: defer {slug}")
    } else {
        format!("chore: resume {slug}")
    };
    crate::git::commit(workspace, &subject)?;
    Ok(())
}

/// Prepare a fresh agent branch off the current base tip for the artifact
/// commit. Caller must already hold the busy marker.
fn prepare_agent_branch(repo: &RepositoryConfig, workspace: &Path) -> anyhow::Result<()> {
    let _ = crate::queue::clear_stale_locks(workspace);
    let _ = crate::git::reset_hard_head(workspace);
    let _ = crate::git::clean_force(workspace);
    let _ = crate::git::fetch(workspace);
    crate::git::checkout(workspace, &repo.base_branch)?;
    let _ = crate::git::pull_ff_only(workspace, &repo.base_branch);
    crate::git::recreate_branch(workspace, &repo.agent_branch)?;
    Ok(())
}

/// Stage every change under the workspace, commit if there is anything, push,
/// and open a PR on the agent branch. Returns `Ok(Some(url))` on a PR,
/// `Ok(None)` when there was nothing to commit.
async fn commit_and_open_pr(
    repo: &RepositoryConfig,
    github: &GithubConfig,
    workspace: &Path,
    request_id: &str,
) -> anyhow::Result<Option<String>> {
    // Nothing produced by the agent → no PR.
    if crate::git::status_entries(workspace)?.is_empty() {
        return Ok(None);
    }
    crate::git::add_all(workspace)?;
    let subject = format!("discuss artifact (request {request_id})");
    crate::git::commit(workspace, &subject)?;

    let push_remote = if github.fork_owner.is_some() { "fork" } else { "origin" };
    crate::git::push_force_with_lease(workspace, &repo.agent_branch, push_remote)?;

    let (owner, name) = crate::github::parse_repo_url(&repo.url)?;
    let token = crate::github_credentials::resolve_token(github, &owner)?;
    let head = if let Some(fork_owner) = github.fork_owner.as_deref() {
        format!("{fork_owner}:{}", repo.agent_branch)
    } else {
        repo.agent_branch.clone()
    };
    let title = format!("discuss: artifact from chat discussion (request {request_id})");
    let body = "This PR carries the artifact produced by a `discuss` → `send it` \
        chat conversation. Review + merge to apply it.";
    let pr = crate::github::create_pull_request(
        &owner,
        &name,
        &head,
        &repo.base_branch,
        &title,
        body,
        &token,
        None,
        false,
    )
    .await?;
    Ok(Some(pr.html_url))
}

/// Try to acquire the per-repo busy marker, waiting (bounded) for a running
/// executor pass to finish. Returns the guard on success, `None` on timeout.
async fn acquire_busy_when_free(
    deps: &DiscussHandlerDeps,
    repo: &RepositoryConfig,
    workspace: &Path,
) -> Option<crate::busy_marker::BusyGuard> {
    // Poll up to ~10 minutes; a normal executor pass finishes well within that.
    for _ in 0..120 {
        match crate::busy_marker::try_acquire(&deps.paths, workspace, &repo.url, 3_600) {
            Ok(crate::busy_marker::AcquireOutcome::Acquired(guard)) => return Some(guard),
            Ok(_) => {
                // Fresh in-progress OR ambiguous — wait and retry.
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            Err(e) => {
                tracing::warn!("discuss send-it: busy-marker acquire error: {e:#}");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
    None
}

/// The periodic sweep: fire once-per-stale-discussion 7-day idle reminders for
/// deferred discussions, then prune `DiscussionState` older than 14 days.
async fn run_sweep(deps: &DiscussHandlerDeps) {
    let state_root = discussion_state::default_state_root(&deps.paths);
    let now = Utc::now();
    let states = discussion_state::list_states(&state_root).unwrap_or_default();
    for mut state in states {
        let stale = now - state.last_activity_at > chrono::Duration::days(7);
        let holds_defer = state.deferred_slug.is_some();
        let already_reminded = state.reminded_at.is_some();
        let active = state.status != DiscussionStatus::Completed;
        if stale && holds_defer && !already_reminded && active {
            let slug = state.deferred_slug.clone().unwrap_or_default();
            let body = format!(
                "This discussion has been idle for 7 days. `{slug}` is still deferred. \
                 Run @<bot> send it to proceed or @<bot> undefer {repo} {slug} to release it.",
                repo = state.repo_url,
            );
            post_reply(deps, &state.channel, &state.thread_ts, &body).await;
            state.reminded_at = Some(now);
            let _ = discussion_state::write_state(&state_root, &state);
        }
    }
    match discussion_state::prune_stale_entries(&state_root, chrono::Duration::days(14)) {
        Ok(0) => {}
        Ok(n) => tracing::debug!(count = n, "discussions prune removed {n} stale entry(ies)"),
        Err(e) => tracing::warn!("discussions prune failed: {e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_defer_signal_pulls_slug_and_strips_line() {
        let reply = "Sure, this touches an existing spec.\nDISCUSS-DEFER: a03-spec-revision-thread\nLet me know.";
        let (clean, slug) = extract_defer_signal(reply);
        assert_eq!(slug.as_deref(), Some("a03-spec-revision-thread"));
        assert!(!clean.contains("DISCUSS-DEFER"));
        assert!(clean.contains("existing spec"));
        assert!(clean.contains("Let me know."));
    }

    #[test]
    fn extract_defer_signal_absent_returns_reply_unchanged() {
        let reply = "Just a normal answer with no signal.";
        let (clean, slug) = extract_defer_signal(reply);
        assert!(slug.is_none());
        assert_eq!(clean, reply);
    }

    #[test]
    fn extract_defer_signal_rejects_bad_slug() {
        let reply = "text\nDISCUSS-DEFER: Not A Slug!\nmore";
        let (_clean, slug) = extract_defer_signal(reply);
        assert!(slug.is_none(), "invalid slug shape must not be captured");
    }

    #[test]
    fn extract_defer_signal_backtick_wrapped() {
        let (_c, slug) = extract_defer_signal("DISCUSS-DEFER: `my-change`");
        assert_eq!(slug.as_deref(), Some("my-change"));
    }

    /// 5.6: a DiscussSendItAction artifact job waits for a running executor
    /// before starting. The gate is the per-repo busy marker: while a pass
    /// holds it, a fresh acquire attempt does NOT succeed (the handler loops).
    #[test]
    fn send_it_waits_when_executor_running() {
        let (_td, paths) = crate::testing::test_daemon_paths();
        let ws = std::path::Path::new("/tmp/discuss-ws-56");
        let url = "git@github.com:owner/repo.git";
        // Simulate a running executor pass: hold the busy marker.
        let _running = match crate::busy_marker::try_acquire(&paths, ws, url, 3_600) {
            Ok(crate::busy_marker::AcquireOutcome::Acquired(g)) => g,
            _ => panic!("first acquire should succeed"),
        };
        // The send-it gate's acquire attempt must NOT get the marker → it waits.
        match crate::busy_marker::try_acquire(&paths, ws, url, 3_600) {
            Ok(crate::busy_marker::AcquireOutcome::Acquired(_)) => {
                panic!("send-it must wait while an executor holds the marker")
            }
            Ok(_) => { /* SkipFreshInProgress / SkipAmbiguous — job waits */ }
            Err(e) => panic!("acquire errored: {e:#}"),
        }
    }

    /// 5.7: a DiscussSendItAction artifact job starts immediately when no
    /// executor is running — the busy marker is free, so the first acquire wins.
    #[test]
    fn send_it_starts_immediately_when_no_executor() {
        let (_td, paths) = crate::testing::test_daemon_paths();
        let ws = std::path::Path::new("/tmp/discuss-ws-57");
        let url = "git@github.com:owner/repo.git";
        match crate::busy_marker::try_acquire(&paths, ws, url, 3_600) {
            Ok(crate::busy_marker::AcquireOutcome::Acquired(_g)) => { /* starts immediately */ }
            Ok(_) => panic!("no executor running → acquire must succeed immediately"),
            Err(e) => panic!("acquire errored: {e:#}"),
        }
    }
}
