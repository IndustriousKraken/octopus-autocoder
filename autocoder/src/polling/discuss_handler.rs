//! Always-on discuss handler (the conversational `discuss`/`propose` flow).
//!
//! Spawned once at daemon launch, separate from the per-repo polling loops. It
//! consumes [`DiscussEvent`]s off an mpsc channel fed by the control-socket
//! dispatcher, so a `discuss` reply reaches the agent with no polling-loop
//! delay:
//!
//!   - [`DiscussEvent::Start`] / [`DiscussEvent::Continue`] run a READ-ONLY
//!     agentic session (the agent writes nothing) AND post the reply to the
//!     thread. The resumable session id is persisted to `DiscussionState` so
//!     the next turn resumes the cached prefix.
//!   - [`DiscussEvent::SendIt`] queues an artifact-creation job on the per-repo
//!     sequential executor gate (the busy marker): it waits for a running
//!     executor to finish, then resumes the session in WRITE mode, the daemon
//!     commits the artifact, opens a PR on `agent-q`, AND posts the PR URL.
//!
//! A periodic tick posts the 7-day idle reminder for stale discussions holding
//! a defer marker (once each). The 14-day prune lives in the polling loop's
//! `run_state_housekeeping`, mirroring the proposal-request prune.

use crate::config::{GithubConfig, RepositoryConfig};
use crate::control_socket::{
    ChatOpsHolder, DiscussAction, DiscussContinueAction, DiscussEvent, DiscussSendItAction,
    GithubHolder, RepoTaskMap,
};
use crate::discussion_state::{self, DiscussionState, DiscussionStatus};
use crate::executor::{DiscussContext, Executor};
use crate::paths::DaemonPaths;
use chrono::Utc;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Line prefix the discuss agent emits to signal it is amending an existing
/// spec/change; the handler defers that unit AND strips the line before the
/// operator sees the reply.
const DISCUSS_DEFER_PREFIX: &str = "DISCUSS-DEFER:";

/// Idle threshold (days) after which a defer-holding discussion gets one
/// reminder in its thread.
const IDLE_REMINDER_AFTER_DAYS: i64 = 7;

/// How often the handler scans for stale discussions to remind. Hourly is far
/// finer than the 7-day threshold; the `reminded_at` gate makes it fire once.
const IDLE_TICK: Duration = Duration::from_secs(3600);

/// The always-on handler loop. Returns when the channel closes or the daemon
/// cancels.
pub async fn run(
    paths: Arc<DaemonPaths>,
    executor: Arc<dyn Executor>,
    github: GithubHolder,
    chatops: ChatOpsHolder,
    repo_tasks: RepoTaskMap,
    stuck_threshold_secs: u64,
    mut rx: mpsc::UnboundedReceiver<DiscussEvent>,
    cancel: CancellationToken,
) {
    tracing::info!("discuss handler started");
    let mut idle_tick = tokio::time::interval(IDLE_TICK);
    idle_tick.tick().await; // consume the immediate first tick
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            maybe = rx.recv() => match maybe {
                Some(event) => {
                    process_event(
                        &paths, executor.as_ref(), &github, &chatops, &repo_tasks,
                        stuck_threshold_secs, &cancel, event,
                    )
                    .await;
                }
                None => break, // all senders dropped
            },
            _ = idle_tick.tick() => idle_reminder_pass(&paths, &chatops).await,
        }
    }
    tracing::info!("discuss handler exiting");
}

#[allow(clippy::too_many_arguments)]
async fn process_event(
    paths: &DaemonPaths,
    executor: &dyn Executor,
    github: &GithubHolder,
    chatops: &ChatOpsHolder,
    repo_tasks: &RepoTaskMap,
    stuck_threshold_secs: u64,
    cancel: &CancellationToken,
    event: DiscussEvent,
) {
    match event {
        DiscussEvent::Start(a) => on_start(paths, executor, chatops, repo_tasks, a).await,
        DiscussEvent::Continue(a) => on_continue(paths, executor, chatops, repo_tasks, a).await,
        DiscussEvent::SendIt(a) => {
            on_send_it(
                paths,
                executor,
                github,
                chatops,
                repo_tasks,
                stuck_threshold_secs,
                cancel,
                a,
            )
            .await
        }
    }
}

