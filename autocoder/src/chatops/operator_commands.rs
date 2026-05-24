//! ChatOps operator commands: backend-independent parser, repo-substring
//! matcher, in-memory pending-confirmation tracker for the destructive
//! `wipe-workspace` flow, and reply formatters.
//!
//! Messages that don't start with the bot mention OR don't match one of
//! the known verbs return `None` from `parse_command` — operators typing
//! ordinary chat near the bot must NOT see error spam. Verb matching is
//! case-insensitive and whitespace-tolerant.
//!
//! Recognized verbs:
//!   - `status <repo-substring>`
//!   - `clear-perma-stuck <repo-substring> <change-slug>`
//!   - `clear-revision <repo-substring> <change-slug>`
//!   - `wipe-workspace <repo-substring>`     (first step)
//!   - `confirm`                              (second step; only within 60s
//!                                            of a wipe-workspace in the
//!                                            same channel)
//!   - `rebuild-specs <repo-substring>`       (schedules a canonical-spec
//!                                            rebuild from archive history
//!                                            for the next iteration;
//!                                            never triggers --immediate)

use crate::config::RepositoryConfig;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Default chat-channel TTL for a wipe-workspace pending confirmation.
/// Per spec scenario: "Reply 'confirm' within 60 seconds."
pub const WIPE_CONFIRM_TTL_SECS: u64 = 60;

/// Parsed operator command. The parser does NOT resolve the repo or
/// validate the change slug — the caller is responsible for that step so
/// the parsing layer stays pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperatorCommand {
    Status {
        repo_substring: String,
    },
    ClearPermaStuck {
        repo_substring: String,
        change: String,
    },
    ClearRevision {
        repo_substring: String,
        change: String,
    },
    WipeWorkspace {
        repo_substring: String,
    },
    /// Bare `confirm` reply OR explicit `wipe-workspace-confirm` form.
    /// The caller looks up the channel's pending confirmation; the
    /// `repo_substring` (when present) is informational only — the
    /// authoritative repo URL was captured at the time the original
    /// `wipe-workspace` was issued.
    WipeWorkspaceConfirm {
        repo_substring: Option<String>,
    },
    /// Schedule a canonical-spec rebuild for the next iteration of the
    /// matched repo's polling loop. Chatops NEVER supports `--immediate`:
    /// killing a running executor mid-iteration via chat is too easy to
    /// fire accidentally. Operators wanting `--immediate` SSH to the
    /// daemon host and run the CLI directly.
    RebuildSpecs {
        repo_substring: String,
    },
}

/// Try to parse `message` as an operator command addressed to the bot.
/// Returns `None` for any message that:
///   - does not start with the bot's mention (after leading whitespace), AND
///     is not the bare confirmation form (`confirm`), OR
///   - mentions the bot but uses an unknown verb, OR
///   - matches a known verb but is missing required arguments.
///
/// Bare `confirm` (no mention) returns `Some(WipeWorkspaceConfirm{None})` so
/// the dispatcher can look up a per-channel pending confirmation — that's
/// the friendly second-step UX from the spec.
pub fn parse_command(message: &str, bot_mention: &str) -> Option<OperatorCommand> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mention = bot_mention.trim();

    // Special case: a bare `confirm` (case-insensitive) with no mention.
    // The dispatcher needs to look at the channel's pending-confirmation
    // table; if there's no pending entry, the dispatcher posts the
    // "no pending wipe-workspace confirmation" reply.
    if mention.is_empty() || !trimmed.starts_with(mention) {
        if trimmed.eq_ignore_ascii_case("confirm") {
            return Some(OperatorCommand::WipeWorkspaceConfirm {
                repo_substring: None,
            });
        }
        return None;
    }

    // Strip the mention prefix and any whitespace that follows.
    let after_mention = trimmed[mention.len()..].trim_start();
    if after_mention.is_empty() {
        return None;
    }

    let mut tokens = after_mention.split_whitespace();
    let verb = tokens.next()?;
    let rest: Vec<&str> = tokens.collect();

    match verb.to_ascii_lowercase().as_str() {
        "status" => {
            if rest.len() != 1 {
                return None;
            }
            Some(OperatorCommand::Status {
                repo_substring: rest[0].to_string(),
            })
        }
        "clear-perma-stuck" => {
            if rest.len() != 2 {
                return None;
            }
            Some(OperatorCommand::ClearPermaStuck {
                repo_substring: rest[0].to_string(),
                change: rest[1].to_string(),
            })
        }
        "clear-revision" => {
            if rest.len() != 2 {
                return None;
            }
            Some(OperatorCommand::ClearRevision {
                repo_substring: rest[0].to_string(),
                change: rest[1].to_string(),
            })
        }
        "wipe-workspace" => {
            if rest.len() != 1 {
                return None;
            }
            Some(OperatorCommand::WipeWorkspace {
                repo_substring: rest[0].to_string(),
            })
        }
        "wipe-workspace-confirm" | "confirm" => {
            // Either the explicit form (`@bot wipe-workspace-confirm myrepo`)
            // or the friendly form (`@bot confirm`). The substring is
            // informational; the channel's pending entry is authoritative.
            let substring = rest.first().map(|s| s.to_string());
            Some(OperatorCommand::WipeWorkspaceConfirm {
                repo_substring: substring,
            })
        }
        "rebuild-specs" => {
            if rest.len() != 1 {
                return None;
            }
            Some(OperatorCommand::RebuildSpecs {
                repo_substring: rest[0].to_string(),
            })
        }
        _ => None,
    }
}

// ====================================================================
// Repo-substring matcher
// ====================================================================

