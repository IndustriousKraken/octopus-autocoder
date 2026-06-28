//! Unix-domain control socket for live daemon interaction. The daemon
//! exposes a `0600`-perm socket at `<system-temp>/autocoder/control/control.sock`
//! and accepts JSON line-delimited requests. The only registered action
//! today is `reload`, which re-reads the YAML config and hot-applies
//! changes to the `github`, `reviewer`, `chatops`, and `repositories`
//! sections. Only the `executor` section requires a process restart.

use crate::alert_state::AlertState;
use crate::busy_marker;
use crate::chatops::ChatOpsBackend;
use crate::chatops::operator_commands::{
    LastIteration, MarkerEntry, RepoStatusResponse, ThrottledAlertEntry,
};
use crate::git;
use crate::github;
use crate::github_credentials;
use crate::code_reviewer::CodeReviewer;
use crate::config::{
    ChatOpsConfig, Config, GithubConfig, NotificationsConfig, RepositoryConfig, ReviewerConfig,
};
use crate::failure_state;
use crate::{queue, workspace};
use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Snapshot of ChatOps runtime state (backend + notification flags +
/// default channel). Held inside an `ArcSwap` so a reload can hot-swap
/// the whole thing atomically; consumers `.load()` once per iteration
/// to read a stable snapshot.
#[derive(Clone)]
pub struct ChatOpsSlot {
    pub backend: Arc<dyn ChatOpsBackend>,
    pub default_channel_id: String,
    pub start_work_enabled: bool,
    pub failure_alerts_enabled: bool,
    pub pr_opened_enabled: bool,
}

pub type GithubHolder = Arc<ArcSwap<GithubConfig>>;
pub type ReviewerHolder = Arc<ArcSwap<Option<Arc<CodeReviewer>>>>;
pub type ChatOpsHolder = Arc<ArcSwap<Option<ChatOpsSlot>>>;
pub type ConfigHolder = Arc<ArcSwap<Config>>;
/// Hot-swappable holder for the workspace-cache config (a65). The reload
/// handler swaps a new `CacheConfig` in; each polling loop reads a
/// snapshot at iteration start so a reload applies the new cap at the
/// next iteration (per the orchestrator-cli hot-reload subset).
pub type CacheHolder = Arc<ArcSwap<crate::config::CacheConfig>>;

/// One in-flight chat-driven proposal-request awaiting triage. The
/// chatops dispatcher's `propose` verb appends to
/// `RepoTaskHandle::pending_proposal_requests`; the polling loop drains
/// it at iteration start (alongside the existing revision-request queue,
/// audit-thread `send it` queue, and on-demand audit queue). The full
/// `ProposalRequestState` lives on disk; this in-memory shape carries
/// only the minimum the polling loop needs to look the state up.
///
/// Most fields mirror the on-disk state file so a caller that only has
/// the in-memory queue entry (no disk read) still has the full request
/// context. The polling loop today only reads `request_id` and then
/// loads the state file for the rest — keeping the other fields here
/// is forward-compat shape, not current consumption.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ProposalRequest {
    pub request_id: String,
    pub channel: String,
    /// Bot's ack-message ts; the request's lifecycle thread.
    pub thread_ts: String,
    pub operator_user: String,
    pub request_text: String,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

/// One in-flight chat-driven changelog-request awaiting stylist run. The
/// chatops dispatcher's `changelog` verb appends to
/// `RepoTaskHandle::pending_changelog_requests`; the polling loop drains
/// it at iteration start. The full `ChangelogRequestState` lives on
/// disk; this in-memory shape carries only what the polling loop needs.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ChangelogRequest {
    pub request_id: String,
    pub repo_url: String,
    pub raw_args: String,
    pub channel: String,
    /// Bot's ack-message ts; the request's lifecycle thread.
    pub lifecycle_thread_ts: String,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

/// One in-flight chat-driven brownfield-request awaiting the
/// brownfield-draft executor pass. The chatops dispatcher's
/// `brownfield` verb appends to
/// `RepoTaskHandle::pending_brownfield_requests`; the polling loop
/// drains it (one request per iteration) after the proposal and
/// changelog drains AND before the standard change-processing pass.
/// The full `BrownfieldRequestState` lives on disk at
/// `<workspace>/.state/brownfield_requests/<request_id>.json`; this
/// in-memory shape carries only what the polling loop needs.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BrownfieldRequest {
    pub request_id: String,
    pub repo_url: String,
    pub capability_name: String,
    pub guidance: Option<String>,
    pub channel: String,
    /// Bot's ack-message ts; the request's lifecycle thread.
    pub thread_ts: String,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

/// One in-flight scout request awaiting the scout-mode executor pass
/// (a25). The chatops dispatcher's `scout` verb appends to
/// `RepoTaskHandle::pending_scout_requests`; the polling loop drains
/// it (one request per iteration) AFTER the brownfield drain AND
/// BEFORE the standard change-processing pass.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ScoutRequest {
    pub request_id: String,
    pub repo_url: String,
    pub guidance: Option<String>,
    pub channel: String,
    /// Bot's ack-message ts; the request's lifecycle thread.
    pub thread_ts: String,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

/// One in-flight spec-it request awaiting translation into a
/// proposal-request (a25). The chatops dispatcher's `spec-it` verb
/// appends to `RepoTaskHandle::pending_spec_it_requests`; the polling
/// loop drains it AFTER scout drains AND BEFORE the proposal-request
/// drain (the spec-it handler internally enqueues a fresh
/// `ProposalRequest`).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SpecItRequest {
    pub repo_url: String,
    pub scout_request_id: String,
    pub item_id: usize,
    pub guidance: Option<String>,
    pub channel: String,
    /// The scout's lifecycle thread ts (status updates land here).
    pub thread_ts: String,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

/// One in-flight sync-upstream request awaiting the polling
/// iteration's handler (a26). The chatops dispatcher's `sync-upstream`
/// verb appends to `RepoTaskHandle::pending_sync_upstream_requests`;
/// the polling loop drains the queue at the start of each iteration.
/// Carries the chatops thread context so the rebase result OR
/// conflict notice can be posted as a follow-up reply.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SyncUpstreamRequest {
    pub request_id: String,
    pub repo_url: String,
    pub channel: String,
    pub thread_ts: String,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

/// One in-flight brownfield-survey request (a29). The chatops
/// dispatcher's `brownfield-survey` verb appends to
/// `RepoTaskHandle::pending_brownfield_survey_requests`; the polling
/// loop drains at most ONE entry per iteration AND runs the survey
/// pass in survey mode (read-only sandbox; JSON-array response). The
/// survey state is NOT persisted by the dispatcher — the survey
/// handler creates `BrownfieldSurveyState` AFTER the executor pass
/// validates.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BrownfieldSurveyRequest {
    pub request_id: String,
    pub repo_url: String,
    pub guidance: Option<String>,
    pub channel: String,
    pub thread_ts: String,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

/// One in-flight brownfield-batch request (a29). The chatops
/// dispatcher's `send it`-in-survey-thread routing pushes here. The
/// polling loop's batch handler loads the referenced
/// `BrownfieldSurveyState`, transitions it to `InProgress`, AND drains
/// one `SurveyItem` per iteration into a single-capability brownfield
/// run.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BrownfieldBatchRequest {
    pub repo_url: String,
    pub survey_request_id: String,
    pub channel: String,
    pub thread_ts: String,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

/// One in-flight spec-revision ADVISOR request (a03). The chatops
/// dispatcher's non-`send it` `@<bot>` reply in a revision thread pushes
/// here via the `revision_advise` control-socket action. The polling loop
/// drains at most ONE per iteration AND runs a read-only agentic session
/// reconstructed from the change deltas, the canon, the marker's
/// contradiction, AND the thread transcript so far — replying in the thread
/// without writing anything. `reply_text` is the operator's current message
/// (the transcript is re-fetched from chat at drain time).
#[derive(Debug, Clone)]
pub struct RevisionAdviseRequest {
    pub repo_url: String,
    pub change_slug: String,
    pub channel: String,
    pub thread_ts: String,
    pub reply_text: String,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

/// One in-flight spec-revision EXECUTOR request (a03). The chatops
/// dispatcher's `@<bot> send it` in a revision thread pushes here via the
/// `revision_execute` control-socket action. The polling loop drains at most
/// ONE per iteration AND runs a write-scoped session that edits the change's
/// spec deltas, re-runs the `[in]` / `[canon]` gates, AND opens a PR on a
/// clean re-gate.
#[derive(Debug, Clone)]
pub struct RevisionExecuteRequest {
    pub repo_url: String,
    pub change_slug: String,
    pub channel: String,
    pub thread_ts: String,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

/// The two revision-thread request queues (a03), bundled so they thread
/// through the polling loop as a single unit AND sit on the
/// [`RepoTaskHandle`] for the control-socket handlers to push onto. Drained
/// one-per-iteration each (mirroring the brownfield-batch drain).
#[derive(Clone)]
pub struct RevisionRequestQueues {
    pub advise: Arc<Mutex<std::collections::VecDeque<RevisionAdviseRequest>>>,
    pub execute: Arc<Mutex<std::collections::VecDeque<RevisionExecuteRequest>>>,
}

impl RevisionRequestQueues {
    pub fn new() -> Self {
        Self {
            advise: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            execute: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        }
    }
}

impl Default for RevisionRequestQueues {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle for a per-repository polling task. The reload handler uses
/// `cancel` to ask one task to exit (without affecting siblings), and
/// `config` to hot-swap the `RepositoryConfig` so a still-running task
/// picks up the new values on its next iteration. `join` is the spawned
/// task's `JoinHandle`; it lets the daemon shutdown path await every
/// per-repo task before exiting.
pub struct RepoTaskHandle {
    pub cancel: CancellationToken,
    pub config: Arc<ArcSwap<RepositoryConfig>>,
    pub join: JoinHandle<()>,
    /// "Run a canonical-spec rebuild at the next iteration" flag. The
    /// control socket's `rebuild_specs` action sets this to `true`; the
    /// polling loop checks + clears it at iteration start (see
    /// `polling_loop::run`).
    pub pending_rebuild: Arc<std::sync::atomic::AtomicBool>,
    /// Queue of `thread_ts` values awaiting audit-triage execution
    /// (`audit-reply-acts`). The chatops dispatcher's `trigger_audit_action`
    /// handler pushes here when an operator posts `@<bot> send it`; the
    /// polling loop drains the queue at the start of each iteration.
    pub pending_triages: Arc<Mutex<Vec<String>>>,
    /// Queue of audit-type names awaiting on-demand execution
    /// (`chatops-on-demand-audit-trigger`). The chatops `audit` verb and
    /// the CLI `audit run` subcommand push canonical audit-type names
    /// onto this list via the `queue_audit` control-socket action; the
    /// polling loop consumes an entry ONLY once it has actually run
    /// (bypassing cadence), so a skipped, early-returning, or bounded-out
    /// pass never drops an acknowledged request. The queue is mirrored to
    /// `<state>/pending-audit-runs/<workspace-basename>.json` on every
    /// mutation (enqueue + post-run prune) AND reloaded at task spawn, so it
    /// also survives a daemon restart (persist-on-demand-audit-queue). Each
    /// entry carries the originating chat context so the scheduler can post a
    /// terminal
    /// completion notification back to the operator. De-duplicated on insert
    /// so multiple `audit` commands collapse to one run.
    pub pending_audit_runs: Arc<Mutex<Vec<crate::polling_loop::QueuedAudit>>>,
    /// Queue of chat-driven proposal requests awaiting triage
    /// (`chat-request-triage`). The chatops dispatcher's `propose` verb
    /// pushes here via the `queue_proposal_request` control-socket
    /// action; the polling loop drains the queue at the start of each
    /// iteration AFTER the revision-loop processing AND the on-demand
    /// audit processing AND BEFORE the pending-change walk. Each entry
    /// keys into the on-disk `ProposalRequestState` file via
    /// `request_id` so a daemon restart between enqueue and drain does
    /// not lose the operator's request.
    pub pending_proposal_requests: Arc<Mutex<Vec<ProposalRequest>>>,
    /// Queue of chat-driven changelog requests awaiting stylist run
    /// (`@<bot> changelog`). The chatops dispatcher's `changelog` verb
    /// pushes here via the `queue_changelog_request` control-socket
    /// action; the polling loop drains the queue at the start of each
    /// iteration AFTER the proposal-request drain AND BEFORE the
    /// pending-change walk. Each entry keys into the on-disk
    /// `ChangelogRequestState` file via `request_id`.
    pub pending_changelog_requests: Arc<Mutex<Vec<ChangelogRequest>>>,
    /// Queue of chat-driven brownfield requests awaiting the
    /// brownfield-draft executor pass (`@<bot> brownfield`). The
    /// chatops dispatcher's `brownfield` verb pushes here via the
    /// `queue_brownfield_request` control-socket action; the polling
    /// loop drains at most ONE entry per iteration AFTER the
    /// proposal/changelog drains AND BEFORE the standard change-
    /// processing pass. Each entry keys into the on-disk
    /// `BrownfieldRequestState` file via `request_id`.
    pub pending_brownfield_requests:
        Arc<Mutex<std::collections::VecDeque<BrownfieldRequest>>>,
    /// Queue of chat-driven scout requests (a25). The chatops
    /// dispatcher's `scout` verb pushes here via the
    /// `queue_scout_request` control-socket action; the polling loop
    /// drains at most ONE entry per iteration AFTER the brownfield
    /// drain AND BEFORE the standard change-processing pass.
    pub pending_scout_requests:
        Arc<Mutex<std::collections::VecDeque<ScoutRequest>>>,
    /// Queue of chat-driven spec-it requests (a25). The chatops
    /// dispatcher's `spec-it` verb pushes here via the
    /// `queue_spec_it_request` control-socket action; the polling
    /// loop drains at most ONE entry per iteration AFTER the scout
    /// drain AND BEFORE the proposal-request drain (the spec-it
    /// handler internally enqueues a fresh `ProposalRequest`).
    pub pending_spec_it_requests:
        Arc<Mutex<std::collections::VecDeque<SpecItRequest>>>,
    /// Queue of chat-driven sync-upstream requests (a26). The chatops
    /// dispatcher's `sync-upstream` verb pushes here via the
    /// `queue_sync_upstream_request` control-socket action; the
    /// polling loop drains the queue at the start of each iteration
    /// (FIFO). Each entry carries its own request_id so duplicate
    /// queueing within a single window is deduplicated on insert.
    pub pending_sync_upstream_requests:
        Arc<Mutex<std::collections::VecDeque<SyncUpstreamRequest>>>,
    /// Queue of chat-driven brownfield-survey requests (a29). The
    /// chatops dispatcher's `brownfield-survey` verb pushes here via
    /// the `queue_brownfield_survey_request` control-socket action;
    /// the polling loop drains at most ONE entry per iteration.
    pub pending_brownfield_survey_requests:
        Arc<Mutex<std::collections::VecDeque<BrownfieldSurveyRequest>>>,
    /// Queue of chat-driven brownfield-batch requests (a29). The
    /// chatops dispatcher's `send it`-in-survey-thread routing pushes
    /// here via the `queue_brownfield_batch_request` control-socket
    /// action; the polling loop drains at most ONE entry per iteration
    /// to flip the referenced survey into `InProgress`. Once in
    /// progress, the survey's items drain one-per-iteration through
    /// the batch handler.
    pub pending_brownfield_batch_requests:
        Arc<Mutex<std::collections::VecDeque<BrownfieldBatchRequest>>>,
    /// Queues of chat-driven spec-revision thread requests (a03). The
    /// chatops dispatcher's `revision_advise` action (a non-`send it` reply
    /// in a revision thread) pushes onto `advise`; its `revision_execute`
    /// action (`send it` in a revision thread) pushes onto `execute`. The
    /// polling loop drains at most ONE of EACH per iteration AFTER the
    /// brownfield-batch drain.
    pub pending_revision_requests: RevisionRequestQueues,
    /// Per-iteration cancel token. The polling loop populates this with
    /// a child of the global cancel at iteration start and clears it
    /// back to `None` at iteration end (via an `IterationGuard` drop).
    /// The `wipe_workspace` control-socket handler fires it to ask the
    /// in-flight iteration to drain cleanly before the workspace is
    /// deleted; firing it does NOT cancel the per-repo polling task —
    /// only the current iteration body sees the cancellation.
    pub iteration_cancel: Arc<Mutex<Option<CancellationToken>>>,
    /// Per-repo `Notify` that fires every time the iteration's per-
    /// iteration cleanup runs. The `wipe_workspace` handler awaits this
    /// after firing `iteration_cancel` so the wipe can run on a quiet
    /// workspace instead of yanking the directory out from under a
    /// live executor subprocess.
    pub iteration_drained: Arc<Notify>,
}

/// Daemon-level task registry keyed by repository URL. Mutated only by
/// the reload handler (add/cancel/swap-in-place) and by each polling
/// task's exit hook (remove-self).
pub type RepoTaskMap = Arc<Mutex<HashMap<String, RepoTaskHandle>>>;

/// Outcome of asking the runtime to spawn a polling task for a new repo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnOutcome {
    /// A new task was spawned and inserted into the map.
    Spawned,
    /// The URL is already present in the task map (either a live task or
    /// a still-shutting-down one). Caller treats this as "unchanged" so
    /// the response shape stays accurate.
    AlreadyPresent,
    /// The repository's startup check failed (workspace init, dirty
    /// recovery, etc.). The task was not spawned.
    StartupCheckFailed,
}

/// Closure-style hook for spawning a polling task at reload time. The
/// daemon's `execute` function constructs one of these in `cli/run.rs`,
/// capturing the executor, holders, cancellation parent, and thresholds.
/// The reload handler invokes it for every URL it has decided to add.
pub type SpawnRepoFn =
    Arc<dyn Fn(RepositoryConfig) -> SpawnOutcome + Send + Sync>;

/// Handles the control socket task needs to mutate live config + read
/// disk. Constructed once at startup and shared with the listener task.
#[derive(Clone)]
pub struct ControlState {
    pub github: GithubHolder,
    pub reviewer: ReviewerHolder,
    pub chatops: ChatOpsHolder,
    /// Hot-swappable workspace-cache config (a65). The reload handler
    /// swaps a new `CacheConfig` in here; the same holder is shared with
    /// every polling task so a reload's new `workspaces_max_gb` cap takes
    /// effect at the next iteration.
    pub cache: CacheHolder,
    /// The most recently parsed-and-applied `Config`. Reload diffs
    /// against this snapshot; on a successful reload, the snapshot is
    /// swapped to the new value.
    pub last_config: ConfigHolder,
    pub config_path: PathBuf,
    /// Registry of running per-repo polling tasks, keyed by URL.
    pub repo_tasks: RepoTaskMap,
    /// Notify that fires every time `repo_tasks` is mutated (insert /
    /// remove). Both the production spawn closure and the test fixtures
    /// notify after their map writes so consumers can wait on map state
    /// changes without sleep-polling.
    pub repo_tasks_changed: Arc<Notify>,
    /// Factory the reload handler uses to spawn a polling task for a
    /// newly-added repository. Captured at daemon startup so the reload
    /// handler doesn't need direct access to executor/holders.
    pub spawn_repo: SpawnRepoFn,
    /// Per-workspace canonical-spec RAG registry (a21). Populated by
    /// the polling loop's workspace-init step when `canonical_rag` is
    /// enabled. The `query_canonical_specs` action looks up the
    /// workspace by sanitized basename and dispatches against the
    /// store; an absent basename is fail-open (empty hits + hint).
    pub canonical_rag_registry: crate::rag::CanonicalRagRegistry,
    /// Execution-scoped outcome store (a27a0). Populated by the
    /// `record_outcome` action when the per-execution MCP child
    /// relays an outcome tool call; drained by the `consume_outcome`
    /// action when the executor's classifier runs after subprocess
    /// exit. Keyed by `(workspace_basename, change)`.
    pub outcome_store: crate::outcome_store::OutcomeStore,
    /// Execution-scoped structured-submission store (a56). Populated by the
    /// `record_submission` action when the per-execution MCP child relays a
    /// role's `submit_*` tool call; drained by the `consume_submission`
    /// action when the role's daemon-side caller runs after subprocess
    /// exit. Keyed by `(workspace_basename, change)`. The per-role payload
    /// schemas are registered by the changes that add each `submit_*` tool.
    pub submission_store: crate::submission_store::SubmissionStore,
    /// Daemon-wide resolved `DaemonPaths`, threaded from the entrypoint
    /// per the canonical `Production paths SHALL be threaded` requirement
    /// (constructor-field pattern). Every handler that resolves a
    /// state/cache/logs/runtime path uses this reference instead of a
    /// process-global.
    pub paths: Arc<crate::paths::DaemonPaths>,
}

/// Canonical control-socket path: `<runtime_dir>/control.sock`. The
/// runtime dir is resolved from the daemon's `DaemonPaths` (typically
/// `/run/autocoder/` under systemd or `${XDG_RUNTIME_DIR}/autocoder/`
/// in dev mode); reboot-cleared tmpfs is the correct location for a
/// socket that should never outlive the process that owns it.
pub fn socket_path(paths: &crate::paths::DaemonPaths) -> PathBuf {
    paths.control_socket_path()
}

/// Guard returned by [`spawn_submission_listener`]. Holds the
/// `CancellationToken` for the in-process control-socket listener; on
/// `Drop` it fires the token, which stops `serve` (the accept loop) and
/// removes the bound socket file. Hold it for the lifetime of the gate or
/// audit run that needs the submission transport; drop it to tear the
/// transport down. The control-socket env var is set by
/// `spawn_submission_listener` and is NOT cleared on drop (the process is
/// exiting in every caller; clearing it would be a cross-thread env race).
pub struct SubmissionListenerGuard {
    cancel: CancellationToken,
    /// The bound socket path, exposed so callers can confirm cleanup in
    /// tests AND so the env var the gates read points at exactly this path.
    socket: PathBuf,
    /// Join handle for the spawned `serve` task; awaited on drop is not
    /// possible (Drop is sync), so we only cancel — `serve` removes the
    /// socket file as it unwinds.
    _handle: JoinHandle<()>,
}

impl SubmissionListenerGuard {
    /// The path of the bound control socket. Equals the value the
    /// control-socket env var is set to for the duration of the guard.
    pub fn socket_path(&self) -> &Path {
        &self.socket
    }
}

impl Drop for SubmissionListenerGuard {
    fn drop(&mut self) {
        // Fire the token; `serve` observes it, breaks its accept loop, and
        // removes the socket file. The spawned task is detached — we do not
        // (and cannot, in a sync Drop) await it; cancellation is sufficient
        // for the per-invocation lifecycle.
        self.cancel.cancel();
    }
}

/// Build a `ControlState` that carries ONLY what the submission-relay
/// handlers (`record_submission` / `consume_submission`) need: the
/// `submission_store` and `paths`. Every other field is a cheap, inert
/// placeholder — the in-process listener handles ONLY submission traffic
/// from a gate/audit's MCP child, so the github/reviewer/chatops/reload
/// handlers are never reached. Used by [`spawn_submission_listener`].
fn submission_only_state(
    paths: Arc<crate::paths::DaemonPaths>,
    submission_store: crate::submission_store::SubmissionStore,
) -> ControlState {
    // GithubConfig + CacheConfig are fully serde-defaulted / Default;
    // Config requires repositories/executor/github, supplied here as the
    // empty-repos placeholder. None of these are read on the submission
    // path; they exist only to satisfy the struct.
    let github: GithubConfig =
        serde_json::from_value(serde_json::json!({})).expect("GithubConfig is fully serde-defaulted");
    let placeholder_cfg = Config {
        repositories: Vec::new(),
        executor: crate::config::placeholder_executor_config(),
        github: github.clone(),
        reviewer: None,
        chatops: None,
        audits: None,
        paths: crate::config::DaemonPathsConfig::default(),
        cache: crate::config::CacheConfig::default(),
        features: crate::config::FeaturesConfig::default(),
        canonical_rag: None,
        models: None,
        journal_log: None,
    };
    // A no-op spawn closure: the reload handler is never invoked on the
    // submission path, so this returning `StartupCheckFailed` is unreachable.
    let spawn_repo: SpawnRepoFn =
        Arc::new(|_repo: RepositoryConfig| SpawnOutcome::StartupCheckFailed);
    ControlState {
        github: Arc::new(ArcSwap::from_pointee(github)),
        reviewer: Arc::new(ArcSwap::from_pointee(None)),
        chatops: Arc::new(ArcSwap::from_pointee(None)),
        cache: Arc::new(ArcSwap::from_pointee(crate::config::CacheConfig::default())),
        last_config: Arc::new(ArcSwap::from_pointee(placeholder_cfg)),
        config_path: PathBuf::new(),
        repo_tasks: Arc::new(Mutex::new(HashMap::new())),
        repo_tasks_changed: Arc::new(Notify::new()),
        spawn_repo,
        canonical_rag_registry: crate::rag::CanonicalRagRegistry::new(),
        outcome_store: crate::outcome_store::OutcomeStore::new(),
        submission_store,
        paths,
    }
}

/// Register the gate AND advisory-audit `submit_*` payload schemas on
/// `store`. This is the schema set every submission-based gate or audit
/// validates against; it is the SINGLE source the daemon startup AND the
/// in-process [`spawn_submission_listener`] both call (the daemon
/// additionally registers the reviewer's and `[out]` gate's schemas, which
/// `verify` / standalone-audit do not exercise).
pub fn register_gate_and_audit_submission_schemas(
    store: &crate::submission_store::SubmissionStore,
) {
    // The advisory audits' `submit_findings` schema.
    crate::audits::register_submission_schemas(store);
    // The `[in]` gate's `submit_contradictions` schema.
    crate::preflight::change_contradiction::register_contradiction_submission_schema(store);
    // The `[canon]` gate's `submit_canon_contradictions` schema.
    crate::preflight::canon_contradiction::register_canon_contradiction_submission_schema(store);
    // The `[rules]` gate's `submit_rule_violations` schema.
    crate::preflight::global_rules::register_rule_violations_submission_schema(store);
}

/// Stand up the in-process submission transport for a single invocation,
/// so an agentic gate or submission-based audit can capture its
/// `submit_*` verdict over the control socket WITHOUT a running daemon.
///
/// This is the SINGLE source of the bootstrap sequence the daemon performs
/// inline at startup. In order it: constructs a
/// [`crate::submission_store::SubmissionStore`]; registers the gate
/// submission schemas (`register_contradiction_submission_schema`,
/// `register_canon_contradiction_submission_schema`,
/// `register_rule_violations_submission_schema`) AND the audit submission
/// schemas (`crate::audits::register_submission_schemas`); binds the
/// control socket at [`socket_path`]; sets the control-socket env var
/// ([`crate::mcp_askuser_server::ENV_CONTROL_SOCKET`]) to the bound path;
/// spawns [`serve`] on a submission-only [`ControlState`]; AND returns a
/// [`SubmissionListenerGuard`] whose `Drop` cancels the listener (stopping
/// `serve`, which removes the socket file).
///
/// Three callers: the daemon entry point (`cli/run.rs`), the `verify`
/// subcommand (`cli/verify.rs`), AND the standalone audit path
/// (`cli/audit.rs::run_standalone`). Without the listener, an agentic gate
/// or submission-based audit drains `None` from `try_consume_submission`
/// and fails closed; with it, verdicts are captured exactly as under the
/// daemon.
pub fn spawn_submission_listener(
    paths: &crate::paths::DaemonPaths,
) -> Result<SubmissionListenerGuard> {
    let submission_store = crate::submission_store::SubmissionStore::new();
    // Register the gate AND advisory-audit submission schemas (single source).
    register_gate_and_audit_submission_schemas(&submission_store);

    let path = socket_path(paths);
    let listener = bind_at(&path)?;
    // SAFETY: the callers (verify / standalone audit / daemon startup) set
    // this env var before spawning any agentic session that reads it; in
    // the verify/audit CLI paths the process is single-threaded at this
    // point. The guard intentionally does NOT clear it on drop (the
    // process exits; a clear would race other threads).
    unsafe {
        std::env::set_var(
            crate::mcp_askuser_server::ENV_CONTROL_SOCKET,
            path.as_os_str(),
        );
    }

    let cancel = CancellationToken::new();
    let state = submission_only_state(Arc::new(paths.clone()), submission_store);
    let serve_cancel = cancel.clone();
    let serve_path = path.clone();
    let handle: JoinHandle<()> = tokio::spawn(async move {
        if let Err(e) = serve(listener, serve_path, state, serve_cancel).await {
            tracing::error!("in-process submission listener exited: {e:#}");
        }
    });

    Ok(SubmissionListenerGuard {
        cancel,
        socket: path,
        _handle: handle,
    })
}

/// Bind the listener at the canonical socket path and accept connections
/// until `cancel` fires. Removes the socket file on shutdown.
pub async fn listen(state: ControlState, cancel: CancellationToken) -> Result<()> {
    let path = socket_path(&state.paths);
    listen_at(path, state, cancel).await
}

/// Same as `listen` but binds at an explicit path. Used by tests so
/// parallel runs don't collide on the canonical path.
pub async fn listen_at(
    path: PathBuf,
    state: ControlState,
    cancel: CancellationToken,
) -> Result<()> {
    let listener = bind_at(&path)?;
    serve(listener, path, state, cancel).await
}

/// Bind a `UnixListener` at `path` (creating the parent directory and
/// removing any stale socket file first). Returns synchronously once the
/// listener is ready to accept, so test callers can spawn `serve` and
/// know — without polling — that the socket is live.
pub fn bind_at(path: &Path) -> Result<UnixListener> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("creating control-socket directory {}", parent.display())
        })?;
    }
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    let listener = UnixListener::bind(path)
        .with_context(|| format!("binding control socket at {}", path.display()))?;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        tracing::warn!(
            "could not chmod control socket {} to 0600: {e}",
            path.display()
        );
    }
    tracing::info!("control socket listening at {}", path.display());
    Ok(listener)
}