/// Handle a new discuss request: run the first read-only turn AND reply.
async fn on_start(
    paths: &DaemonPaths,
    executor: &dyn Executor,
    chatops: &ChatOpsHolder,
    repo_tasks: &RepoTaskMap,
    action: DiscussAction,
) {
    let state_root = discussion_state::default_state_root(paths);
    let Some(repo) = resolve_repo(repo_tasks, &action.repo_url) else {
        post(
            chatops,
            &action.channel,
            &action.thread_ts,
            "✗ discuss: no live polling task for this repo; cannot start a session.",
        )
        .await;
        return;
    };
    let workspace = crate::workspace::resolve_path(paths, &repo);
    let prompt = build_turn_prompt(&repo.url, &action.initial_text, true, false);
    let ctx = DiscussContext {
        prompt,
        resume_session_id: None,
        write_mode: false,
    };
    match executor.run_discuss(&workspace, &ctx).await {
        Ok(turn) => {
            let (visible, defer_slug) = extract_defer(&turn.reply);
            update_state(&state_root, &action.thread_ts, |s| {
                s.session_id = turn.session_id.clone();
                s.last_activity_at = Utc::now();
            });
            if let Some(slug) = defer_slug {
                maybe_auto_defer(paths, &repo, &workspace, &state_root, &action.thread_ts, &slug);
                post(chatops, &action.channel, &action.thread_ts, &defer_reply(&repo.url, &slug))
                    .await;
            }
            post(chatops, &action.channel, &action.thread_ts, &reply_or_placeholder(&visible)).await;
        }
        Err(e) => {
            post(
                chatops,
                &action.channel,
                &action.thread_ts,
                &format!("✗ discuss: session error: {e}"),
            )
            .await;
        }
    }
}

/// Handle a follow-up reply: resume the session read-only AND reply.
async fn on_continue(
    paths: &DaemonPaths,
    executor: &dyn Executor,
    chatops: &ChatOpsHolder,
    repo_tasks: &RepoTaskMap,
    action: DiscussContinueAction,
) {
    let state_root = discussion_state::default_state_root(paths);
    let state = match discussion_state::read_state(&state_root, &action.thread_ts) {
        Ok(Some(s)) if s.status == DiscussionStatus::Active => s,
        _ => {
            post(
                chatops,
                &action.channel,
                &action.thread_ts,
                "✗ This discussion is no longer active. Start a new one with @<bot> discuss <repo> <text>.",
            )
            .await;
            return;
        }
    };
    let Some(repo) = resolve_repo(repo_tasks, &action.repo_url) else {
        post(chatops, &action.channel, &action.thread_ts, "✗ discuss: no live task for this repo.")
            .await;
        return;
    };
    let workspace = crate::workspace::resolve_path(paths, &repo);
    // Resume the cached session when we have its id; otherwise re-seed a fresh
    // turn with the template so the agent still has its instructions.
    let fresh = state.session_id.is_none();
    let prompt = build_turn_prompt(&repo.url, &action.text, fresh, false);
    let ctx = DiscussContext {
        prompt,
        resume_session_id: state.session_id.clone(),
        write_mode: false,
    };
    match executor.run_discuss(&workspace, &ctx).await {
        Ok(turn) => {
            let (visible, defer_slug) = extract_defer(&turn.reply);
            update_state(&state_root, &action.thread_ts, |s| {
                if turn.session_id.is_some() {
                    s.session_id = turn.session_id.clone();
                }
                s.last_activity_at = Utc::now();
            });
            if let Some(slug) = defer_slug
                && state.deferred_slug.as_deref() != Some(slug.as_str())
            {
                maybe_auto_defer(paths, &repo, &workspace, &state_root, &action.thread_ts, &slug);
                post(chatops, &action.channel, &action.thread_ts, &defer_reply(&repo.url, &slug))
                    .await;
            }
            post(chatops, &action.channel, &action.thread_ts, &reply_or_placeholder(&visible)).await;
        }
        Err(e) => {
            post(
                chatops,
                &action.channel,
                &action.thread_ts,
                &format!("✗ discuss: session error: {e}"),
            )
            .await;
        }
    }
}