/// Outcome of resolving an operator-supplied repo substring against the
/// configured repositories.
#[derive(Debug)]
pub enum RepoMatch<'a> {
    /// Exactly one configured repo matched the substring.
    Unique(&'a RepositoryConfig),
    /// More than one configured repo matched. The caller formats a
    /// "be more specific" reply listing each candidate's URL.
    Multiple(Vec<&'a RepositoryConfig>),
    /// No configured repo matched the substring.
    None,
}

/// Case-insensitive substring match against `repository.url`. Liberal: any
/// configured URL whose lowercase form contains the lowercase of
/// `substring` is a match. Empty substring matches every configured repo
/// (returned as `Multiple` so the operator sees the full list instead of
/// a silent everything-match).
pub fn match_repo<'a>(
    substring: &str,
    configured: &'a [RepositoryConfig],
) -> RepoMatch<'a> {
    let needle = substring.to_ascii_lowercase();
    let mut matches: Vec<&RepositoryConfig> = Vec::new();
    for repo in configured {
        if repo.url.to_ascii_lowercase().contains(&needle) {
            matches.push(repo);
        }
    }
    match matches.len() {
        0 => RepoMatch::None,
        1 => RepoMatch::Unique(matches.into_iter().next().unwrap()),
        _ => RepoMatch::Multiple(matches),
    }
}

// ====================================================================
// Repo-status aggregate response shape
// ====================================================================

