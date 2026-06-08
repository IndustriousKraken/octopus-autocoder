//! Per-repository polling loop. Each iteration runs a single pass: branch
//! init → queue walk → push + PR if commits were produced. Failures inside
//! one iteration are logged and the loop continues to the next sleep.

use crate::alert_state::{AlertCategory, AlertEntry, AlertState};
use crate::alerts::{handle_classified_recovery_failure, handle_predictable_failure};
use crate::audits::AuditRegistry;
use crate::audits::scheduler::run_due_audits;
use crate::busy_marker;
use crate::chatops::{self, AnswerPayload, ChatOpsBackend, QuestionPayload};
use crate::code_reviewer::{
    CodeReviewer, PerChangeContext, ReviewConcern, ReviewReport, ReviewVerdict,
    build_cross_change_preamble,
};
use crate::config::{AuditSettings, AuditsConfig, GithubConfig, RepositoryConfig};
use crate::control_socket::{
    CacheHolder, ChatOpsHolder, ChatOpsSlot, GithubHolder, ReviewerHolder,
};
use crate::executor::{Executor, ExecutorOutcome, ResumeHandle, UnimplementableTask};
use crate::paths::DaemonPaths;
use crate::recovery_classification::{RecoveryFailureClass, classify_recovery_failure};
use crate::spec_revision::{self, SpecNeedsRevisionDetail};
use crate::{failure_state, git, github, perma_stuck, queue, workspace};
use anyhow::{Context, Result, anyhow};
use arc_swap::ArcSwap;
use chrono::{Duration as ChronoDuration, Utc};
use rand::Rng;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

mod alerts_throttle;
pub(crate) use alerts_throttle::*;
mod alerts_comment;
pub(crate) use alerts_comment::*;
mod alerts_notify;
pub(crate) use alerts_notify::*;
mod queue_walk;
pub(crate) use queue_walk::*;
mod operator_requests;
pub(crate) use operator_requests::*;
mod queue_waiting;
pub(crate) use queue_waiting::*;
mod preflight_checks;
pub(crate) use preflight_checks::*;
mod review_context;
pub(crate) use review_context::*;
mod pr_open;
pub(crate) use pr_open::*;
mod pr_body;
pub(crate) use pr_body::*;
mod rebuild;
pub(crate) use rebuild::*;
mod triage;
pub(crate) use triage::*;
mod triage_scrub;
pub(crate) use triage_scrub::*;
mod proposals;
pub(crate) use proposals::*;
mod outcome;
pub(crate) use outcome::*;
mod loop_drive;
pub(crate) use loop_drive::*;
mod pass;
pub(crate) use pass::*;
mod commits;
pub(crate) use commits::*;

/// Per-pass ChatOps context: the provider-agnostic backend + the resolved
/// channel id for THIS repository, plus the operator's notification
/// preferences. Constructed once at startup from the global `chatops:`
/// config and the per-repo `chatops_channel_id` override.
pub struct ChatOpsContext {
    pub chatops: Arc<dyn ChatOpsBackend>,
    pub channel: String,
    /// Whether to post a one-line notification each time a pending change
    /// is dequeued for execution. Defaults to `true` when the operator did
    /// not set `chatops.notifications.start_work`.
    pub start_work_enabled: bool,
    /// Whether to emit throttled chatops alerts at the three predictable
    /// failure sites (workspace init, branch push, PR creation). Defaults
    /// to `true` when the operator did not set
    /// `chatops.notifications.failure_alerts`.
    pub failure_alerts_enabled: bool,
    /// Whether to post a one-line notification each time a PR is opened
    /// (with a link to the PR). Defaults to `true` when the operator did
    /// not set `chatops.notifications.pr_opened`.
    pub pr_opened_enabled: bool,
}