/// Handle `send it`: run the artifact-creation job sequentially after any
/// running executor, commit + open a PR, post the URL.
#[allow(clippy::too_many_arguments)]
async fn on_send_it(
    paths: &DaemonPaths,
    executor: &dyn Executor,
    github: &GithubHolder,
    chatops: &ChatOpsHolder,
    repo_tasks: &RepoTaskMap,
    stuck_threshold_secs: u64,
    cancel: &CancellationToken,
    action: DiscussSendItAction,
) {
    let state_root = discussion_state::default_state_root(paths);
    let Some(mut state) = discussion_state::read_state(&state_root, &action.thread_ts).ok().flatten()
    else {
        post(
            chatops,
            &action.channel,
            &action.thread_ts,
            "✗ This discussion is no longer active. Start a new one with @<bot> discuss <repo> <text>.",
        )
        .await;
        return;
    };
    let Some(repo) = resolve_repo(repo_tasks, &action.repo_url) else {
        post(chatops, &action.channel, &action.thread_ts, "✗ discuss: no live task for this repo.")
            .await;
        return;
    };
    let github_cfg = github.load_full().as_ref().clone();
    let workspace = crate::workspace::resolve_path(paths, &repo);

    // Mark Executing so a repeat `send it` doesn't double-fire.
    state.status = DiscussionStatus::Executing;
    state.last_activity_at = Utc::now();
    let _ = discussion_state::write_state(&state_root, &state);

    // Sequential executor gate: wait for a running executor to finish (start
    // immediately if none). Held for the whole artifact-creation + PR.
    let _guard = match acquire_executor_slot(
        paths,
        &workspace,
        &repo.url,
        stuck_threshold_secs,
        cancel,
    )
    .await
    {
        Some(g) => g,
        None => {
            // Cancelled while waiting.
            state.status = DiscussionStatus::Active;
            let _ = discussion_state::write_state(&state_root, &state);
            return;
        }
    };

    // Prepare a clean agent branch at the base tip.
    if let Err(e) = prepare_artifact_branch(paths, &workspace, &repo, &github_cfg) {
        post(
            chatops,
            &action.channel,
            &action.thread_ts,
            &format!("✗ discuss: could not prepare the workspace: {e:#}"),
        )
        .await;
        state.status = DiscussionStatus::Active;
        let _ = discussion_state::write_state(&state_root, &state);
        return;
    }

    // Resume the session in write mode; the agent writes the artifact files.
    let prompt = build_send_it_prompt(&repo.url, action.final_context.as_deref(), state.session_id.is_none());
    let ctx = DiscussContext {
        prompt,
        resume_session_id: state.session_id.clone(),
        write_mode: true,
    };
    let turn = match executor.run_discuss(&workspace, &ctx).await {
        Ok(t) => t,
        Err(e) => {
            let _ = crate::git::reset_hard_head(&workspace);
            let _ = crate::git::clean_force(&workspace);
            let _ = crate::git::checkout(&workspace, &repo.base_branch);
            post(
                chatops,
                &action.channel,
                &action.thread_ts,
                &format!("✗ discuss: artifact session failed: {e}"),
            )
            .await;
            state.status = DiscussionStatus::Active;
            let _ = discussion_state::write_state(&state_root, &state);
            return;
        }
    };

    // Did the agent actually produce anything?
    let changed = crate::git::status_entries(&workspace).unwrap_or_default();
    if changed.is_empty() {
        let _ = crate::git::checkout(&workspace, &repo.base_branch);
        post(
            chatops,
            &action.channel,
            &action.thread_ts,
            "ℹ️ discuss: the agent produced no artifact to commit. Reply with more direction, or start over.",
        )
        .await;
        state.status = DiscussionStatus::Active;
        let _ = discussion_state::write_state(&state_root, &state);
        return;
    }

    // Commit + push + open the PR on agent-q.
    let pr_url = match commit_and_open_pr(paths, &workspace, &repo, &github_cfg, &turn.reply).await {
        Ok(url) => url,
        Err(e) => {
            let _ = crate::git::reset_hard_head(&workspace);
            let _ = crate::git::clean_force(&workspace);
            let _ = crate::git::checkout(&workspace, &repo.base_branch);
            post(
                chatops,
                &action.channel,
                &action.thread_ts,
                &format!("✗ discuss: committing/opening the PR failed: {e:#}"),
            )
            .await;
            state.status = DiscussionStatus::Active;
            let _ = discussion_state::write_state(&state_root, &state);
            return;
        }
    };

    // On success, clear any defer marker AND finalize state.
    if let Some(slug) = state.deferred_slug.clone() {
        clear_auto_defer(paths, &repo, &workspace, &github_cfg, &slug);
    }
    let _ = crate::git::checkout(&workspace, &repo.base_branch);
    state.status = DiscussionStatus::Completed;
    state.deferred_slug = None;
    state.last_activity_at = Utc::now();
    let _ = discussion_state::write_state(&state_root, &state);

    post(
        chatops,
        &action.channel,
        &action.thread_ts,
        &format!("✅ Done. Opened a PR on `{}`:\n{pr_url}", repo.agent_branch),
    )
    .await;
}

