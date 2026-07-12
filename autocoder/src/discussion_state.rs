//! Discussion-thread state IO for the conversational `discuss` flow
//! (discuss-verb-conversational-propose).
//!
//! When an operator posts `@<bot> discuss <repo> <text>` (or the `propose`
//! alias), the chatops dispatcher posts a top-level ack, captures its `ts` as
//! the discussion `thread_ts`, and stamps a [`DiscussionState`] keyed by that
//! `thread_ts`. The dedicated discuss handler reads/updates it as the
//! conversation runs; the chatops listener consults it to route in-thread
//! replies:
//!
//!   - `@<bot> send it` in the thread → the artifact-creation job.
//!   - any other `@<bot>` reply → a continuation turn.
//!
//! Mirrors `crate::revision_thread`'s storage: JSON files under
//! `<state_dir>/discussions/<thread_ts>.json`, atomically written via
//! tempfile-then-rename so a torn write is never visible to a concurrent
//! reader, keyed by `thread_ts` so a reply's parent `thread_ts` resolves to at
//! most one record with a direct read. The file is the SINGLE source of truth
//! for the discuss-thread set in the `send it` five-context dispatch.

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One discussion thread's tracked state. Written by the chatops dispatcher on
/// a new `discuss`/`propose`; updated by the discuss handler after each agent
/// turn (session id, activity timestamp) and by `send it` (status, deferral).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscussionState {
    pub thread_ts: String,
    pub channel: String,
    pub repo_url: String,
    pub request_id: String,
    pub operator_user: String,
    /// The operator's initial free-form message (the discussion seed).
    pub initial_text: String,
    pub status: DiscussionStatus,
    /// Backend session id used to resume the agentic session token-cache-
    /// friendly across continuation turns AND the eventual `send it` write
    /// turn. `None` until the first agent turn persists it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// When the agent signalled it is discussing a modification to an existing
    /// spec/change, the slug the handler auto-deferred (moved to
    /// `deferred-*/`). Cleared on `send it` PR-open. `None` when nothing is
    /// deferred for this discussion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred_slug: Option<String>,
    /// Set once the 7-day idle reminder has fired, so it fires exactly once per
    /// stale discussion. Reset to `None` on any new thread activity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reminded_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// Last time the operator or agent touched this discussion. Drives the
    /// 7-day idle reminder AND the 14-day prune.
    pub last_activity_at: DateTime<Utc>,
}

/// Lifecycle states for a discussion. Transitions:
///   - Initial: `Active` (written by the chatops dispatcher).
///   - `Active` → `Executing` when `send it` is accepted (artifact job queued).
///   - `Executing` → `Completed` when the artifact PR is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscussionStatus {
    Active,
    Executing,
    Completed,
}

impl DiscussionStatus {
    /// Human-readable label for chatops replies and log lines.
    #[allow(dead_code)] // symmetry with sibling state modules; consumed by reply formatters
    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Executing => "executing",
            Self::Completed => "completed",
        }
    }
}

/// Canonical state file path: `<state_dir>/discussions/<thread_ts>.json`.
pub fn state_path(state_dir_root: &Path, thread_ts: &str) -> PathBuf {
    state_dir(state_dir_root).join(format!("{thread_ts}.json"))
}

/// Directory holding every discussion state file. Created on first
/// `write_state` call; tests probe this directly.
pub fn state_dir(root: &Path) -> PathBuf {
    root.join("discussions")
}

/// Default state directory: the daemon's resolved `state_dir`. The discussion
/// files survive reboot alongside audit-thread, revision-thread, and
/// proposal-request state — the same persistent data category.
pub fn default_state_root(paths: &crate::paths::DaemonPaths) -> PathBuf {
    paths.state.clone()
}