/// Run the accept loop against an already-bound `listener` until `cancel`
/// fires. Removes the socket file on shutdown.
pub async fn serve(
    listener: UnixListener,
    path: PathBuf,
    state: ControlState,
    cancel: CancellationToken,
) -> Result<()> {
    let state = Arc::new(state);
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                tracing::info!("control socket: cancellation received; shutting down");
                break;
            }
            res = listener.accept() => {
                match res {
                    Ok((stream, _addr)) => {
                        let state = state.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, state).await {
                                tracing::warn!("control-socket connection failed: {e:#}");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!("control-socket accept failed: {e}");
                    }
                }
            }
        }
    }
    if let Err(e) = std::fs::remove_file(&path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(
                "failed to remove control socket {} on shutdown: {e}",
                path.display()
            );
        }
    }
    Ok(())
}

async fn handle_connection(stream: UnixStream, state: Arc<ControlState>) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    match reader.read_line(&mut line).await {
        Ok(0) => return Ok(()),
        Ok(_) => {}
        Err(e) => {
            let resp = json!({"ok": false, "error": format!("read failed: {e}")});
            let _ = write_response(&mut write_half, &resp).await;
            return Ok(());
        }
    }
    let response = dispatch_request(&line, state.as_ref()).await;
    write_response(&mut write_half, &response).await?;
    Ok(())
}