// ---------------------------------------------------------------------------
// Executor gate
// ---------------------------------------------------------------------------

/// Wait for the per-repo sequential executor slot (busy marker) to be free,
/// then acquire it. Returns the held guard, or `None` if cancelled while
/// waiting. This IS the "wait for a running executor to finish; start
/// immediately if none" gate.
async fn acquire_executor_slot(
    paths: &DaemonPaths,
    workspace: &Path,
    repo_url: &str,
    stuck_threshold_secs: u64,
    cancel: &CancellationToken,
) -> Option<crate::busy_marker::BusyGuard> {
    loop {
        match crate::busy_marker::try_acquire(paths, workspace, repo_url, stuck_threshold_secs) {
            Ok(crate::busy_marker::AcquireOutcome::Acquired(guard)) => return Some(guard),
            Ok(_) => {
                // An executor (or another job) holds the marker — wait + retry.
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => return None,
                    () = tokio::time::sleep(Duration::from_secs(5)) => {}
                }
            }
            Err(e) => {
                tracing::warn!(url = %repo_url, "discuss: busy-marker acquire error: {e:#}; retrying");
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => return None,
                    () = tokio::time::sleep(Duration::from_secs(5)) => {}
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Git + PR helpers for the artifact-creation step
// ---------------------------------------------------------------------------

fn push_remote_for(github_cfg: &GithubConfig) -> &'static str {
    if github_cfg.fork_owner.is_some() {
        "fork"
    } else {
        "origin"
    }
}

/// Ensure the workspace is initialized on a clean base, then recreate the agent
/// branch at the base tip so the artifact commit rides the normal PR flow.
fn prepare_artifact_branch(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
) -> anyhow::Result<()> {
    let fork_url = match github_cfg.fork_owner.as_deref() {
        Some(owner) => Some(crate::github::derive_fork_url(&repo.url, owner)?),
        None => None,
    };
    let fork_arg = fork_url.as_deref().map(|u| (u, repo.agent_branch.as_str()));
    crate::workspace::ensure_initialized(paths, workspace, &repo.url, fork_arg)?;
    let _ = crate::git::reset_hard_head(workspace);
    let _ = crate::git::clean_force(workspace);
    crate::git::fetch(workspace)?;
    crate::git::checkout(workspace, &repo.base_branch)?;
    if crate::git::pull_ff_only(workspace, &repo.base_branch).is_err() {
        crate::git::reset_hard_to_remote(workspace, &repo.base_branch)?;
    }
    crate::git::recreate_branch(workspace, &repo.agent_branch)?;
    Ok(())
}

/// Stage everything the agent wrote, commit on the agent branch, push, and open
/// a PR. Returns the PR URL.
async fn commit_and_open_pr(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    summary: &str,
) -> anyhow::Result<String> {
    crate::git::add_all(workspace)?;
    crate::git::commit(workspace, "discuss: artifact from chat discussion")?;
    crate::git::push_force_with_lease(workspace, &repo.agent_branch, push_remote_for(github_cfg))?;
    let title = "discuss: artifact from chat discussion";
    let body = format!(
        "This PR carries the artifact produced from a `discuss` chat session.\n\n{}",
        summary.trim()
    );
    let url = crate::polling_loop::open_triage_pull_request(
        paths,
        repo,
        github_cfg,
        &repo.agent_branch,
        &repo.base_branch,
        title,
        &body,
    )
    .await?;
    Ok(url)
}

// ---------------------------------------------------------------------------
// Auto-defer
// ---------------------------------------------------------------------------

/// Reply text posted when the agent signals a deferral.
fn defer_reply(repo_url: &str, slug: &str) -> String {
    let _ = repo_url;
    format!(
        "I've deferred `{slug}` while we discuss. If you decide not to follow through, clear it with `@<bot> undefer <repo> {slug}`. I'll clear it automatically when I open its PR."
    )
}

/// Best-effort auto-defer of a change under discussion: move
/// `openspec/changes/<slug>/` → `deferred-changes/<slug>/`, commit on the agent
/// branch, AND record the slug on the discussion state. A slug that does not
/// map to an active change directory is recorded only (no dir marker).
///
/// ponytail: auto-defer maps to the change dir-move (the existing defer marker
/// shape). It does NOT push during the read-only phase to avoid racing the
/// executor on `agent-q`; the marker is a local commit protecting the change,
/// and `send it` supersedes it. A bare canonical-requirement slug is recorded
/// + reminded but has no dir marker.
fn maybe_auto_defer(
    paths: &DaemonPaths,
    repo: &RepositoryConfig,
    workspace: &Path,
    state_root: &Path,
    thread_ts: &str,
    slug: &str,
) {
    let _ = paths;
    let from = workspace.join("openspec/changes").join(slug);
    let to = workspace.join("deferred-changes").join(slug);
    if from.is_dir() && !to.exists() {
        let moved = crate::git::recreate_branch(workspace, &repo.agent_branch)
            .and_then(|()| {
                if let Some(parent) = to.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::rename(&from, &to)?;
                crate::git::add_all(workspace)?;
                crate::git::commit(workspace, &format!("chore: defer {slug}"))
            });
        if let Err(e) = moved {
            tracing::warn!(slug = %slug, "discuss auto-defer: marker commit failed: {e:#}");
        }
    }
    update_state(state_root, thread_ts, |s| {
        s.deferred_slug = Some(slug.to_string());
        s.last_activity_at = Utc::now();
    });
}

/// Clear an auto-defer marker on `send it` completion: move the change back out
/// of `deferred-changes/` if it is still there. Best-effort.
fn clear_auto_defer(
    paths: &DaemonPaths,
    repo: &RepositoryConfig,
    workspace: &Path,
    github_cfg: &GithubConfig,
    slug: &str,
) {
    let _ = (paths, repo, github_cfg);
    let deferred = workspace.join("deferred-changes").join(slug);
    let lane = workspace.join("openspec/changes").join(slug);
    if deferred.is_dir() && !lane.exists() {
        if let Some(parent) = lane.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::rename(&deferred, &lane) {
            tracing::warn!(slug = %slug, "discuss: clearing auto-defer marker failed: {e:#}");
        }
    }
}

// ---------------------------------------------------------------------------
// Idle reminder (task 2.6)
// ---------------------------------------------------------------------------

/// Scan all discussions once; for each Active discussion holding a defer marker
/// whose last activity is older than 7 days AND that has not been reminded,
/// post one reminder AND stamp `reminded_at`.
async fn idle_reminder_pass(paths: &DaemonPaths, chatops: &ChatOpsHolder) {
    let state_root = discussion_state::default_state_root(paths);
    let states = match discussion_state::list_states(&state_root) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("discuss idle-reminder: list failed: {e:#}");
            return;
        }
    };
    let now = Utc::now();
    for mut s in states {
        let idle = now - s.last_activity_at > chrono::Duration::days(IDLE_REMINDER_AFTER_DAYS);
        let holds_defer = s.deferred_slug.is_some();
        let already = s.reminded_at.is_some();
        if s.status == DiscussionStatus::Active && idle && holds_defer && !already {
            let slug = s.deferred_slug.clone().unwrap_or_default();
            post(
                chatops,
                &s.channel,
                &s.thread_ts,
                &format!(
                    "⏰ This discussion has been idle for {IDLE_REMINDER_AFTER_DAYS} days. `{slug}` is still deferred. Run `@<bot> send it` to proceed or `@<bot> undefer <repo> {slug}` to release it."
                ),
            )
            .await;
            s.reminded_at = Some(now);
            let _ = discussion_state::write_state(&state_root, &s);
        }
    }
}

