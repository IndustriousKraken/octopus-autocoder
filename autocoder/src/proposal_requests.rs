//! Proposal-request state IO for the `chat-request-triage` flow.
//!
//! When an operator posts `@<bot> propose <repo> <text>`, the chatops
//! dispatcher writes a `ProposalRequestState` to disk and pushes a
//! `ProposalRequest` onto the matching `RepoTaskHandle::pending_proposal_requests`
//! queue. The polling loop drains the queue at iteration start and
//! transitions the state file through `Pending → TriagePending →
//! (Acted | Discussed | TriageFailed)`.
//!
//! State files are JSON, atomically written via tempfile-then-rename so a
//! torn write is never visible to a concurrent reader. They live at
//! `<state_dir>/proposal-requests/<repo-sanitized>/<request-id>.json`.

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Cap on the operator's free-form request text. The spec disallows
/// requests over 10,000 characters; the parser enforces the cap on the
/// inbound message before constructing a `ProposalRequestState`.
#[allow(dead_code)] // referenced by callers building a fresh request; parser uses MAX_PROPOSE_REQUEST_TEXT_LEN
pub const REQUEST_TEXT_CAP: usize = 10_000;

/// One proposal-request's tracked state. Written by the chatops
/// dispatcher when `@<bot> propose ...` is accepted; the polling loop
/// reads, transitions, and (on terminal status) leaves on disk so the
/// 7-day prune can sweep it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalRequestState {
    pub request_id: String,
    pub repo_url: String,
    pub channel: String,
    /// The lifecycle thread anchor for chat replies — the bot's ack
    /// message's `ts` in the threading-capable backends, or the operator's
    /// own message ts as a fallback.
    pub thread_ts: String,
    /// The bot's ack message's `ts` specifically. Equal to `thread_ts`
    /// when the bot's ack IS the top-level lifecycle anchor; left blank
    /// when the dispatcher had no chatops backend to post through.
    pub ack_message_ts: String,
    pub operator_user: String,
    pub request_text: String,
    pub submitted_at: DateTime<Utc>,
    pub status: ProposalRequestStatus,
    /// Populated for `TriageFailed` so operators see why the prior
    /// attempt failed (and any other non-success terminal state).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Lifecycle states for a proposal-request entry. Transitions:
///   - Initial: `Pending` (written by the chatops dispatcher).
///   - `Pending` → `TriagePending` when the polling loop picks it up.
///   - `TriagePending` → `Acted` when the executor returns Completed AND
///     the iteration opens the spec PR (a43; or posts the empty-diff
///     reply with no PR).
///   - `TriagePending` → `Discussed` when the executor classified the
///     request as a QUESTION (wrote `.chat-reply.md`).
///   - `TriagePending` → `TriageFailed` when the executor errors out OR
///     produced only code and no spec content (a43).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalRequestStatus {
    Pending,
    TriagePending,
    Acted,
    Discussed,
    TriageFailed,
}

impl ProposalRequestStatus {
    /// Human-readable label for chatops replies and log lines.
    #[allow(dead_code)] // public API shape; consumed by chatops reply formatters when added
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::TriagePending => "triage-pending",
            Self::Acted => "acted",
            Self::Discussed => "discussed",
            Self::TriageFailed => "triage-failed",
        }
    }
}

/// Per-repo subdirectory under `proposal-requests/`. Uses the same
/// URL-sanitization rule as workspace paths so an operator can correlate
/// proposal-request files with the workspace on disk.
fn repo_subdir(repo_url: &str) -> String {
    crate::workspace::sanitize_url(repo_url)
}

/// Canonical state file path: `<state_dir>/proposal-requests/<repo-sanitized>/<request-id>.json`.
pub fn state_path(state_dir_root: &Path, repo_url: &str, request_id: &str) -> PathBuf {
    state_dir_root
        .join("proposal-requests")
        .join(repo_subdir(repo_url))
        .join(format!("{request_id}.json"))
}

/// Top-level directory holding every proposal-request state file
/// (across all repos).
pub fn state_dir(state_dir_root: &Path) -> PathBuf {
    state_dir_root.join("proposal-requests")
}