async fn write_response(
    write_half: &mut tokio::net::unix::OwnedWriteHalf,
    response: &Value,
) -> Result<()> {
    let mut bytes = serde_json::to_vec(response).unwrap_or_else(|_| b"{}".to_vec());
    bytes.push(b'\n');
    write_half.write_all(&bytes).await?;
    write_half.shutdown().await?;
    Ok(())
}

/// One control-socket action handler, normalized to a single shape so the
/// dispatch table can hold sync AND async handlers (and handlers that ignore
/// the parsed request) side by side. Each table entry adapts its underlying
/// `handle_*` function into this signature: an async handler is boxed
/// directly, a sync handler is wrapped in an `async` block, AND a handler that
/// takes only `&ControlState` simply ignores the parsed argument.
type Handler = for<'a> fn(
    &'a Value,
    &'a ControlState,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Value> + Send + 'a>>;

/// Action-string → handler table (replaces the former hand-maintained
/// `match action.as_str()` arm list). Adding an action is now a single row
/// here plus the handler itself; the router below is a flat lookup. Order is
/// irrelevant to behaviour — an unmatched action falls through to the same
/// `unknown action: <action>` reply the match produced.
const DISPATCH: &[(&str, Handler)] = &[
    ("reload", |_p, s| Box::pin(handle_reload(s))),
    ("repo_status", |p, s| Box::pin(handle_repo_status(p, s))),
    ("repo_status_all", |_p, s| Box::pin(handle_repo_status_all(s))),
    ("clear_perma_stuck_marker", |p, s| {
        Box::pin(async move { handle_clear_perma_stuck(p, s) })
    }),
    ("clear_revision_marker", |p, s| {
        Box::pin(async move { handle_clear_revision(p, s) })
    }),
    ("ignore_for_queue_marker", |p, s| {
        Box::pin(async move { handle_ignore_for_queue(p, s) })
    }),
    ("clear_ignore_for_queue_marker", |p, s| {
        Box::pin(async move { handle_clear_ignore_for_queue(p, s) })
    }),
    ("prioritize", |p, s| {
        Box::pin(async move { handle_prioritize(p, s) })
    }),
    ("wipe_workspace", |p, s| Box::pin(handle_wipe_workspace(p, s))),
    ("rebuild_specs", |p, s| Box::pin(handle_rebuild_specs(p, s))),
    ("trigger_audit_action", |p, s| {
        Box::pin(handle_trigger_audit_action(p, s))
    }),
    ("queue_audit", |p, s| {
        Box::pin(async move { handle_queue_audit(p, s) })
    }),
    ("queue_proposal_request", |p, s| {
        Box::pin(async move { handle_queue_proposal_request(p, s) })
    }),
    ("queue_changelog_request", |p, s| {
        Box::pin(async move { handle_queue_changelog_request(p, s) })
    }),
    ("queue_brownfield_request", |p, s| {
        Box::pin(async move { handle_queue_brownfield_request(p, s) })
    }),
    ("queue_scout_request", |p, s| {
        Box::pin(async move { handle_queue_scout_request(p, s) })
    }),
    ("queue_spec_it_request", |p, s| {
        Box::pin(async move { handle_queue_spec_it_request(p, s) })
    }),
    ("queue_clear_scout", |p, s| {
        Box::pin(async move { handle_queue_clear_scout(p, s) })
    }),
    ("queue_sync_upstream_request", |p, s| {
        Box::pin(async move { handle_queue_sync_upstream_request(p, s) })
    }),
    ("queue_brownfield_survey_request", |p, s| {
        Box::pin(async move { handle_queue_brownfield_survey_request(p, s) })
    }),
    ("queue_brownfield_batch_request", |p, s| {
        Box::pin(async move { handle_queue_brownfield_batch_request(p, s) })
    }),
    ("revision_advise", |p, s| {
        Box::pin(async move { handle_revision_advise(p, s) })
    }),
    ("revision_execute", |p, s| {
        Box::pin(async move { handle_revision_execute(p, s) })
    }),
    ("queue_clear_survey", |p, s| {
        Box::pin(async move { handle_queue_clear_survey(p, s) })
    }),
    ("promote_issue_candidate", |p, s| {
        Box::pin(async move { handle_promote_issue_candidate(p, s) })
    }),
    ("review_target", |p, s| Box::pin(handle_review_target(p, s))),
    ("recent_commits_log", |p, s| {
        Box::pin(async move { handle_recent_commits_log(p, s) })
    }),
    ("survival_analysis", |p, s| {
        Box::pin(async move { handle_survival_analysis(p, s) })
    }),
    ("provenance_lookup", |p, s| {
        Box::pin(async move { handle_provenance_lookup(p, s) })
    }),
    ("rollback_recovery", |p, s| {
        Box::pin(handle_rollback_recovery(p, s))
    }),
    ("defer_unit", |p, s| Box::pin(handle_defer_unit(p, s))),
    ("undefer_unit", |p, s| Box::pin(handle_undefer_unit(p, s))),
    ("query_canonical_specs", |p, s| {
        Box::pin(handle_query_canonical_specs(p, s))
    }),
    ("record_outcome", |p, s| {
        Box::pin(async move { handle_record_outcome(p, s) })
    }),
    ("consume_outcome", |p, s| {
        Box::pin(async move { handle_consume_outcome(p, s) })
    }),
    ("record_submission", |p, s| {
        Box::pin(async move { handle_record_submission(p, s) })
    }),
    ("record_advertised_tool", |p, s| {
        Box::pin(async move { handle_record_advertised_tool(p, s) })
    }),
    ("consume_submission", |p, s| {
        Box::pin(async move { handle_consume_submission(p, s) })
    }),
];