// ---------------------------------------------------------------------------
// Small shared helpers
// ---------------------------------------------------------------------------

/// Resolve the current `RepositoryConfig` for `url` from the live task map.
fn resolve_repo(repo_tasks: &RepoTaskMap, url: &str) -> Option<RepositoryConfig> {
    let g = repo_tasks.lock().unwrap();
    g.get(url).map(|h| h.config.load().as_ref().clone())
}

/// Post a threaded reply through the current chatops backend (best-effort).
async fn post(chatops: &ChatOpsHolder, channel: &str, thread_ts: &str, body: &str) {
    let slot = chatops.load_full();
    if let Some(slot) = slot.as_ref() {
        if let Err(e) = slot.backend.post_threaded_reply(channel, thread_ts, body).await {
            tracing::warn!("discuss: thread reply failed: {e:#}");
        }
    } else {
        tracing::warn!("discuss: no chatops backend; dropping reply: {body}");
    }
}

/// Read → mutate → write a `DiscussionState`, best-effort.
fn update_state(state_root: &Path, thread_ts: &str, f: impl FnOnce(&mut DiscussionState)) {
    match discussion_state::read_state(state_root, thread_ts) {
        Ok(Some(mut s)) => {
            f(&mut s);
            if let Err(e) = discussion_state::write_state(state_root, &s) {
                tracing::warn!(thread_ts = %thread_ts, "discuss: state write failed: {e:#}");
            }
        }
        Ok(None) => {
            tracing::warn!(thread_ts = %thread_ts, "discuss: state file missing on update");
        }
        Err(e) => tracing::warn!(thread_ts = %thread_ts, "discuss: state read failed: {e:#}"),
    }
}