/// Run the polling loop for a single repository. Each iteration is wrapped in
/// `execute_one_pass`; failures inside a pass are logged and do not break the
/// loop. Cancellation is checked between iterations and during the sleep.
///
/// The `github`, `reviewer`, and `chatops` holders are reloaded at the top
/// of each pass — see the control socket (`autocoder reload`) for the
/// mechanism that swaps values into them. The `repo` holder is also reloaded
/// at the top of each pass so the reload handler can hot-swap repository
/// settings (base/agent branch, poll interval, channel id, local_path,
/// per-repo PR cap); the snapshot captured at the start of an iteration is
/// used consistently for the rest of that iteration. The next iteration
/// picks up any swap that happened during the previous sleep.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    paths: Arc<DaemonPaths>,
    repo: Arc<ArcSwap<RepositoryConfig>>,
    executor: Arc<dyn Executor>,
    github_holder: GithubHolder,
    reviewer_holder: ReviewerHolder,
    chatops_holder: ChatOpsHolder,
    cache_holder: CacheHolder,
    stuck_threshold_secs: u64,
    perma_stuck_threshold: u32,
    executor_max_changes_per_pr: Option<u32>,
    revision_cap: u32,
    human_revise_cap: u32,
    startup_jitter_max_secs: u64,
    inter_iteration_jitter_pct: u8,
    audit_registry: Arc<AuditRegistry>,
    audits_cfg: Option<Arc<AuditsConfig>>,
    audit_settings: Arc<HashMap<String, AuditSettings>>,
    pending_rebuild: Arc<std::sync::atomic::AtomicBool>,
    pending_triages: Arc<std::sync::Mutex<Vec<String>>>,
    pending_audit_runs: Arc<std::sync::Mutex<Vec<String>>>,
    pending_proposal_requests: Arc<std::sync::Mutex<Vec<crate::control_socket::ProposalRequest>>>,
    pending_changelog_requests: Arc<std::sync::Mutex<Vec<crate::control_socket::ChangelogRequest>>>,
    pending_brownfield_requests: Arc<
        std::sync::Mutex<std::collections::VecDeque<crate::control_socket::BrownfieldRequest>>,
    >,
    pending_scout_requests: Arc<
        std::sync::Mutex<std::collections::VecDeque<crate::control_socket::ScoutRequest>>,
    >,
    pending_spec_it_requests: Arc<
        std::sync::Mutex<std::collections::VecDeque<crate::control_socket::SpecItRequest>>,
    >,
    pending_sync_upstream_requests: Arc<
        std::sync::Mutex<std::collections::VecDeque<crate::control_socket::SyncUpstreamRequest>>,
    >,
    pending_brownfield_survey_requests: Arc<
        std::sync::Mutex<
            std::collections::VecDeque<crate::control_socket::BrownfieldSurveyRequest>,
        >,
    >,
    pending_brownfield_batch_requests: Arc<
        std::sync::Mutex<std::collections::VecDeque<crate::control_socket::BrownfieldBatchRequest>>,
    >,
    iteration_cancel: Arc<std::sync::Mutex<Option<CancellationToken>>>,
    iteration_drained: Arc<tokio::sync::Notify>,
    cancel: CancellationToken,
) {
    run_with_hooks(
        paths,
        repo,
        executor,
        github_holder,
        reviewer_holder,
        chatops_holder,
        cache_holder,
        stuck_threshold_secs,
        perma_stuck_threshold,
        executor_max_changes_per_pr,
        revision_cap,
        human_revise_cap,
        startup_jitter_max_secs,
        inter_iteration_jitter_pct,
        audit_registry,
        audits_cfg,
        audit_settings,
        pending_rebuild,
        pending_triages,
        pending_audit_runs,
        pending_proposal_requests,
        pending_changelog_requests,
        pending_brownfield_requests,
        pending_scout_requests,
        pending_spec_it_requests,
        pending_sync_upstream_requests,
        pending_brownfield_survey_requests,
        pending_brownfield_batch_requests,
        iteration_cancel,
        iteration_drained,
        cancel,
        RunHooks::default(),
    )
    .await
}

/// Test-only hooks for synchronizing with the polling loop's internal
/// state. Production code always passes `RunHooks::default()` (every
/// field `None`); tests inject a `Notify` so they can wait on iteration
/// boundaries event-driven instead of sleep-polling.
#[derive(Default, Clone)]
pub struct RunHooks {
    /// Fires once each time the loop has finished an iteration and is
    /// about to enter its inter-iteration sleep. Tests that need to
    /// race a cancel against the sleep wait on this to know the loop
    /// reached the sleep window.
    pub on_iteration_sleep: Option<Arc<tokio::sync::Notify>>,
}

/// Drops at the end of the iteration body — including the panic-unwind
/// path — so the per-iteration cancel handle is cleared and the
/// `iteration_drained` Notify fires from every exit path without manual
/// repetition. The wipe-workspace handler awaits the Notify after firing
/// the per-iteration cancel; the drop ensures it always wakes.
struct IterationGuard<'a> {
    iteration_cancel: &'a std::sync::Mutex<Option<CancellationToken>>,
    iteration_drained: &'a tokio::sync::Notify,
}