pub async fn dispatch_request(line: &str, state: &ControlState) -> Value {
    let parsed: Value = match serde_json::from_str(line.trim()) {
        Ok(v) => v,
        Err(e) => {
            return json!({"ok": false, "error": format!("malformed JSON: {e}")});
        }
    };
    let action = match parsed.get("action").and_then(|a| a.as_str()) {
        Some(a) => a.to_string(),
        None => {
            return json!({"ok": false, "error": "malformed request: missing `action` field"});
        }
    };
    match DISPATCH.iter().find(|(name, _)| *name == action.as_str()) {
        Some((_, handler)) => handler(&parsed, state).await,
        None => json!({"ok": false, "error": format!("unknown action: {action}")}),
    }
}

// =====================================================================
// Operator-command action handlers
// =====================================================================

/// Look up the configured repository whose `url` matches `url_arg`. Errors
/// when the URL is unknown to the daemon.
fn find_repo(state: &ControlState, url_arg: &str) -> std::result::Result<RepositoryConfig, String> {
    let cfg = state.last_config.load_full();
    cfg.repositories
        .iter()
        .find(|r| r.url == url_arg)
        .cloned()
        .ok_or_else(|| format!("no repository configured with url `{url_arg}`"))
}

/// Look up the configured repository whose `url` exactly matches OR
/// case-insensitively contains `selector` (a59). Mirrors the chatops
/// `match_repo` selector so the on-demand `review_target` action resolves a
/// repo identically whether the caller already resolved an exact URL (the
/// chatops dispatcher) OR passed a raw substring (the `autocoder review`
/// CLI). An exact-URL match is preferred; otherwise a unique substring match
/// resolves; zero or multiple matches return a clear error.
fn find_repo_by_substring(
    state: &ControlState,
    selector: &str,
) -> std::result::Result<RepositoryConfig, String> {
    let cfg = state.last_config.load_full();
    // Exact URL match first (the chatops path submits the resolved URL).
    if let Some(r) = cfg.repositories.iter().find(|r| r.url == selector) {
        return Ok(r.clone());
    }
    let needle = selector.to_ascii_lowercase();
    let matches: Vec<&RepositoryConfig> = cfg
        .repositories
        .iter()
        .filter(|r| r.url.to_ascii_lowercase().contains(&needle))
        .collect();
    match matches.len() {
        0 => Err(format!(
            "no configured repository matches `{selector}` (by URL or substring)"
        )),
        1 => Ok(matches[0].clone()),
        _ => {
            let urls: Vec<&str> = matches.iter().map(|r| r.url.as_str()).collect();
            Err(format!(
                "`{selector}` matched {} repositories: {}. Use a more specific substring.",
                urls.len(),
                urls.join(", ")
            ))
        }
    }
}