/// Fall back to a friendly line when the agent returned an empty reply.
fn reply_or_placeholder(visible: &str) -> String {
    if visible.trim().is_empty() {
        "(the agent finished reading; reply with a question or `@<bot> send it` to create the artifact.)".to_string()
    } else {
        visible.to_string()
    }
}

/// Build the prompt for one read-only turn. `fresh` prepends the discuss-mode
/// template (only for a session that isn't being resumed).
fn build_turn_prompt(repo_url: &str, message: &str, fresh: bool, write_mode: bool) -> String {
    let mut out = String::new();
    if fresh {
        let tmpl = crate::prompts::PromptLoader::load(
            crate::prompts::PromptId::DiscussMode,
            None,
            None,
            None,
        );
        out.push_str(&tmpl.replace("{{repo_url}}", repo_url));
        out.push_str("\n\n---\n\nThe operator's message:\n\n");
    }
    if write_mode {
        out.push_str("WRITE MODE: you may now create and modify files.\n\n");
    }
    out.push_str(message);
    out
}

/// Build the `send it` write-mode prompt: a write-mode banner + optional final
/// context. `fresh` seeds the template when the session id was lost.
fn build_send_it_prompt(repo_url: &str, final_context: Option<&str>, fresh: bool) -> String {
    let mut out = String::new();
    if fresh {
        let tmpl = crate::prompts::PromptLoader::load(
            crate::prompts::PromptId::DiscussMode,
            None,
            None,
            None,
        );
        out.push_str(&tmpl.replace("{{repo_url}}", repo_url));
        out.push_str("\n\n---\n\n");
    }
    out.push_str(
        "WRITE MODE: The operator sent `send it`. You may now create and modify files. \
         Produce the single artifact the conversation converged on (a change under \
         `openspec/changes/<slug>/`, a roadmap item, an issue, or a docs update), per the \
         routing in your instructions. Write only the artifact + its planning files; do NOT \
         implement code fixes. The daemon commits your files and opens the PR. End with a \
         one-line summary of what you created.",
    );
    if let Some(fc) = final_context.map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str("\n\nThe operator's final context to fold in:\n\n");
        out.push_str(fc);
    }
    out
}