/// Daemon's view of a repo, returned by the control-socket `RepoStatus`
/// action. Fields are independent: empty vectors mean "nothing in this
/// section"; the formatter collapses empty sections rather than printing
/// `(none)`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepoStatusResponse {
    pub url: String,
    pub perma_stuck_changes: Vec<MarkerEntry>,
    pub revision_marked_changes: Vec<MarkerEntry>,
    pub throttled_alerts: Vec<ThrottledAlertEntry>,
    pub pending_changes: Vec<String>,
    pub waiting_changes: Vec<String>,
    pub last_iteration: Option<LastIteration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkerEntry {
    pub change: String,
    pub marked_at: DateTime<Utc>,
    /// Free-form detail for the marker (e.g. `consecutive_failures: 2`).
    /// Omitted from the reply when empty.
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThrottledAlertEntry {
    pub label: String,
    pub last_fired_at: DateTime<Utc>,
    pub throttle_window_hours: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastIteration {
    pub finished_at: DateTime<Utc>,
    pub outcome_summary: String,
    pub next_iteration_estimate: Option<DateTime<Utc>>,
    pub poll_interval_sec: u64,
}

// ====================================================================
// Reply formatters
// ====================================================================

/// Format the status response into the multi-line chat reply shape
/// from the proposal. Empty sections are omitted entirely.
pub fn format_status_reply(resp: &RepoStatusResponse) -> String {
    let mut out = String::new();
    out.push_str(&format!("📊 {}\n", resp.url));

    let has_markers =
        !resp.perma_stuck_changes.is_empty() || !resp.revision_marked_changes.is_empty();
    if has_markers {
        out.push_str("\nactive markers (excluded from list_pending):\n");
        for m in &resp.perma_stuck_changes {
            let age = human_age_since(m.marked_at);
            if m.detail.is_empty() {
                out.push_str(&format!(
                    "  • {} (.perma-stuck.json — marked {age} ago)\n",
                    m.change
                ));
            } else {
                out.push_str(&format!(
                    "  • {} (.perma-stuck.json — {}, marked {age} ago)\n",
                    m.change, m.detail
                ));
            }
        }
        for m in &resp.revision_marked_changes {
            let age = human_age_since(m.marked_at);
            out.push_str(&format!(
                "  • {} (.needs-spec-revision.json — marked {age} ago)\n",
                m.change
            ));
        }
    }

    if !resp.throttled_alerts.is_empty() {
        out.push_str("\n24h-throttled alerts currently engaged:\n");
        for a in &resp.throttled_alerts {
            let last_fired = human_age_since(a.last_fired_at);
            let remaining_h = a.throttle_window_hours
                - (Utc::now() - a.last_fired_at).num_hours();
            let remaining = if remaining_h < 0 { 0 } else { remaining_h };
            out.push_str(&format!(
                "  • {} — last fired {last_fired} ago ({remaining}h remaining)\n",
                a.label
            ));
        }
    }

    if let Some(li) = &resp.last_iteration {
        out.push_str("\nlast iteration:\n");
        out.push_str(&format!(
            "  finished: {} ago\n",
            human_age_since(li.finished_at)
        ));
        if !li.outcome_summary.is_empty() {
            out.push_str(&format!("  outcome: {}\n", li.outcome_summary));
        }
        if let Some(next) = li.next_iteration_estimate {
            let delta = next - Utc::now();
            if delta.num_seconds() > 0 {
                out.push_str(&format!(
                    "  next iteration: in ~{} (poll_interval {}s)\n",
                    human_age_duration(delta),
                    li.poll_interval_sec,
                ));
            } else {
                out.push_str(&format!(
                    "  next iteration: due (poll_interval {}s)\n",
                    li.poll_interval_sec
                ));
            }
        } else {
            out.push_str(&format!(
                "  next iteration: poll_interval {}s\n",
                li.poll_interval_sec
            ));
        }
    }

    let queue_has_content = !resp.pending_changes.is_empty()
        || !resp.waiting_changes.is_empty()
        || !resp.perma_stuck_changes.is_empty()
        || !resp.revision_marked_changes.is_empty();
    if queue_has_content {
        out.push_str("\nqueue snapshot:\n");
        if !resp.pending_changes.is_empty() {
            out.push_str(&format!(
                "  pending: {}\n",
                resp.pending_changes.join(", ")
            ));
        }
        if !resp.waiting_changes.is_empty() {
            out.push_str(&format!(
                "  waiting: {}\n",
                resp.waiting_changes.join(", ")
            ));
        }
        let excluded: Vec<String> = resp
            .perma_stuck_changes
            .iter()
            .chain(resp.revision_marked_changes.iter())
            .map(|m| m.change.clone())
            .collect();
        if !excluded.is_empty() {
            out.push_str(&format!(
                "  excluded: {} (see markers above)\n",
                excluded.join(", ")
            ));
        }
    }

    // Strip trailing newline so chatops backends post a single message
    // without an empty terminal line.
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Reply when the operator-supplied substring resolves to more than one
/// configured repository.
pub fn format_multiple_matches(substring: &str, matches: &[&RepositoryConfig]) -> String {
    let urls: Vec<String> = matches.iter().map(|r| r.url.clone()).collect();
    format!(
        "✗ `{substring}` matched multiple repos: {} — be more specific",
        urls.join(", ")
    )
}

/// Reply when the operator-supplied substring matches no configured
/// repository. Lists every configured URL so the operator sees their
/// available options.
pub fn format_no_match(substring: &str, configured: &[RepositoryConfig]) -> String {
    if configured.is_empty() {
        return format!("✗ no repo matched `{substring}`; no repositories configured");
    }
    let urls: Vec<String> = configured.iter().map(|r| r.url.clone()).collect();
    format!(
        "✗ no repo matched `{substring}`; configured: {}",
        urls.join(", ")
    )
}

// ====================================================================
// Pending wipe-workspace confirmations
// ====================================================================

#[derive(Debug, Clone)]
pub struct PendingConfirmation {
    pub repo_url: String,
    pub expires_at: Instant,
}

/// In-memory per-channel pending-confirmation tracker for the destructive
/// `wipe-workspace` flow. The `Instant`-based expiry gives the second-step
/// reply a hard 60-second window (per the spec).
#[derive(Debug, Default)]
pub struct ConfirmationStore {
    pending: Mutex<HashMap<String, PendingConfirmation>>,
}

impl ConfirmationStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a pending wipe-workspace confirmation for `channel_id`,
    /// replacing any prior pending entry on that channel.
    pub fn record(&self, channel_id: &str, repo_url: String, ttl: Duration) {
        let mut g = self.pending.lock().unwrap();
        g.insert(
            channel_id.to_string(),
            PendingConfirmation {
                repo_url,
                expires_at: Instant::now() + ttl,
            },
        );
    }

    /// Look up the pending confirmation for `channel_id`, returning the
    /// captured `repo_url` and consuming the entry. Returns `None` when
    /// no entry exists OR when the entry has expired (an expired entry
    /// is also removed).
    pub fn take_valid(&self, channel_id: &str) -> Option<String> {
        let mut g = self.pending.lock().unwrap();
        let entry = g.remove(channel_id)?;
        if Instant::now() > entry.expires_at {
            return None;
        }
        Some(entry.repo_url)
    }

    /// Test-only: count of in-memory pending entries.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.pending.lock().unwrap().len()
    }
}

// ====================================================================
// Action-submission abstraction
// ====================================================================

/// Submit-action trait that the dispatcher uses to invoke the four
/// control-socket actions. Implementations:
///   - In production: `ControlSocketSubmitter` writes JSON to the
///     daemon's Unix-domain control socket.
///   - In tests: `InProcessSubmitter` calls `control_socket::dispatch_request`
///     directly so the full flow can be driven without a listening
///     socket.
#[async_trait]
pub trait ActionSubmitter: Send + Sync {
    async fn submit(&self, action: serde_json::Value) -> serde_json::Value;
}

// ====================================================================
// OperatorCommandDispatcher — message-in → action → reply-out
// ====================================================================

/// Full-flow dispatcher: parses an incoming chat message, resolves the
/// repo substring against the configured repositories, submits the
/// corresponding action via the supplied `ActionSubmitter`, and returns
/// the formatted chat reply.
///
/// Two-step destructive `wipe-workspace`:
///   - The first step records a pending confirmation keyed by
///     `channel_id` with a 60-second TTL.
///   - The second step (bare `confirm` OR explicit
///     `wipe-workspace-confirm`) consumes the pending entry and submits
///     the actual `wipe_workspace` action.
///   - If no pending entry exists OR it has expired, the dispatcher
///     posts the "no pending wipe-workspace confirmation" error.
pub struct OperatorCommandDispatcher {
    pending: ConfirmationStore,
}

impl Default for OperatorCommandDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl OperatorCommandDispatcher {
    pub fn new() -> Self {
        Self {
            pending: ConfirmationStore::new(),
        }
    }

    /// Parse `text` and execute the resulting command. Returns `Some(reply)`
    /// when the message was recognized AND produced a chat reply (success
    /// or actionable error). Returns `None` for messages that don't address
    /// the bot or don't match a known verb — the caller should fall
    /// through to the existing AskUser-reply detection in that case.
    pub async fn handle_message(
        &self,
        text: &str,
        channel_id: &str,
        bot_mention: &str,
        repositories: &[RepositoryConfig],
        submitter: &dyn ActionSubmitter,
    ) -> Option<String> {
        let cmd = parse_command(text, bot_mention)?;
        Some(self.dispatch(cmd, channel_id, repositories, submitter).await)
    }

    async fn dispatch(
        &self,
        cmd: OperatorCommand,
        channel_id: &str,
        repositories: &[RepositoryConfig],
        submitter: &dyn ActionSubmitter,
    ) -> String {
        match cmd {
            OperatorCommand::Status { repo_substring } => {
                let repo = match match_repo(&repo_substring, repositories) {
                    RepoMatch::Unique(r) => r,
                    RepoMatch::Multiple(ms) => {
                        return format_multiple_matches(&repo_substring, &ms);
                    }
                    RepoMatch::None => return format_no_match(&repo_substring, repositories),
                };
                let resp = submitter
                    .submit(serde_json::json!({
                        "action": "repo_status",
                        "url": repo.url,
                    }))
                    .await;
                if !resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                    let err = resp
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(no error message)");
                    return format!("✗ status failed: {err}");
                }
                let status: RepoStatusResponse =
                    match serde_json::from_value(resp["status"].clone()) {
                        Ok(s) => s,
                        Err(e) => return format!("✗ status decode failed: {e}"),
                    };
                format_status_reply(&status)
            }
            OperatorCommand::ClearPermaStuck {
                repo_substring,
                change,
            } => {
                let repo = match match_repo(&repo_substring, repositories) {
                    RepoMatch::Unique(r) => r,
                    RepoMatch::Multiple(ms) => {
                        return format_multiple_matches(&repo_substring, &ms);
                    }
                    RepoMatch::None => return format_no_match(&repo_substring, repositories),
                };
                let resp = submitter
                    .submit(serde_json::json!({
                        "action": "clear_perma_stuck_marker",
                        "url": repo.url,
                        "change": change,
                    }))
                    .await;
                if resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                    format!(
                        "✓ cleared .perma-stuck.json for {change} on {}",
                        short_repo_label(&repo.url)
                    )
                } else {
                    let err = resp
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(no error message)");
                    format!("✗ {err}")
                }
            }
            OperatorCommand::ClearRevision {
                repo_substring,
                change,
            } => {
                let repo = match match_repo(&repo_substring, repositories) {
                    RepoMatch::Unique(r) => r,
                    RepoMatch::Multiple(ms) => {
                        return format_multiple_matches(&repo_substring, &ms);
                    }
                    RepoMatch::None => return format_no_match(&repo_substring, repositories),
                };
                let resp = submitter
                    .submit(serde_json::json!({
                        "action": "clear_revision_marker",
                        "url": repo.url,
                        "change": change,
                    }))
                    .await;
                if resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                    format!(
                        "✓ cleared .needs-spec-revision.json for {change} on {}",
                        short_repo_label(&repo.url)
                    )
                } else {
                    let err = resp
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(no error message)");
                    format!("✗ {err}")
                }
            }
            OperatorCommand::WipeWorkspace { repo_substring } => {
                let repo = match match_repo(&repo_substring, repositories) {
                    RepoMatch::Unique(r) => r,
                    RepoMatch::Multiple(ms) => {
                        return format_multiple_matches(&repo_substring, &ms);
                    }
                    RepoMatch::None => return format_no_match(&repo_substring, repositories),
                };
                let sanitized = crate::workspace::resolve_path(repo);
                self.pending.record(
                    channel_id,
                    repo.url.clone(),
                    Duration::from_secs(WIPE_CONFIRM_TTL_SECS),
                );
                format!(
                    "⚠️ This will delete {} (forces a re-clone on the next \
                     iteration). Reply 'confirm' within {WIPE_CONFIRM_TTL_SECS} seconds.",
                    sanitized.display()
                )
            }
            OperatorCommand::RebuildSpecs { repo_substring } => {
                let repo = match match_repo(&repo_substring, repositories) {
                    RepoMatch::Unique(r) => r,
                    RepoMatch::Multiple(ms) => {
                        return format_multiple_matches(&repo_substring, &ms);
                    }
                    RepoMatch::None => return format_no_match(&repo_substring, repositories),
                };
                let resp = submitter
                    .submit(serde_json::json!({
                        "action": "rebuild_specs",
                        "url": repo.url,
                        "immediate": false,
                    }))
                    .await;
                if resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                    let poll = resp
                        .get("poll_interval_sec")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(repo.poll_interval_sec);
                    format!(
                        "✓ rebuild scheduled for {} — will run within ~{poll}s (current iteration must finish first)",
                        short_repo_label(&repo.url)
                    )
                } else {
                    let err = resp
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(no error message)");
                    format!("✗ {err}")
                }
            }
            OperatorCommand::WipeWorkspaceConfirm { .. } => {
                let url = match self.pending.take_valid(channel_id) {
                    Some(u) => u,
                    None => {
                        return "✗ no pending wipe-workspace confirmation in this \
                                channel (or it expired — re-issue the original command)"
                            .to_string();
                    }
                };
                let resp = submitter
                    .submit(serde_json::json!({
                        "action": "wipe_workspace",
                        "url": url,
                    }))
                    .await;
                if resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                    let path = resp
                        .get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("workspace");
                    format!(
                        "✓ wiped {path}; next iteration will re-clone"
                    )
                } else {
                    let err = resp
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(no error message)");
                    format!("✗ wipe-workspace failed: {err}")
                }
            }
        }
    }

    #[cfg(test)]
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