/// Look up the configured repository whose resolved workspace path
/// matches `target` (after canonicalisation when both paths exist). Used
/// by the `queue_audit` action's CLI path so an operator can pass
/// `--workspace <path>` instead of the upstream URL.
fn find_repo_by_workspace(state: &ControlState, target: &Path) -> Option<String> {
    let cfg = state.last_config.load_full();
    let target_canon = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    for repo in cfg.repositories.iter() {
        let ws = workspace::resolve_path(&state.paths, repo);
        if ws == target {
            return Some(repo.url.clone());
        }
        let ws_canon = std::fs::canonicalize(&ws).unwrap_or_else(|_| ws);
        if ws_canon == target_canon {
            return Some(repo.url.clone());
        }
    }
    None
}

/// Render the comma-separated list of `url@workspace_path` pairs the
/// daemon is currently managing. Used in error replies so the operator
/// sees their configured repos.
fn managed_repo_list_for_error(state: &ControlState) -> String {
    let cfg = state.last_config.load_full();
    if cfg.repositories.is_empty() {
        return "(none)".to_string();
    }
    cfg.repositories
        .iter()
        .map(|r| {
            format!(
                "`{}` @ `{}`",
                r.url,
                workspace::resolve_path(&state.paths, r).display()
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn require_str(parsed: &Value, field: &str) -> std::result::Result<String, String> {
    parsed
        .get(field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("missing `{field}` field"))
}

/// Outcome of [`preempt_and_acquire_busy_marker`] on success: the held
/// busy-marker guard the caller keeps alive for the WHOLE workspace
/// operation (its `Drop` releases the marker), plus the slug of the
/// change the cancelled pass was working — `None` when no pass was in
/// flight. The caller surfaces `preempted_change` to the operator (the
/// chatops preempt acknowledgement) and otherwise just holds the guard.
pub struct PreemptAcquireOutcome {
    pub guard: busy_marker::BusyGuard,
    pub preempted_change: Option<String>,
}

/// Failure modes of [`preempt_and_acquire_busy_marker`]. `Busy` means the
/// marker could not be acquired — either it is ambiguous (PID alive,
/// PID-reuse suspected) so we refuse to barge in, or it was still held
/// after the bounded preempt wait. The caller surfaces this as a clear
/// "repo busy with an unrecognized holder; investigate" error and does
/// NOT mutate the workspace. `Internal` carries an unexpected error from
/// the underlying `try_acquire` filesystem path.
#[derive(Debug)]
pub enum PreemptAcquireError {
    Busy(String),
    Internal(String),
}

/// Abstraction over the one OS effect this helper has that we do not
/// want to fire for real in a unit test: sending `SIGTERM` to the
/// in-flight executor's process group. Production uses [`RealSignaller`]
/// (`libc::killpg`); tests inject a recording fake so no real process is
/// signalled. The drain-coordination half (`iteration_cancel`) and the
/// marker filesystem half run unmocked in both — only the kill is gated.
pub trait PreemptSignaller: Send + Sync {
    /// Send `SIGTERM` to the process group `pgid`. Best-effort: a failure
    /// is logged but never aborts the preempt (the bounded marker-release
    /// wait + clean-base preamble recover regardless).
    fn sigterm_pgid(&self, pgid: i32);
}

/// Production signaller: `killpg(pgid, SIGTERM)`, mirroring the
/// `--immediate` spec-rebuild coordination path
/// (`cli/sync_specs.rs`).
pub struct RealSignaller;

impl PreemptSignaller for RealSignaller {
    fn sigterm_pgid(&self, pgid: i32) {
        let rc = unsafe { libc::killpg(pgid as libc::pid_t, libc::SIGTERM) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            tracing::warn!(
                pgid,
                "preempt: SIGTERM to executor process group failed: {err}"
            );
        }
    }
}

/// Preempt an in-flight pass on `repo` and acquire the per-repo busy
/// marker, holding it for the whole subsequent workspace-mutating
/// operation. This is the shared primitive every workspace-mutating
/// control-socket handler uses to conform to the invariant: a workspace
/// op MUST NOT run concurrently with a pass on the same repo, AND MUST
/// preempt the in-flight pass rather than wait for it.
///
/// Composed from the two existing halves (no new signal invented):
///   1. Drain coordination via the per-repo `RepoTaskHandle`'s
///      `iteration_cancel` token (as `handle_wipe_workspace` uses). This
///      makes the pass body observe cancellation at its next await — but
///      it does NOT terminate a long-running executor child.
///   2. SIGTERM to the executor child via the busy-marker subprocess
///      sidecar (as `coordinate_with_daemon`'s `--immediate` path and
///      the busy-marker stuck-recovery do). This is the half that
///      actually stops the child writing the workspace AND stops it
///      spending tokens; a SIGTERM'd executor classifies as ABORTED and
///      opens NO PR.
///
/// After signalling, it waits — bounded by `executor.wipe_drain_timeout_secs`
/// (the SAME single drain/preempt timeout the wipe uses; no new knob) —
/// for the marker file to release, then `busy_marker::try_acquire`s it.
/// On `SkipAmbiguous` (PID-reuse suspected) it surfaces a clear `Busy`
/// error rather than barging in. When no pass is in flight (no handle +
/// no sidecar), it skips the preempt and acquires directly.
pub async fn preempt_and_acquire_busy_marker(
    state: &ControlState,
    repo: &RepositoryConfig,
    workspace: &Path,
) -> std::result::Result<PreemptAcquireOutcome, PreemptAcquireError> {
    preempt_and_acquire_busy_marker_with(
        state,
        repo,
        workspace,
        &RealSignaller,
        &busy_marker::RealProcessOps,
    )
    .await
}

/// FORCEFUL variant of [`preempt_and_acquire_busy_marker`] for the CONFIRMED,
/// operator-acknowledged destructive rollback. After the SAME polite preempt
/// (iteration cancel + `PreemptSignaller` SIGTERM + bounded marker-release
/// wait), this ESCALATES on a still-held (`SkipFreshInProgress`) OR
/// PID-reuse-suspected (`SkipAmbiguous`) marker instead of returning `Busy`: it
/// drives the busy-marker forced reclaim (`busy_marker::force_reclaim` —
/// SIGKILL the process group, clear the marker file + subprocess sidecar) and
/// re-acquires. The reclaim fires regardless of marker age — the operator's
/// confirmation, not `stuck_threshold_secs`, is the authority.
///
/// Terminal outcomes are ONLY `Acquired` (possibly via the escalation) OR
/// `Internal` (a real filesystem error). It NEVER returns `Busy`. The polite
/// preempt the non-destructive ops use is unchanged.
pub async fn preempt_and_force_acquire_busy_marker(
    state: &ControlState,
    repo: &RepositoryConfig,
    workspace: &Path,
) -> std::result::Result<PreemptAcquireOutcome, PreemptAcquireError> {
    preempt_and_acquire_busy_marker_inner(
        state,
        repo,
        workspace,
        &RealSignaller,
        &busy_marker::RealProcessOps,
        true,
    )
    .await
}

/// Test-injectable forceful variant. Mirrors
/// [`preempt_and_acquire_busy_marker_with`] but escalates to a forced reclaim
/// on a stuck/ambiguous marker (the confirmed-rollback behavior).
#[cfg(test)]
pub async fn preempt_and_force_acquire_busy_marker_with(
    state: &ControlState,
    repo: &RepositoryConfig,
    workspace: &Path,
    signaller: &dyn PreemptSignaller,
    ops: &(dyn busy_marker::ProcessOps + Send + Sync),
) -> std::result::Result<PreemptAcquireOutcome, PreemptAcquireError> {
    preempt_and_acquire_busy_marker_inner(state, repo, workspace, signaller, ops, true).await
}

/// Test-injectable variant of [`preempt_and_acquire_busy_marker`]. The
/// `signaller` seam lets unit tests record the SIGTERM target without
/// killing a real process group; the `ops` seam lets them drive the
/// busy-marker PID-liveness / comm classification (e.g. to produce the
/// `SkipAmbiguous` outcome deterministically on any platform).
pub async fn preempt_and_acquire_busy_marker_with(
    state: &ControlState,
    repo: &RepositoryConfig,
    workspace: &Path,
    signaller: &dyn PreemptSignaller,
    ops: &(dyn busy_marker::ProcessOps + Send + Sync),
) -> std::result::Result<PreemptAcquireOutcome, PreemptAcquireError> {
    preempt_and_acquire_busy_marker_inner(state, repo, workspace, signaller, ops, false).await
}

/// Shared body for the polite ([`preempt_and_acquire_busy_marker_with`]) AND
/// forceful ([`preempt_and_force_acquire_busy_marker_with`]) preempt-and-acquire
/// paths. `forceful == false` is the polite preempt the non-destructive ops use
/// (maps a still-held / ambiguous marker to `Busy`). `forceful == true` is the
/// confirmed-rollback escalation: on a still-held / ambiguous marker it drives
/// `busy_marker::force_reclaim` (SIGKILL + clear) and re-acquires, so it NEVER
/// returns `Busy`.
async fn preempt_and_acquire_busy_marker_inner(
    state: &ControlState,
    repo: &RepositoryConfig,
    workspace: &Path,
    signaller: &dyn PreemptSignaller,
    ops: &(dyn busy_marker::ProcessOps + Send + Sync),
    forceful: bool,
) -> std::result::Result<PreemptAcquireOutcome, PreemptAcquireError> {
    let cfg = state.last_config.load_full();
    let stale_threshold_secs = cfg.executor.busy_marker_stale_threshold_secs();

    // (1) Read the marker's currently-worked change BEFORE preempting, so
    // the caller can name the cancelled change in the chatops
    // acknowledgement. An empty/absent change → None.
    let preempted_change =
        busy_marker::current_with(&state.paths, workspace, stale_threshold_secs, ops).and_then(
            |summary| {
                let change = summary.change.trim().to_string();
                if change.is_empty() { None } else { Some(change) }
            },
        );

    // (2) Preempt the in-flight pass. Look up the per-repo handle's
    // iteration_cancel token under the briefest lock (mirror
    // handle_wipe_workspace). Fire it so the pass body drains at its next
    // await; then SIGTERM the executor child via the sidecar so the
    // running child actually stops writing the workspace AND opening a PR.
    let iter_token: Option<CancellationToken> = {
        let guard = state.repo_tasks.lock().unwrap();
        guard
            .get(&repo.url)
            .and_then(|h| h.iteration_cancel.lock().unwrap().clone())
    };
    let sidecar_pid = busy_marker::read_subprocess_marker(&state.paths, workspace);

    let pass_in_flight = iter_token.is_some() || sidecar_pid.is_some();
    if pass_in_flight {
        if let Some(token) = &iter_token {
            token.cancel();
        }
        match sidecar_pid {
            Some(pid) if pid > 0 => {
                tracing::info!(
                    url = %repo.url,
                    pgid = pid,
                    "preempt: busy marker present; SIGTERM to executor process group"
                );
                signaller.sigterm_pgid(pid);
            }
            Some(_) => {
                tracing::warn!(
                    url = %repo.url,
                    "preempt: subprocess sidecar pid is non-positive; cannot SIGTERM"
                );
            }
            None => {
                // iteration_cancel fired but no executor child has spawned
                // yet (pass between acquire and executor launch). The cancel
                // drains the pass at its next await; nothing to SIGTERM.
                tracing::info!(
                    url = %repo.url,
                    "preempt: iteration cancelled; no executor sidecar present (no child to SIGTERM yet)"
                );
            }
        }

        // (3) Wait, bounded, for the busy marker to release. Reuse the
        // single wipe/preempt drain timeout — NO new config field.
        let drain_timeout_secs = cfg.executor.wipe_drain_timeout_secs_clamped();
        let marker = busy_marker::marker_path(&state.paths, workspace);
        if drain_timeout_secs > 0 {
            let start = std::time::Instant::now();
            let max = std::time::Duration::from_secs(drain_timeout_secs);
            while start.elapsed() < max {
                if !marker.exists() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    }

    // (4) Acquire the marker. A SIGTERM'd child's marker (PID now dead)
    // is recovered immediately by try_acquire's dead-pid branch, so the
    // common post-preempt case yields Acquired.
    match busy_marker::try_acquire_with(
        &state.paths,
        workspace,
        &repo.url,
        stale_threshold_secs,
        ops,
    ) {
        Ok(busy_marker::AcquireOutcome::Acquired(guard)) => Ok(PreemptAcquireOutcome {
            guard,
            preempted_change,
        }),
        Ok(busy_marker::AcquireOutcome::SkipAmbiguous(m)) if forceful => {
            // CONFIRMED destructive override: the operator authorized killing
            // an unrecognized holder. Forcibly reclaim past the ambiguity.
            tracing::warn!(
                url = %repo.url,
                pid = m.pid,
                "rollback: forceful escalation — reclaiming ambiguous (PID-reuse-suspected) marker"
            );
            escalate_force_reclaim(state, repo, workspace, ops, &m, preempted_change)
        }
        Ok(busy_marker::AcquireOutcome::SkipAmbiguous(m)) => Err(PreemptAcquireError::Busy(format!(
            "repository `{}` is busy with an unrecognized holder (PID {} alive, PID-reuse suspected); \
             refusing to preempt — investigate before retrying",
            repo.url, m.pid
        ))),
        Ok(busy_marker::AcquireOutcome::SkipFreshInProgress(details)) if forceful => {
            // CONFIRMED destructive override: the marker did not release within
            // the bounded wait. Forcibly reclaim regardless of age.
            tracing::warn!(
                url = %repo.url,
                pid = details.marker.pid,
                age_secs = details.age_secs,
                "rollback: forceful escalation — reclaiming still-held marker (did not release in time)"
            );
            escalate_force_reclaim(state, repo, workspace, ops, &details.marker, preempted_change)
        }
        Ok(busy_marker::AcquireOutcome::SkipFreshInProgress(details)) => {
            Err(PreemptAcquireError::Busy(format!(
                "repository `{}` is still busy after the preempt wait \
                 (marker age {}s, PID {}); the prior pass did not release in time",
                repo.url, details.age_secs, details.marker.pid
            )))
        }
        Err(e) => Err(PreemptAcquireError::Internal(format!(
            "acquiring busy marker for `{}`: {e:#}",
            repo.url
        ))),
    }
}

/// The forced-reclaim escalation shared by both escalating arms of
/// [`preempt_and_acquire_busy_marker_inner`]. Drives `busy_marker::force_reclaim`
/// (SIGKILL the holder's process group + clear the marker file and subprocess
/// sidecar — the SAME kill-and-clear mechanism the age-based stuck branch uses)
/// against the held `marker`, then re-acquires. A cleared marker yields
/// `Acquired`; a residual still-held marker (should not happen after a clear)
/// OR a filesystem error maps to `Internal` — NEVER `Busy`.
fn escalate_force_reclaim(
    state: &ControlState,
    repo: &RepositoryConfig,
    workspace: &Path,
    ops: &(dyn busy_marker::ProcessOps + Send + Sync),
    marker: &busy_marker::BusyMarker,
    preempted_change: Option<String>,
) -> std::result::Result<PreemptAcquireOutcome, PreemptAcquireError> {
    busy_marker::force_reclaim(&state.paths, workspace, marker, ops);
    let stale_threshold_secs = state
        .last_config
        .load_full()
        .executor
        .busy_marker_stale_threshold_secs();
    match busy_marker::try_acquire_with(
        &state.paths,
        workspace,
        &repo.url,
        stale_threshold_secs,
        ops,
    ) {
        Ok(busy_marker::AcquireOutcome::Acquired(guard)) => Ok(PreemptAcquireOutcome {
            guard,
            preempted_change,
        }),
        Ok(other) => Err(PreemptAcquireError::Internal(format!(
            "rollback forceful reclaim cleared the marker for `{}` but re-acquire still saw it held \
             ({}); this should not happen after a force_reclaim",
            repo.url,
            match other {
                busy_marker::AcquireOutcome::Acquired(_) => "acquired",
                busy_marker::AcquireOutcome::SkipFreshInProgress(_) => "skip-fresh-in-progress",
                busy_marker::AcquireOutcome::SkipAmbiguous(_) => "skip-ambiguous",
            }
        ))),
        Err(e) => Err(PreemptAcquireError::Internal(format!(
            "re-acquiring busy marker for `{}` after forceful reclaim: {e:#}",
            repo.url
        ))),
    }
}


// =====================================================================
// Handler submodules
// =====================================================================
//
// `dispatch_request` AND `ControlState` (plus the rest of the transport,
// the shared types, AND the busy-marker preemption above) stay in this
// module root. The action handlers themselves live in the submodules below
// AND are re-exported here, so the `DISPATCH` table, every external call
// site, AND the test module resolve them at `crate::control_socket::*`
// exactly as before the split.
mod enqueue;
mod handlers;

pub(crate) use enqueue::*;
pub(crate) use handlers::*;

#[cfg(test)]
mod tests;