/// Atomically write `state` to its canonical file. Parent directory is created
/// if absent.
pub fn write_state(state_dir_root: &Path, state: &DiscussionState) -> Result<()> {
    let dir = state_dir(state_dir_root);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating discussions dir {}", dir.display()))?;
    let path = state_path(state_dir_root, &state.thread_ts);
    let tmp = tempfile::NamedTempFile::new_in(&dir)
        .with_context(|| format!("creating tempfile in {}", dir.display()))?;
    serde_json::to_writer_pretty(&tmp, state)
        .with_context(|| format!("serializing discussion state for {}", path.display()))?;
    tmp.persist(&path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

/// Read the discussion state for `thread_ts`. Returns `Ok(None)` when no file
/// exists; surfaces an error on read or parse failure. This IS the
/// dispatcher's fifth-context lookup: the file is keyed by `thread_ts`, so a
/// reply's parent `thread_ts` resolves to at most one record with a direct
/// read — no scan required.
pub fn read_state(state_dir_root: &Path, thread_ts: &str) -> Result<Option<DiscussionState>> {
    let path = state_path(state_dir_root, thread_ts);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow!("reading {}: {e}", path.display())),
    };
    serde_json::from_str::<DiscussionState>(&raw)
        .map(Some)
        .with_context(|| format!("parsing {}", path.display()))
}

/// Remove the state file for `thread_ts`. Missing file is a no-op; other
/// errors propagate.
#[allow(dead_code)] // used by handler cleanup paths + tests
pub fn remove_state(state_dir_root: &Path, thread_ts: &str) -> Result<()> {
    let path = state_path(state_dir_root, thread_ts);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow!("removing {}: {e}", path.display())),
    }
}

/// Return every discussion state currently on disk (across all repos). Skips
/// unreadable/unparseable files with a WARN so one bad file cannot stall the
/// caller. Used by the idle-reminder scan.
pub fn list_states(state_dir_root: &Path) -> Result<Vec<DiscussionState>> {
    let dir = state_dir(state_dir_root);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .with_context(|| format!("reading discussions dir {}", dir.display()))?
    {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("discussions list: read_dir entry error: {e}");
                continue;
            }
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(path = %path.display(), "discussions list: skipping unreadable file: {e}");
                continue;
            }
        };
        match serde_json::from_str::<DiscussionState>(&raw) {
            Ok(s) => out.push(s),
            Err(e) => {
                tracing::warn!(path = %path.display(), "discussions list: skipping unparseable file: {e}");
            }
        }
    }
    Ok(out)
}