/// Production `ActionSubmitter` that writes a single JSON line to the
/// daemon's Unix-domain control socket and reads back the response.
/// Tests use `FakeSubmitter` instead.
pub struct ControlSocketSubmitter {
    socket_path: std::path::PathBuf,
}

impl ControlSocketSubmitter {
    pub fn new(socket_path: std::path::PathBuf) -> Self {
        Self { socket_path }
    }
}

#[async_trait]
impl ActionSubmitter for ControlSocketSubmitter {
    async fn submit(&self, action: serde_json::Value) -> serde_json::Value {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::UnixStream;

        let stream = match UnixStream::connect(&self.socket_path).await {
            Ok(s) => s,
            Err(e) => {
                return serde_json::json!({
                    "ok": false,
                    "error": format!(
                        "could not connect to control socket {}: {e}",
                        self.socket_path.display()
                    ),
                });
            }
        };
        let (read_half, mut write_half) = stream.into_split();
        let mut payload = action.to_string();
        payload.push('\n');
        if let Err(e) = write_half.write_all(payload.as_bytes()).await {
            return serde_json::json!({
                "ok": false,
                "error": format!("writing to control socket: {e}"),
            });
        }
        if let Err(e) = write_half.shutdown().await {
            return serde_json::json!({
                "ok": false,
                "error": format!("shutdown of control socket: {e}"),
            });
        }
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        if let Err(e) = reader.read_line(&mut line).await {
            return serde_json::json!({
                "ok": false,
                "error": format!("reading control socket response: {e}"),
            });
        }
        match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(e) => serde_json::json!({
                "ok": false,
                "error": format!("parsing control socket response: {e}; raw: {line}"),
            }),
        }
    }
}

/// Strip the URL down to a short readable label for chat replies. For a
/// typical `git@host:owner/repo.git`, returns `repo` (the trailing path
/// segment without the `.git` suffix). Falls back to the full URL when
/// the form is unfamiliar.
fn short_repo_label(url: &str) -> String {
    let trimmed = url.trim_end_matches(".git");
    let after_slash = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let after_colon = after_slash.rsplit(':').next().unwrap_or(after_slash);
    after_colon.to_string()
}

// ====================================================================
// Reply-formatting helpers (private)
// ====================================================================

fn human_age_since(when: DateTime<Utc>) -> String {
    let delta = Utc::now() - when;
    human_age_duration(delta)
}