/// Default state directory: the daemon's resolved `state_dir`. Mirrors
/// `audit-threads`' default-root convention so both flows persist into
/// the same `state_dir` tree.
pub fn default_state_root(paths: &crate::paths::DaemonPaths) -> PathBuf {
    paths.state.clone()
}

/// Atomically write `state` to its canonical file. Parent directories
/// are created if absent.
pub fn write_state(state_dir_root: &Path, state: &ProposalRequestState) -> Result<()> {
    let dir = state_dir(state_dir_root).join(repo_subdir(&state.repo_url));
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating proposal-requests dir {}", dir.display()))?;
    let path = state_path(state_dir_root, &state.repo_url, &state.request_id);
    let tmp = tempfile::NamedTempFile::new_in(&dir)
        .with_context(|| format!("creating tempfile in {}", dir.display()))?;
    serde_json::to_writer_pretty(&tmp, state)
        .with_context(|| format!("serializing proposal-request state for {}", path.display()))?;
    tmp.persist(&path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

/// Read the proposal-request state for `(repo_url, request_id)`. Returns
/// `Ok(None)` when no file exists; surfaces an error on read/parse.
pub fn read_state(
    state_dir_root: &Path,
    repo_url: &str,
    request_id: &str,
) -> Result<Option<ProposalRequestState>> {
    let path = state_path(state_dir_root, repo_url, request_id);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow!("reading {}: {e}", path.display())),
    };
    serde_json::from_str::<ProposalRequestState>(&raw)
        .map(Some)
        .with_context(|| format!("parsing {}", path.display()))
}

/// Remove the state file at `(repo_url, request_id)`. Missing file is
/// a no-op; other errors propagate.
#[allow(dead_code)]
pub fn remove_state(
    state_dir_root: &Path,
    repo_url: &str,
    request_id: &str,
) -> Result<()> {
    let path = state_path(state_dir_root, repo_url, request_id);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow!("removing {}: {e}", path.display())),
    }
}

/// Remove every state file whose `submitted_at` is older than `max_age`,
/// regardless of `status`. Returns the number of entries removed.
/// Unparseable files and stat failures are logged at WARN and skipped;
/// a single bad file cannot stall the prune.
pub fn prune_stale_entries(state_dir_root: &Path, max_age: Duration) -> Result<usize> {
    let dir = state_dir(state_dir_root);
    if !dir.is_dir() {
        return Ok(0);
    }
    let now = Utc::now();
    let mut removed = 0usize;
    // Two-level walk: <root>/proposal-requests/<repo-sanitized>/<request-id>.json
    let outer = std::fs::read_dir(&dir)
        .with_context(|| format!("reading proposal-requests dir {}", dir.display()))?;
    for repo_entry in outer {
        let repo_entry = match repo_entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("proposal-requests prune: outer read_dir entry error: {e}");
                continue;
            }
        };
        let repo_path = repo_entry.path();
        if !repo_path.is_dir() {
            continue;
        }
        let inner = match std::fs::read_dir(&repo_path) {
            Ok(rd) => rd,
            Err(e) => {
                tracing::warn!(
                    path = %repo_path.display(),
                    "proposal-requests prune: inner read_dir error: {e}"
                );
                continue;
            }
        };
        for entry in inner {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(
                        "proposal-requests prune: inner entry error: {e}"
                    );
                    continue;
                }
            };
            let path = entry.path();
            let raw = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        "proposal-requests prune: skipping unreadable file: {e}"
                    );
                    continue;
                }
            };
            let state: ProposalRequestState = match serde_json::from_str(&raw) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        "proposal-requests prune: skipping unparseable file: {e}"
                    );
                    continue;
                }
            };
            if now - state.submitted_at > max_age {
                match std::fs::remove_file(&path) {
                    Ok(()) => removed += 1,
                    Err(e) => tracing::warn!(
                        path = %path.display(),
                        "proposal-requests prune: remove failed: {e}"
                    ),
                }
            }
        }
    }
    Ok(removed)
}