/// Remove every state file whose `last_activity_at` is older than `max_age`,
/// regardless of `status`. Returns the number removed. Unparseable files and
/// stat failures are logged and skipped; the function never propagates such
/// errors so a single bad file cannot stall the prune. Mirrors
/// `crate::revision_thread::prune_stale_entries` but keys off
/// `last_activity_at` rather than a fixed post time.
pub fn prune_stale_entries(state_dir_root: &Path, max_age: Duration) -> Result<usize> {
    let dir = state_dir(state_dir_root);
    if !dir.is_dir() {
        return Ok(0);
    }
    let now = Utc::now();
    let mut removed = 0usize;
    for entry in std::fs::read_dir(&dir)
        .with_context(|| format!("reading discussions dir {}", dir.display()))?
    {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("discussions prune: read_dir entry error: {e}");
                continue;
            }
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(path = %path.display(), "discussions prune: skipping unreadable file: {e}");
                continue;
            }
        };
        let state: DiscussionState = match serde_json::from_str(&raw) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(path = %path.display(), "discussions prune: skipping unparseable file: {e}");
                continue;
            }
        };
        if now - state.last_activity_at > max_age {
            match std::fs::remove_file(&path) {
                Ok(()) => removed += 1,
                Err(e) => tracing::warn!(path = %path.display(), "discussions prune: remove failed: {e}"),
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture_state(thread_ts: &str) -> DiscussionState {
        let now = Utc::now();
        DiscussionState {
            thread_ts: thread_ts.to_string(),
            channel: "C_OPS".to_string(),
            repo_url: "git@github.com:owner/repo.git".to_string(),
            request_id: "req-1".to_string(),
            operator_user: "U0RAB".to_string(),
            initial_text: "how does the revision executor stop retrying?".to_string(),
            status: DiscussionStatus::Active,
            session_id: None,
            deferred_slug: None,
            reminded_at: None,
            created_at: now,
            last_activity_at: now,
        }
    }

    #[test]
    fn read_missing_state_file_returns_ok_none() {
        let tmp = TempDir::new().unwrap();
        assert!(read_state(tmp.path(), "1748.001").unwrap().is_none());
    }

    #[test]
    fn write_then_read_round_trips_every_field() {
        let tmp = TempDir::new().unwrap();
        let mut state = fixture_state("1748.999");
        state.status = DiscussionStatus::Executing;
        state.session_id = Some("sess-abc".into());
        state.deferred_slug = Some("a03-spec-revision-thread".into());
        write_state(tmp.path(), &state).unwrap();
        let got = read_state(tmp.path(), "1748.999").unwrap().unwrap();
        assert_eq!(got, state);
        assert_eq!(got.session_id.as_deref(), Some("sess-abc"));
        assert_eq!(got.deferred_slug.as_deref(), Some("a03-spec-revision-thread"));
    }

    #[test]
    fn status_transition_preserves_other_fields() {
        let tmp = TempDir::new().unwrap();
        let initial = fixture_state("1748.t1");
        write_state(tmp.path(), &initial).unwrap();
        let mut updated = initial.clone();
        updated.status = DiscussionStatus::Completed;
        write_state(tmp.path(), &updated).unwrap();
        let got = read_state(tmp.path(), "1748.t1").unwrap().unwrap();
        assert_eq!(got.status, DiscussionStatus::Completed);
        assert_eq!(got.initial_text, initial.initial_text);
        assert_eq!(got.channel, initial.channel);
        assert_eq!(got.repo_url, initial.repo_url);
    }

    #[test]
    fn prune_removes_old_entries_and_keeps_fresh() {
        let tmp = TempDir::new().unwrap();
        let mut old = fixture_state("1700.old");
        old.last_activity_at = Utc::now() - Duration::days(15);
        write_state(tmp.path(), &old).unwrap();
        let young = fixture_state("1700.young");
        write_state(tmp.path(), &young).unwrap();

        let removed = prune_stale_entries(tmp.path(), Duration::days(14)).unwrap();
        assert_eq!(removed, 1, "exactly the 15-day-old entry must be removed");
        assert!(read_state(tmp.path(), "1700.old").unwrap().is_none());
        assert!(read_state(tmp.path(), "1700.young").unwrap().is_some());
    }

    #[test]
    fn prune_ignores_status_and_uses_last_activity() {
        // A Completed discussion still alive within 14 days must survive.
        let tmp = TempDir::new().unwrap();
        let mut done = fixture_state("1700.done");
        done.status = DiscussionStatus::Completed;
        write_state(tmp.path(), &done).unwrap();
        assert_eq!(prune_stale_entries(tmp.path(), Duration::days(14)).unwrap(), 0);
    }

    #[test]
    fn prune_on_empty_or_missing_dir_returns_zero() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(prune_stale_entries(tmp.path(), Duration::days(14)).unwrap(), 0);
        std::fs::create_dir_all(state_dir(tmp.path())).unwrap();
        assert_eq!(prune_stale_entries(tmp.path(), Duration::days(14)).unwrap(), 0);
    }

    #[test]
    fn list_states_returns_all_written() {
        let tmp = TempDir::new().unwrap();
        write_state(tmp.path(), &fixture_state("1700.a")).unwrap();
        write_state(tmp.path(), &fixture_state("1700.b")).unwrap();
        let mut got: Vec<String> = list_states(tmp.path())
            .unwrap()
            .into_iter()
            .map(|s| s.thread_ts)
            .collect();
        got.sort();
        assert_eq!(got, vec!["1700.a".to_string(), "1700.b".to_string()]);
    }

    #[test]
    fn remove_state_missing_is_noop() {
        let tmp = TempDir::new().unwrap();
        remove_state(tmp.path(), "nope").unwrap();
    }

    #[test]
    fn state_path_lives_under_discussions_subdir() {
        let p = state_path(Path::new("/var/lib/autocoder"), "1700.abc");
        let s = p.to_string_lossy();
        assert!(s.contains("discussions"), "{s}");
        assert!(s.ends_with("1700.abc.json"), "{s}");
    }

    #[test]
    fn status_label_round_trips() {
        assert_eq!(DiscussionStatus::Active.label(), "active");
        assert_eq!(DiscussionStatus::Executing.label(), "executing");
        assert_eq!(DiscussionStatus::Completed.label(), "completed");
    }
}