/// Extract a `DISCUSS-DEFER: <slug>` signal line from the agent's reply,
/// returning the reply with that line stripped AND the slug (if present).
fn extract_defer(reply: &str) -> (String, Option<String>) {
    let mut slug: Option<String> = None;
    let mut kept: Vec<&str> = Vec::new();
    for line in reply.lines() {
        if let Some(rest) = line.trim().strip_prefix(DISCUSS_DEFER_PREFIX) {
            let s = rest.trim();
            if !s.is_empty() && slug.is_none() {
                // The slug is agent-supplied AND used in filesystem paths
                // (maybe_auto_defer / clear_auto_defer join it under the
                // workspace). Reject path-traversal components so an
                // adversarially-prompted agent cannot move directories outside
                // the intended lanes. Line is still stripped from the reply.
                if s.contains("..") || s.contains('/') || s.contains('\\') {
                    tracing::warn!(slug = %s, "discuss auto-defer: rejecting slug with path-traversal components");
                } else {
                    slug = Some(s.to_string());
                }
            }
            continue; // strip the signal line
        }
        kept.push(line);
    }
    (kept.join("\n").trim().to_string(), slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_defer_pulls_slug_and_strips_line() {
        let reply = "Here is my understanding.\nDISCUSS-DEFER: some-existing-change\nMore text.";
        let (visible, slug) = extract_defer(reply);
        assert_eq!(slug.as_deref(), Some("some-existing-change"));
        assert!(!visible.contains("DISCUSS-DEFER"));
        assert!(visible.contains("Here is my understanding."));
        assert!(visible.contains("More text."));
    }

    #[test]
    fn extract_defer_none_when_absent() {
        let (visible, slug) = extract_defer("just a normal reply");
        assert_eq!(slug, None);
        assert_eq!(visible, "just a normal reply");
    }

    #[test]
    fn extract_defer_rejects_path_traversal_slugs() {
        // Slug flows into workspace-relative path joins; traversal components
        // must be dropped (line still stripped from the visible reply).
        for bad in [
            "DISCUSS-DEFER: ../../other-change",
            "DISCUSS-DEFER: foo/bar",
            "DISCUSS-DEFER: ..\\evil",
        ] {
            let reply = format!("Understood.\n{bad}\nTrailing.");
            let (visible, slug) = extract_defer(&reply);
            assert_eq!(slug, None, "traversal slug must be rejected: {bad}");
            assert!(!visible.contains("DISCUSS-DEFER"), "signal line still stripped");
        }
    }

    #[test]
    fn build_turn_prompt_fresh_includes_template_and_message() {
        let p = build_turn_prompt("git@github.com:o/r.git", "how does X work?", true, false);
        assert!(p.contains("Discuss mode"));
        assert!(p.contains("git@github.com:o/r.git"));
        assert!(p.contains("how does X work?"));
    }

    #[test]
    fn build_turn_prompt_resumed_is_just_message() {
        let p = build_turn_prompt("git@github.com:o/r.git", "follow up", false, false);
        assert_eq!(p, "follow up");
    }

    #[test]
    fn send_it_prompt_folds_final_context() {
        let p = build_send_it_prompt("git@github.com:o/r.git", Some("go with Option B"), false);
        assert!(p.contains("WRITE MODE"));
        assert!(p.contains("go with Option B"));
    }

    #[tokio::test]
    async fn send_it_gate_starts_immediately_when_no_executor_running() {
        // Task 5.7.
        let ws = tempfile::TempDir::new().unwrap();
        let (_td, paths) = crate::testing::test_daemon_paths();
        let cancel = CancellationToken::new(); // not cancelled
        let guard = acquire_executor_slot(
            &paths,
            ws.path(),
            "git@github.com:o/r.git",
            1800,
            &cancel,
        )
        .await;
        assert!(guard.is_some(), "must acquire immediately when no executor holds the marker");
    }

    #[tokio::test]
    async fn send_it_gate_waits_while_executor_holds_marker() {
        // Task 5.6: while an executor holds the busy marker, the gate does NOT
        // start — it waits. With an already-cancelled token it returns None
        // (proving it entered the wait branch rather than acquiring).
        let ws = tempfile::TempDir::new().unwrap();
        let (_td, paths) = crate::testing::test_daemon_paths();
        let url = "git@github.com:o/r.git";
        // Simulate a running executor by holding the busy marker.
        let _held = match crate::busy_marker::try_acquire(&paths, ws.path(), url, 1800).unwrap() {
            crate::busy_marker::AcquireOutcome::Acquired(g) => g,
            _ => panic!("expected to acquire the marker for the test setup"),
        };
        let cancel = CancellationToken::new();
        cancel.cancel();
        let guard = acquire_executor_slot(&paths, ws.path(), url, 1800, &cancel).await;
        assert!(
            guard.is_none(),
            "must wait (not start) while an executor holds the marker"
        );
    }
}