impl Drop for IterationGuard<'_> {
    fn drop(&mut self) {
        *self.iteration_cancel.lock().unwrap() = None;
        self.iteration_drained.notify_waiters();
    }
}

/// Same as `run` but accepts a `RunHooks` for test-only synchronization.
#[allow(clippy::too_many_arguments)]
pub async fn run_with_hooks(
    paths: Arc<DaemonPaths>,
    repo: Arc<ArcSwap<RepositoryConfig>>,
    executor: Arc<dyn Executor>,
    github_holder: GithubHolder,
    reviewer_holder: ReviewerHolder,
    chatops_holder: ChatOpsHolder,
    cache_holder: CacheHolder,
    stuck_threshold_secs: u64,
    perma_stuck_threshold: u32,
    executor_max_changes_per_pr: Option<u32>,
    revision_cap: u32,
    human_revise_cap: u32,
    startup_jitter_max_secs: u64,
    inter_iteration_jitter_pct: u8,
    audit_registry: Arc<AuditRegistry>,
    audits_cfg: Option<Arc<AuditsConfig>>,
    audit_settings: Arc<HashMap<String, AuditSettings>>,
    pending_rebuild: Arc<std::sync::atomic::AtomicBool>,
    pending_triages: Arc<std::sync::Mutex<Vec<String>>>,
    pending_audit_runs: Arc<std::sync::Mutex<Vec<String>>>,
    pending_proposal_requests: Arc<std::sync::Mutex<Vec<crate::control_socket::ProposalRequest>>>,
    pending_changelog_requests: Arc<std::sync::Mutex<Vec<crate::control_socket::ChangelogRequest>>>,
    pending_brownfield_requests: Arc<
        std::sync::Mutex<std::collections::VecDeque<crate::control_socket::BrownfieldRequest>>,
    >,
    pending_scout_requests: Arc<
        std::sync::Mutex<std::collections::VecDeque<crate::control_socket::ScoutRequest>>,
    >,
    pending_spec_it_requests: Arc<
        std::sync::Mutex<std::collections::VecDeque<crate::control_socket::SpecItRequest>>,
    >,
    pending_sync_upstream_requests: Arc<
        std::sync::Mutex<std::collections::VecDeque<crate::control_socket::SyncUpstreamRequest>>,
    >,
    pending_brownfield_survey_requests: Arc<
        std::sync::Mutex<
            std::collections::VecDeque<crate::control_socket::BrownfieldSurveyRequest>,
        >,
    >,
    pending_brownfield_batch_requests: Arc<
        std::sync::Mutex<std::collections::VecDeque<crate::control_socket::BrownfieldBatchRequest>>,
    >,
    iteration_cancel: Arc<std::sync::Mutex<Option<CancellationToken>>>,
    iteration_drained: Arc<tokio::sync::Notify>,
    cancel: CancellationToken,
    hooks: RunHooks,
) {
    if log_startup_and_jitter(&repo, &paths, startup_jitter_max_secs, &cancel).await {
        return;
    }

    // a71: bundle the three operator-chatops-request queue handles so the
    // queue walk can PEEK them between changes and yield the batch early
    // when an operator request is waiting (the iteration-top drains below
    // remain the sole consumer). Bound to a task-local around each
    // iteration's work future via `operator_requests::scope`. Cheap clone
    // (three `Arc`s) per iteration.
    let operator_request_queues = OperatorRequestQueues {
        triages: pending_triages.clone(),
        proposal_requests: pending_proposal_requests.clone(),
        changelog_requests: pending_changelog_requests.clone(),
    };

    loop {
        if cancel.is_cancelled() {
            break;
        }

        // Per-iteration cancel token (child of the global cancel). The
        // IterationGuard clears the slot + fires the drain Notify on every
        // exit path so a wipe-workspace handler always wakes.
        let iter_cancel = cancel.child_token();
        *iteration_cancel.lock().unwrap() = Some(iter_cancel.clone());
        let iter_guard = IterationGuard {
            iteration_cancel: iteration_cancel.as_ref(),
            iteration_drained: iteration_drained.as_ref(),
        };

        // Single-snapshot-per-iteration: read repo/github/reviewer/chatops
        // exactly once so a mid-iteration reload cannot tear the config.
        let snapshot = repo.load();
        let snapshot_ref: &RepositoryConfig = snapshot.as_ref();
        let workspace = workspace::resolve_path(&paths, snapshot_ref);
        let github_snap = github_holder.load_full();
        let reviewer_snap = reviewer_holder.load_full();
        let chatops_snap = chatops_holder.load_full();
        let chatops_ctx = chatops_snap
            .as_ref()
            .as_ref()
            .map(|slot| build_chatops_ctx(snapshot_ref, slot));
        let max_changes_per_pr = resolve_max_changes_per_pr(
            snapshot_ref.max_changes_per_pr,
            executor_max_changes_per_pr,
        );

        run_workspace_cache_eviction(&paths, &workspace, &cache_holder).await;

        // Take-and-clear the rebuild flag so it can't trigger twice.
        let want_rebuild = pending_rebuild.swap(false, std::sync::atomic::Ordering::SeqCst);

        run_state_housekeeping(&paths);

        // Drain the per-repo on-demand audit-run queue; the HashSet collapses
        // any duplicates that survived the control-socket-level de-dup.
        let queued_audit_types: std::collections::HashSet<String> = {
            let mut g = pending_audit_runs.lock().unwrap();
            std::mem::take(&mut *g).into_iter().collect()
        };

        drain_chat_and_triage_queues(
            &paths,
            &workspace,
            snapshot_ref,
            executor.as_ref(),
            &github_snap,
            chatops_ctx.as_ref(),
            &pending_triages,
            &pending_proposal_requests,
            &pending_changelog_requests,
        )
        .await;

        drain_oss_and_scout_queues(
            &paths,
            &workspace,
            snapshot_ref,
            executor.as_ref(),
            &github_snap,
            chatops_ctx.as_ref(),
            &pending_brownfield_requests,
            &pending_scout_requests,
            &pending_spec_it_requests,
            &pending_proposal_requests,
        )
        .await;

        drain_sync_survey_batch_queues(
            &paths,
            &workspace,
            snapshot_ref,
            executor.as_ref(),
            &github_snap,
            chatops_ctx.as_ref(),
            &pending_sync_upstream_requests,
            &pending_brownfield_survey_requests,
            &pending_brownfield_batch_requests,
        )
        .await;

        // a71: bind the operator-request-queue handles for the duration of
        // the iteration's work future so `walk_queue` can peek them between
        // changes (via `operator_requests::current()`) and yield the batch
        // when an operator request is pending.
        operator_requests::scope(
            Some(operator_request_queues.clone()),
            run_iteration_work(
                &paths,
                &workspace,
                snapshot_ref,
                executor.as_ref(),
                &github_snap,
                reviewer_snap.as_deref(),
                chatops_ctx.as_ref(),
                want_rebuild,
                &queued_audit_types,
                stuck_threshold_secs,
                perma_stuck_threshold,
                max_changes_per_pr,
                revision_cap,
                human_revise_cap,
                audit_registry.as_ref(),
                audits_cfg.as_deref(),
                audit_settings.as_ref(),
            ),
        )
        .await;

        // The inter-poll sleep uses the snapshot's poll_interval, not a
        // re-read. Next iteration's read picks up any hot-swap during sleep.
        let base_secs = snapshot_ref.poll_interval_sec;
        drop(snapshot);
        // Drop the guard before sleeping so a wipe handler arriving during
        // the sleep short-circuits straight to deletion.
        drop(iter_guard);
        let sleep_dur = jittered_sleep_duration(base_secs, inter_iteration_jitter_pct);

        if let Some(notify) = &hooks.on_iteration_sleep {
            notify.notify_waiters();
        }
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            () = sleep(sleep_dur) => {}
        }
    }

    tracing::info!(url = %repo.load().url, "polling loop exiting");
}