/// Truncate a chat-reply body to `cap` characters, appending the
/// documented daemon-log pointer when truncated. Returns the input
/// unchanged when it fits under the cap.
pub fn truncate_chat_reply_with_pointer(body: &str, request_id: &str, cap: usize) -> String {
    if body.chars().count() <= cap {
        return body.to_string();
    }
    let truncated: String = body.chars().take(cap).collect();
    format!(
        "{truncated}\n\n… [truncated; full reply at journalctl -u autocoder | grep request_id={request_id}]"
    )
}

/// Cap on a chat-reply body posted into the request's lifecycle thread.
/// Mirrors `audit-threads`' `FINDINGS_EXCERPT_CAP` (35,000 chars) so the
/// Slack threaded-reply length budget is consistent across flows.
pub const CHAT_REPLY_BODY_CAP: usize = 35_000;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture_state(request_id: &str, repo_url: &str) -> ProposalRequestState {
        ProposalRequestState {
            request_id: request_id.to_string(),
            repo_url: repo_url.to_string(),
            channel: "C_OPS".to_string(),
            thread_ts: "1748399999.001234".to_string(),
            ack_message_ts: "1748399999.001234".to_string(),
            operator_user: "U0RAB".to_string(),
            request_text: "add a /healthz endpoint".to_string(),
            submitted_at: Utc::now(),
            status: ProposalRequestStatus::Pending,
            reason: None,
        }
    }

    #[test]
    fn read_missing_state_file_returns_ok_none() {
        let tmp = TempDir::new().unwrap();
        let got = read_state(
            tmp.path(),
            "git@github.com:owner/repo.git",
            "00000000-0000-0000-0000-000000000000",
        )
        .unwrap();
        assert!(got.is_none(), "missing file must surface as None");
    }

    #[test]
    fn write_then_read_round_trips_every_field() {
        let tmp = TempDir::new().unwrap();
        let mut state = fixture_state("req-1", "git@github.com:acme/myrepo.git");
        state.status = ProposalRequestStatus::TriageFailed;
        state.reason = Some("executor timeout".into());
        write_state(tmp.path(), &state).unwrap();
        let got = read_state(tmp.path(), &state.repo_url, &state.request_id)
            .unwrap()
            .unwrap();
        assert_eq!(got, state);
        assert_eq!(got.status, ProposalRequestStatus::TriageFailed);
        assert_eq!(got.reason.as_deref(), Some("executor timeout"));
    }

    #[test]
    fn status_transition_preserves_other_fields() {
        let tmp = TempDir::new().unwrap();
        let initial = fixture_state("req-2", "git@github.com:owner/repo.git");
        write_state(tmp.path(), &initial).unwrap();
        let mut updated = initial.clone();
        updated.status = ProposalRequestStatus::Discussed;
        write_state(tmp.path(), &updated).unwrap();
        let got = read_state(tmp.path(), &initial.repo_url, &initial.request_id)
            .unwrap()
            .unwrap();
        assert_eq!(got.status, ProposalRequestStatus::Discussed);
        assert_eq!(got.request_text, initial.request_text);
        assert_eq!(got.channel, initial.channel);
        assert_eq!(got.thread_ts, initial.thread_ts);
        assert_eq!(got.operator_user, initial.operator_user);
    }

    #[test]
    fn prune_removes_old_entries_and_keeps_fresh() {
        let tmp = TempDir::new().unwrap();
        let mut old = fixture_state("req-old", "git@github.com:owner/repo.git");
        old.submitted_at = Utc::now() - Duration::days(8);
        write_state(tmp.path(), &old).unwrap();
        let young = fixture_state("req-young", "git@github.com:owner/repo.git");
        write_state(tmp.path(), &young).unwrap();

        let removed = prune_stale_entries(tmp.path(), Duration::days(7)).unwrap();
        assert_eq!(removed, 1, "exactly the 8-day-old entry must be removed");
        assert!(
            read_state(tmp.path(), &old.repo_url, &old.request_id)
                .unwrap()
                .is_none(),
            "8-day-old entry must be gone"
        );
        assert!(
            read_state(tmp.path(), &young.repo_url, &young.request_id)
                .unwrap()
                .is_some(),
            "5-day-old entry must be preserved"
        );
    }

    #[test]
    fn prune_preserves_entries_regardless_of_status() {
        // The 7-day rule is independent of status — Acted, Discussed, and
        // TriageFailed all survive until they age out.
        let tmp = TempDir::new().unwrap();
        let mut fresh_acted = fixture_state("req-acted", "git@github.com:owner/repo.git");
        fresh_acted.status = ProposalRequestStatus::Acted;
        write_state(tmp.path(), &fresh_acted).unwrap();
        let mut fresh_discussed = fixture_state(
            "req-discussed",
            "git@github.com:owner/repo.git",
        );
        fresh_discussed.status = ProposalRequestStatus::Discussed;
        write_state(tmp.path(), &fresh_discussed).unwrap();
        let removed = prune_stale_entries(tmp.path(), Duration::days(7)).unwrap();
        assert_eq!(removed, 0);
    }

    #[test]
    fn prune_on_empty_or_missing_dir_returns_zero() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(prune_stale_entries(tmp.path(), Duration::days(7)).unwrap(), 0);
        std::fs::create_dir_all(state_dir(tmp.path())).unwrap();
        assert_eq!(prune_stale_entries(tmp.path(), Duration::days(7)).unwrap(), 0);
    }

    #[test]
    fn status_label_round_trips() {
        assert_eq!(ProposalRequestStatus::Pending.label(), "pending");
        assert_eq!(ProposalRequestStatus::TriagePending.label(), "triage-pending");
        assert_eq!(ProposalRequestStatus::Acted.label(), "acted");
        assert_eq!(ProposalRequestStatus::Discussed.label(), "discussed");
        assert_eq!(ProposalRequestStatus::TriageFailed.label(), "triage-failed");
    }

    #[test]
    fn state_path_under_per_repo_subdir() {
        let p = state_path(
            Path::new("/var/lib/autocoder"),
            "git@github.com:owner/repo.git",
            "req-xyz",
        );
        let s = p.to_string_lossy();
        assert!(s.contains("proposal-requests"), "{s}");
        assert!(s.contains("owner_repo"), "per-repo subdir present: {s}");
        assert!(s.ends_with("req-xyz.json"), "{s}");
    }

    #[test]
    fn remove_state_missing_is_noop() {
        let tmp = TempDir::new().unwrap();
        // Not previously written → remove is Ok.
        remove_state(tmp.path(), "git@github.com:owner/repo.git", "nope").unwrap();
    }

    #[test]
    fn truncate_chat_reply_below_cap_unchanged() {
        let s = "short reply";
        let got = truncate_chat_reply_with_pointer(s, "req-1", 100);
        assert_eq!(got, s);
    }

    #[test]
    fn truncate_chat_reply_over_cap_appends_pointer() {
        let s: String = std::iter::repeat_n('x', 50).collect();
        let got = truncate_chat_reply_with_pointer(&s, "req-abc", 10);
        // First 10 chars + pointer suffix.
        assert!(got.starts_with("xxxxxxxxxx"));
        assert!(got.contains("[truncated; full reply at"));
        assert!(got.contains("request_id=req-abc"));
    }

    #[test]
    fn distinct_repos_get_distinct_subdirs() {
        let tmp = TempDir::new().unwrap();
        let s_a = fixture_state("req-shared", "git@github.com:owner/a.git");
        let s_b = fixture_state("req-shared", "git@github.com:owner/b.git");
        write_state(tmp.path(), &s_a).unwrap();
        write_state(tmp.path(), &s_b).unwrap();
        let got_a = read_state(tmp.path(), &s_a.repo_url, "req-shared")
            .unwrap()
            .unwrap();
        let got_b = read_state(tmp.path(), &s_b.repo_url, "req-shared")
            .unwrap()
            .unwrap();
        assert_eq!(got_a.repo_url, "git@github.com:owner/a.git");
        assert_eq!(got_b.repo_url, "git@github.com:owner/b.git");
    }
}