fn human_age_duration(delta: chrono::Duration) -> String {
    let secs = delta.num_seconds().abs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(url: &str) -> RepositoryConfig {
        RepositoryConfig {
            url: url.to_string(),
            local_path: None,
            base_branch: "main".into(),
            agent_branch: "agent-q".into(),
            poll_interval_sec: 60,
            chatops_channel_id: None,
            max_changes_per_pr: None,
            audits: None,
        }
    }

    // ---------- parse_command ----------

    const BOT: &str = "<@UBOT>";

    #[test]
    fn parse_status_happy_path() {
        let cmd = parse_command(&format!("{BOT} status myrepo"), BOT).unwrap();
        assert_eq!(
            cmd,
            OperatorCommand::Status {
                repo_substring: "myrepo".into()
            }
        );
    }

    #[test]
    fn parse_clear_perma_stuck_happy_path() {
        let cmd =
            parse_command(&format!("{BOT} clear-perma-stuck myrepo a06-foo"), BOT)
                .unwrap();
        assert_eq!(
            cmd,
            OperatorCommand::ClearPermaStuck {
                repo_substring: "myrepo".into(),
                change: "a06-foo".into(),
            }
        );
    }

    #[test]
    fn parse_clear_revision_happy_path() {
        let cmd =
            parse_command(&format!("{BOT} clear-revision myrepo a07-bar"), BOT).unwrap();
        assert_eq!(
            cmd,
            OperatorCommand::ClearRevision {
                repo_substring: "myrepo".into(),
                change: "a07-bar".into(),
            }
        );
    }

    #[test]
    fn parse_rebuild_specs_happy_path() {
        let cmd = parse_command(&format!("{BOT} rebuild-specs myrepo"), BOT).unwrap();
        assert_eq!(
            cmd,
            OperatorCommand::RebuildSpecs {
                repo_substring: "myrepo".into()
            }
        );
    }

    #[test]
    fn parse_rebuild_specs_immediate_not_recognized() {
        // The chatops parser does NOT recognize --immediate. Per spec
        // scenario "Chatops verb does not support --immediate": the
        // verb parses as rebuild-specs with the entire remainder as
        // the repo-substring (i.e. None for too-many args, or matches
        // when the operator's literal substring includes "--immediate").
        let cmd = parse_command(&format!("{BOT} rebuild-specs myrepo --immediate"), BOT);
        assert!(
            cmd.is_none(),
            "two-arg form must not parse (--immediate is not a flag)"
        );
    }

    #[test]
    fn parse_rebuild_specs_missing_arg_returns_none() {
        assert!(parse_command(&format!("{BOT} rebuild-specs"), BOT).is_none());
    }

    #[test]
    fn parse_wipe_workspace_happy_path() {
        let cmd = parse_command(&format!("{BOT} wipe-workspace myrepo"), BOT).unwrap();
        assert_eq!(
            cmd,
            OperatorCommand::WipeWorkspace {
                repo_substring: "myrepo".into()
            }
        );
    }

    #[test]
    fn parse_explicit_wipe_workspace_confirm() {
        let cmd =
            parse_command(&format!("{BOT} wipe-workspace-confirm myrepo"), BOT).unwrap();
        assert_eq!(
            cmd,
            OperatorCommand::WipeWorkspaceConfirm {
                repo_substring: Some("myrepo".into())
            }
        );
    }

    #[test]
    fn parse_bare_confirm_no_mention() {
        let cmd = parse_command("confirm", BOT).unwrap();
        assert_eq!(
            cmd,
            OperatorCommand::WipeWorkspaceConfirm {
                repo_substring: None
            }
        );
    }

    #[test]
    fn parse_bare_confirm_case_insensitive() {
        for form in ["CONFIRM", "Confirm", "ConFIRM"] {
            let cmd = parse_command(form, BOT).unwrap();
            assert_eq!(
                cmd,
                OperatorCommand::WipeWorkspaceConfirm {
                    repo_substring: None
                }
            );
        }
    }

    #[test]
    fn parse_confirm_mentioned() {
        let cmd = parse_command(&format!("{BOT} confirm"), BOT).unwrap();
        assert_eq!(
            cmd,
            OperatorCommand::WipeWorkspaceConfirm {
                repo_substring: None
            }
        );
    }

    #[test]
    fn parse_missing_arg_returns_none() {
        assert!(parse_command(&format!("{BOT} status"), BOT).is_none());
        assert!(parse_command(&format!("{BOT} clear-perma-stuck myrepo"), BOT).is_none());
        assert!(parse_command(&format!("{BOT} clear-revision"), BOT).is_none());
        assert!(parse_command(&format!("{BOT} wipe-workspace"), BOT).is_none());
    }

    #[test]
    fn parse_too_many_args_returns_none() {
        // The spec lists one substring for status; trailing junk is an
        // ambiguous typo, not a known verb.
        assert!(parse_command(&format!("{BOT} status myrepo extra"), BOT).is_none());
    }

    #[test]
    fn parse_message_without_mention_returns_none() {
        // Don't drown random chat in error replies.
        assert!(parse_command("status myrepo", BOT).is_none());
        assert!(parse_command("hello world", BOT).is_none());
        assert!(parse_command("@somebody-else status myrepo", BOT).is_none());
    }

    #[test]
    fn parse_unknown_verb_returns_none() {
        assert!(parse_command(&format!("{BOT} hello"), BOT).is_none());
        assert!(parse_command(&format!("{BOT} please archive everything"), BOT).is_none());
        // Explicitly out-of-scope per spec.
        assert!(parse_command(&format!("{BOT} pause myrepo"), BOT).is_none());
        assert!(parse_command(&format!("{BOT} resume myrepo"), BOT).is_none());
        assert!(parse_command(&format!("{BOT} clear-alert-throttle x"), BOT).is_none());
    }

    #[test]
    fn parse_verb_is_case_insensitive() {
        for verb_form in ["status", "Status", "STATUS", "StAtUs"] {
            let cmd = parse_command(&format!("{BOT} {verb_form} myrepo"), BOT)
                .unwrap_or_else(|| panic!("`{verb_form}` should parse"));
            assert_eq!(
                cmd,
                OperatorCommand::Status {
                    repo_substring: "myrepo".into()
                }
            );
        }
    }

    #[test]
    fn parse_whitespace_tolerance() {
        // Leading/trailing whitespace + multi-space separators are all ok.
        let cmd =
            parse_command(&format!("   {BOT}   status    myrepo   "), BOT).unwrap();
        assert_eq!(
            cmd,
            OperatorCommand::Status {
                repo_substring: "myrepo".into()
            }
        );
    }

    #[test]
    fn parse_empty_message_returns_none() {
        assert!(parse_command("", BOT).is_none());
        assert!(parse_command("   ", BOT).is_none());
    }

    #[test]
    fn parse_mention_only_returns_none() {
        assert!(parse_command(BOT, BOT).is_none());
        assert!(parse_command(&format!("{BOT}   "), BOT).is_none());
    }

    // ---------- match_repo ----------

    #[test]
    fn match_repo_unique() {
        let repos = vec![
            repo("git@github.com:acme/myrepo.git"),
            repo("git@github.com:acme/widgets.git"),
        ];
        match match_repo("myrepo", &repos) {
            RepoMatch::Unique(r) => assert!(r.url.contains("myrepo")),
            other => panic!("expected Unique, got {other:?}"),
        }
    }

    #[test]
    fn match_repo_multiple() {
        let repos = vec![
            repo("git@github.com:org-a/repo.git"),
            repo("git@github.com:org-b/repo.git"),
        ];
        match match_repo("repo", &repos) {
            RepoMatch::Multiple(ms) => assert_eq!(ms.len(), 2),
            other => panic!("expected Multiple, got {other:?}"),
        }
    }

    #[test]
    fn match_repo_none() {
        let repos = vec![repo("git@github.com:owner/foo.git")];
        match match_repo("nonexistent", &repos) {
            RepoMatch::None => {}
            other => panic!("expected None, got {other:?}"),
        }
    }

    #[test]
    fn match_repo_case_insensitive() {
        let repos = vec![repo("git@github.com:acme/myrepo.git")];
        match match_repo("MYREPO", &repos) {
            RepoMatch::Unique(r) => assert!(r.url.contains("myrepo")),
            other => panic!("expected Unique, got {other:?}"),
        }
    }

    #[test]
    fn match_repo_empty_substring_returns_all_as_multiple() {
        let repos = vec![
            repo("git@github.com:owner/a.git"),
            repo("git@github.com:owner/b.git"),
        ];
        match match_repo("", &repos) {
            RepoMatch::Multiple(ms) => assert_eq!(ms.len(), 2),
            other => panic!("expected Multiple, got {other:?}"),
        }
    }

    #[test]
    fn match_repo_empty_substring_with_one_repo_is_unique() {
        let repos = vec![repo("git@github.com:owner/only.git")];
        match match_repo("", &repos) {
            RepoMatch::Unique(_) => {}
            other => panic!("expected Unique (single repo), got {other:?}"),
        }
    }

    // ---------- ConfirmationStore ----------

    #[test]
    fn confirmation_store_round_trip() {
        let store = ConfirmationStore::new();
        store.record("C1", "git@github.com:owner/repo.git".into(), Duration::from_secs(60));
        assert_eq!(store.len(), 1);
        let url = store.take_valid("C1").expect("present");
        assert_eq!(url, "git@github.com:owner/repo.git");
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn confirmation_store_expires_after_ttl() {
        let store = ConfirmationStore::new();
        store.record("C1", "url".into(), Duration::from_millis(10));
        std::thread::sleep(Duration::from_millis(50));
        // Expired → take_valid returns None AND removes the entry.
        assert!(store.take_valid("C1").is_none());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn confirmation_store_cross_channel_isolation() {
        let store = ConfirmationStore::new();
        store.record("A", "url-a".into(), Duration::from_secs(60));
        // Channel B has no pending → take_valid returns None.
        assert!(store.take_valid("B").is_none());
        // A's pending is untouched.
        assert_eq!(store.take_valid("A").as_deref(), Some("url-a"));
    }

    #[test]
    fn confirmation_store_replaces_prior_pending() {
        let store = ConfirmationStore::new();
        store.record("C", "url-1".into(), Duration::from_secs(60));
        store.record("C", "url-2".into(), Duration::from_secs(60));
        // Second record replaces first.
        assert_eq!(store.take_valid("C").as_deref(), Some("url-2"));
    }

    // ---------- Reply formatters ----------

    #[test]
    fn format_status_collapses_empty_sections() {
        let resp = RepoStatusResponse {
            url: "git@github.com:owner/repo.git".into(),
            ..RepoStatusResponse::default()
        };
        let out = format_status_reply(&resp);
        // Header is always present.
        assert!(out.starts_with("📊 git@github.com:owner/repo.git"));
        // No "active markers" / "throttled alerts" / "queue snapshot" /
        // "last iteration" lines appear since every section is empty.
        for label in [
            "active markers",
            "throttled alerts",
            "queue snapshot",
            "last iteration",
            "(none)",
        ] {
            assert!(
                !out.contains(label),
                "empty status reply must not contain `{label}`; got: {out}"
            );
        }
    }

    #[test]
    fn format_status_lists_markers_when_present() {
        let resp = RepoStatusResponse {
            url: "git@github.com:owner/repo.git".into(),
            perma_stuck_changes: vec![MarkerEntry {
                change: "a06-foo".into(),
                marked_at: Utc::now() - chrono::Duration::hours(4),
                detail: "consecutive_failures: 2".into(),
            }],
            revision_marked_changes: vec![MarkerEntry {
                change: "a07-bar".into(),
                marked_at: Utc::now() - chrono::Duration::minutes(22),
                detail: String::new(),
            }],
            ..RepoStatusResponse::default()
        };
        let out = format_status_reply(&resp);
        assert!(out.contains("active markers"));
        assert!(out.contains("a06-foo"));
        assert!(out.contains(".perma-stuck.json"));
        assert!(out.contains("consecutive_failures: 2"));
        assert!(out.contains("a07-bar"));
        assert!(out.contains(".needs-spec-revision.json"));
        // The queue snapshot's "excluded" line lists both markers.
        assert!(out.contains("excluded: a06-foo, a07-bar"));
    }

    #[test]
    fn format_no_match_lists_configured_repos() {
        let repos = vec![
            repo("git@github.com:owner/myrepo.git"),
            repo("git@github.com:owner/widgets.git"),
        ];
        let out = format_no_match("gibberish", &repos);
        assert!(out.starts_with("✗ "));
        assert!(out.contains("gibberish"));
        assert!(out.contains("myrepo"));
        assert!(out.contains("widgets"));
    }

    #[test]
    fn format_multiple_matches_lists_candidates() {
        let r1 = repo("git@github.com:org-a/repo.git");
        let r2 = repo("git@github.com:org-b/repo.git");
        let out = format_multiple_matches("repo", &[&r1, &r2]);
        assert!(out.starts_with("✗ "));
        assert!(out.contains("org-a/repo"));
        assert!(out.contains("org-b/repo"));
        assert!(out.contains("be more specific"));
    }

    // ---------- OperatorCommandDispatcher (full flow) ----------

    /// Test-only `ActionSubmitter` that records every submitted action
    /// JSON and replies with a configurable response. Suitable for
    /// driving the dispatcher's message-in → action → reply-out flow
    /// without a real control socket or daemon.
    struct FakeSubmitter {
        responses: Mutex<HashMap<String, serde_json::Value>>,
        log: Mutex<Vec<serde_json::Value>>,
    }

    impl FakeSubmitter {
        fn new() -> Self {
            Self {
                responses: Mutex::new(HashMap::new()),
                log: Mutex::new(Vec::new()),
            }
        }

        fn set_response(&self, action: &str, value: serde_json::Value) {
            self.responses
                .lock()
                .unwrap()
                .insert(action.to_string(), value);
        }

        fn calls(&self) -> Vec<serde_json::Value> {
            self.log.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ActionSubmitter for FakeSubmitter {
        async fn submit(&self, action: serde_json::Value) -> serde_json::Value {
            self.log.lock().unwrap().push(action.clone());
            let verb = action
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            self.responses
                .lock()
                .unwrap()
                .get(&verb)
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"ok": false, "error": "no fake response"}))
        }
    }

    fn fixture_repos() -> Vec<RepositoryConfig> {
        vec![
            repo("git@github.com:acme/myrepo.git"),
            repo("git@github.com:acme/widgets.git"),
        ]
    }

    #[tokio::test]
    async fn dispatch_status_returns_formatted_reply() {
        let dispatcher = OperatorCommandDispatcher::new();
        let submitter = FakeSubmitter::new();
        submitter.set_response(
            "repo_status",
            serde_json::json!({
                "ok": true,
                "status": {
                    "url": "git@github.com:acme/myrepo.git",
                    "perma_stuck_changes": [],
                    "revision_marked_changes": [],
                    "throttled_alerts": [],
                    "pending_changes": ["a08-deploy"],
                    "waiting_changes": [],
                    "last_iteration": null,
                },
            }),
        );
        let reply = dispatcher
            .handle_message(
                &format!("{BOT} status myrepo"),
                "C1",
                BOT,
                &fixture_repos(),
                &submitter,
            )
            .await
            .expect("dispatcher must produce a reply");
        assert!(reply.contains("git@github.com:acme/myrepo.git"));
        assert!(reply.contains("pending: a08-deploy"));
    }

    #[tokio::test]
    async fn dispatch_clear_perma_stuck_on_unique_repo_submits_action() {
        let dispatcher = OperatorCommandDispatcher::new();
        let submitter = FakeSubmitter::new();
        submitter.set_response("clear_perma_stuck_marker", serde_json::json!({"ok": true}));
        let reply = dispatcher
            .handle_message(
                &format!("{BOT} clear-perma-stuck myrepo a06-foo"),
                "C1",
                BOT,
                &fixture_repos(),
                &submitter,
            )
            .await
            .unwrap();
        assert!(reply.starts_with("✓"));
        assert!(reply.contains("a06-foo"));
        assert!(reply.contains("myrepo"));
        let calls = submitter.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["action"], "clear_perma_stuck_marker");
        assert_eq!(
            calls[0]["url"], "git@github.com:acme/myrepo.git"
        );
        assert_eq!(calls[0]["change"], "a06-foo");
    }

    #[tokio::test]
    async fn dispatch_clear_perma_stuck_propagates_action_error() {
        let dispatcher = OperatorCommandDispatcher::new();
        let submitter = FakeSubmitter::new();
        submitter.set_response(
            "clear_perma_stuck_marker",
            serde_json::json!({
                "ok": false,
                "error": "no perma-stuck marker for change `a99-nope`",
            }),
        );
        let reply = dispatcher
            .handle_message(
                &format!("{BOT} clear-perma-stuck myrepo a99-nope"),
                "C1",
                BOT,
                &fixture_repos(),
                &submitter,
            )
            .await
            .unwrap();
        assert!(reply.starts_with("✗"));
        assert!(reply.contains("no perma-stuck marker"));
        assert!(reply.contains("a99-nope"));
    }

    #[tokio::test]
    async fn dispatch_no_match_replies_with_configured_list() {
        let dispatcher = OperatorCommandDispatcher::new();
        let submitter = FakeSubmitter::new();
        let reply = dispatcher
            .handle_message(
                &format!("{BOT} status gibberish"),
                "C1",
                BOT,
                &fixture_repos(),
                &submitter,
            )
            .await
            .unwrap();
        assert!(reply.starts_with("✗"));
        assert!(reply.contains("gibberish"));
        assert!(reply.contains("myrepo"));
        assert!(reply.contains("widgets"));
        // No action was submitted.
        assert!(submitter.calls().is_empty());
    }

    #[tokio::test]
    async fn dispatch_unknown_verb_returns_none() {
        let dispatcher = OperatorCommandDispatcher::new();
        let submitter = FakeSubmitter::new();
        let reply = dispatcher
            .handle_message(
                &format!("{BOT} please archive everything"),
                "C1",
                BOT,
                &fixture_repos(),
                &submitter,
            )
            .await;
        assert!(reply.is_none(), "unknown verbs must produce None for silent ignore");
    }

    // ---------- wipe-workspace confirmation flow ----------

    #[tokio::test]
    async fn wipe_workspace_two_step_confirm_happy_path() {
        let dispatcher = OperatorCommandDispatcher::new();
        let submitter = FakeSubmitter::new();
        submitter.set_response(
            "wipe_workspace",
            serde_json::json!({
                "ok": true,
                "path": "/tmp/workspaces/github_com_acme_myrepo",
                "already_absent": false,
            }),
        );

        let warn = dispatcher
            .handle_message(
                &format!("{BOT} wipe-workspace myrepo"),
                "C1",
                BOT,
                &fixture_repos(),
                &submitter,
            )
            .await
            .unwrap();
        assert!(warn.starts_with("⚠️"), "first step is a warning: {warn}");
        assert!(warn.contains("confirm"));
        assert!(warn.contains("60 seconds"));
        assert!(submitter.calls().is_empty(), "no action submitted yet");
        assert_eq!(dispatcher.pending_len(), 1);

        let success = dispatcher
            .handle_message("confirm", "C1", BOT, &fixture_repos(), &submitter)
            .await
            .unwrap();
        assert!(success.starts_with("✓"), "confirm should succeed: {success}");
        assert!(success.contains("wiped"));
        assert_eq!(submitter.calls().len(), 1);
        assert_eq!(submitter.calls()[0]["action"], "wipe_workspace");
        assert_eq!(dispatcher.pending_len(), 0);
    }

    #[tokio::test]
    async fn wipe_workspace_confirm_without_pending_returns_error() {
        let dispatcher = OperatorCommandDispatcher::new();
        let submitter = FakeSubmitter::new();
        let reply = dispatcher
            .handle_message("confirm", "C1", BOT, &fixture_repos(), &submitter)
            .await
            .unwrap();
        assert!(reply.starts_with("✗"));
        assert!(reply.contains("no pending"));
        assert!(submitter.calls().is_empty());
    }

    #[tokio::test]
    async fn wipe_workspace_expired_confirmation_returns_error_no_wipe() {
        let dispatcher = OperatorCommandDispatcher::new();
        let submitter = FakeSubmitter::new();
        // Manually record a stale entry to avoid sleeping the test 60s.
        dispatcher
            .pending
            .record("C1", "git@github.com:owner/repo.git".into(), Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(20));
        let reply = dispatcher
            .handle_message("confirm", "C1", BOT, &fixture_repos(), &submitter)
            .await
            .unwrap();
        assert!(reply.starts_with("✗"));
        assert!(reply.contains("no pending"));
        assert!(submitter.calls().is_empty());
    }

    #[tokio::test]
    async fn wipe_workspace_cross_channel_confirm_no_match() {
        let dispatcher = OperatorCommandDispatcher::new();
        let submitter = FakeSubmitter::new();
        submitter.set_response(
            "wipe_workspace",
            serde_json::json!({"ok": true, "path": "/tmp/workspaces/x", "already_absent": false}),
        );
        // Wipe in channel A, confirm in channel B.
        dispatcher
            .handle_message(
                &format!("{BOT} wipe-workspace myrepo"),
                "A",
                BOT,
                &fixture_repos(),
                &submitter,
            )
            .await
            .unwrap();
        let reply_b = dispatcher
            .handle_message("confirm", "B", BOT, &fixture_repos(), &submitter)
            .await
            .unwrap();
        assert!(reply_b.starts_with("✗"));
        assert!(reply_b.contains("no pending"));
        assert!(submitter.calls().is_empty(), "no action submitted from cross-channel confirm");
        // A's pending entry is still live.
        assert_eq!(dispatcher.pending_len(), 1);
    }

    #[tokio::test]
    async fn control_socket_submitter_returns_error_on_missing_socket() {
        // No daemon → no socket → ActionSubmitter reports the failure
        // shape the dispatcher can format into a `✗` reply.
        let dir = tempfile::TempDir::new().unwrap();
        let submitter =
            ControlSocketSubmitter::new(dir.path().join("does-not-exist.sock"));
        let resp = submitter
            .submit(serde_json::json!({"action":"repo_status","url":"x"}))
            .await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false));
        let err = resp["error"].as_str().unwrap();
        assert!(
            err.contains("could not connect"),
            "must explain the failure: {err}"
        );
    }

    #[tokio::test]
    async fn wipe_workspace_reissue_replaces_prior_pending() {
        let dispatcher = OperatorCommandDispatcher::new();
        let submitter = FakeSubmitter::new();
        submitter.set_response(
            "wipe_workspace",
            serde_json::json!({"ok": true, "path": "/tmp/workspaces/sound", "already_absent": false}),
        );
        dispatcher
            .handle_message(
                &format!("{BOT} wipe-workspace myrepo"),
                "C1",
                BOT,
                &fixture_repos(),
                &submitter,
            )
            .await
            .unwrap();
        dispatcher
            .handle_message(
                &format!("{BOT} wipe-workspace widgets"),
                "C1",
                BOT,
                &fixture_repos(),
                &submitter,
            )
            .await
            .unwrap();
        // The second wipe replaced the first pending — `confirm` wipes
        // widgets, NOT myrepo.
        let success = dispatcher
            .handle_message("confirm", "C1", BOT, &fixture_repos(), &submitter)
            .await
            .unwrap();
        assert!(success.starts_with("✓"));
        let calls = submitter.calls();
        let wipe_call = calls
            .iter()
            .find(|c| c["action"] == "wipe_workspace")
            .expect("wipe_workspace must be submitted");
        assert_eq!(
            wipe_call["url"], "git@github.com:acme/widgets.git",
            "the second wipe's URL must win"
        );
    }
}