/// Resolve the per-iteration commit cap for the polling task. Mirrors
/// `RepositoryConfig::max_changes_per_pr` but accepts the per-repo and
/// executor-level defaults as separate values so the polling loop can
/// pick up a hot-swapped per-repo override without taking a reference
/// to the live `ExecutorConfig`.
/// OSS-fork support (a26): opportunistic upstream fetch. When the
/// per-repo `upstream` block is configured, ensure a remote named
/// `<upstream.remote>` exists pointing at `<upstream.url>` AND run
/// `git fetch <remote>` with a 30-second timeout. The fetch is
/// best-effort: any error (remote-add failure, fetch timeout, auth
/// failure) logs a WARN naming the failure AND the function returns
/// without affecting the iteration.
fn opportunistic_upstream_fetch(workspace: &Path, repo: &RepositoryConfig) {
    let Some(upstream) = repo.upstream.as_ref() else {
        return;
    };
    if let Err(e) = git::ensure_remote(workspace, &upstream.remote, &upstream.url) {
        tracing::warn!(
            url = %repo.url,
            remote = %upstream.remote,
            upstream_url = %upstream.url,
            "opportunistic upstream remote-management failed: {e:#}; continuing iteration"
        );
        return;
    }
    if let Err(e) = git::fetch_remote_with_timeout(workspace, &upstream.remote, 30) {
        tracing::warn!(
            url = %repo.url,
            remote = %upstream.remote,
            upstream_url = %upstream.url,
            "opportunistic upstream fetch failed: {e:#}; continuing iteration"
        );
    }
}

fn resolve_max_changes_per_pr(per_repo: Option<u32>, executor_default: Option<u32>) -> u32 {
    const DEFAULT: u32 = 3;
    per_repo.or(executor_default).unwrap_or(DEFAULT).max(1)
}

/// Pick a uniformly-random startup-jitter delay in `[0, max_secs]`. A
/// `max_secs` of `0` short-circuits to `0` without consulting the RNG —
/// `gen_range(0..=0)` is well-defined but skipping the draw keeps the
/// degenerate case obvious to readers.
fn pick_startup_jitter_secs(max_secs: u64) -> u64 {
    if max_secs == 0 {
        return 0;
    }
    rand::rng().random_range(0..=max_secs)
}

/// Compute a jittered inter-iteration sleep duration. The offset is
/// drawn uniformly from `[-max_offset, +max_offset]` where `max_offset
/// = base_secs * jitter_pct / 100`. Saturates at zero on the negative
/// side so a degenerate `jitter_pct = 100` cannot underflow. A
/// `jitter_pct = 0` short-circuits to the exact `base_secs` interval
/// (matching pre-jitter behaviour).
fn jittered_sleep_duration(base_secs: u64, jitter_pct: u8) -> Duration {
    if jitter_pct == 0 {
        return Duration::from_secs(base_secs);
    }
    let max_offset = base_secs.saturating_mul(jitter_pct as u64) / 100;
    if max_offset == 0 {
        return Duration::from_secs(base_secs);
    }
    let offset = rand::rng().random_range(0..=2 * max_offset) as i64 - max_offset as i64;
    let secs = (base_secs as i64).saturating_add(offset).max(0) as u64;
    Duration::from_secs(secs)
}

/// Build the per-iteration `ChatOpsContext` from the loaded snapshot.
/// Notification flags + default channel come from the snapshot; per-repo
/// channel override (immutable, sourced from RepositoryConfig) takes
/// precedence over the snapshot's default channel.
fn build_chatops_ctx(repo: &RepositoryConfig, slot: &ChatOpsSlot) -> ChatOpsContext {
    ChatOpsContext {
        chatops: slot.backend.clone(),
        channel: repo.chatops_channel(&slot.default_channel_id).to_string(),
        start_work_enabled: slot.start_work_enabled,
        failure_alerts_enabled: slot.failure_alerts_enabled,
        pr_opened_enabled: slot.pr_opened_enabled,
    }
}

/// Test-only routing hook for the PR-creation HTTP call. When set, the
/// helper below targets the override URL (a mockito server) instead of
/// `github::DEFAULT_API_BASE`. Tests acquire `test_hooks::lock()` before
/// installing the override so two tests cannot race on the process-wide
/// static. Never linked outside `cfg(test)`.
#[cfg(test)]
pub(crate) mod test_hooks {
    use std::sync::{Mutex, MutexGuard, OnceLock};

    static GITHUB_API_BASE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn cell() -> &'static Mutex<Option<String>> {
        GITHUB_API_BASE.get_or_init(|| Mutex::new(None))
    }

    /// Snapshot the currently-installed override URL (or `None`).
    pub fn github_api_base() -> Option<String> {
        cell().lock().unwrap().clone()
    }

    /// Install a PR-creation API-base override for the duration of a
    /// test. The test holds the returned guard until it has finished
    /// reading mockito's recorded calls; on drop the override is cleared.
    pub fn set_github_api_base(value: Option<String>) {
        *cell().lock().unwrap() = value;
    }

    /// Process-wide mutex held by any test that installs the PR-creation
    /// override. Serializes tests that share the static so two concurrent
    /// tests do not clobber each other's override URL.
    pub fn lock<'a>() -> MutexGuard<'a, ()> {
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }
}

#[cfg(test)]
mod tests;
