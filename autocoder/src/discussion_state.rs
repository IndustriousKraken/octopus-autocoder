//! Discussion-thread state IO for the conversational `discuss` flow
//! (replaces the one-shot `propose` chat-triage flow).
//!
//! When an operator posts `@<bot> discuss <repo> <text>` (or the `propose`
//! alias), the chatops dispatcher posts a top-level ack, captures its `ts` as
//! the discussion's `thread_ts`, AND stamps a [`DiscussionState`] keyed by that
//! `thread_ts`. The always-on discuss handler consults + updates this state as
//! the conversation proceeds:
//!
//!   - Every agent turn persists the resumable `session_id` so a follow-up
//!     (`DiscussContinueAction`) OR the eventual `send it`
//!     (`DiscussSendItAction`) can resume the same cached session.
//!   - The chatops listener consults the active set to route an in-thread
//!     `@<bot>` reply: a plain reply → continuation, `@<bot> send it` →
//!     the artifact-creation `DiscussSendItAction`.
//!   - Auto-defer records the `deferred_slug` the handler committed a defer
//!     marker for, so the marker is cleared on PR open AND the 7-day idle
//!     reminder can name the deferred unit.
//!
//! Mirrors [`crate::revision_thread`]'s storage: JSON files under
//! `<state_dir>/discussions/<thread_ts>.json`, atomically written via
//! tempfile-then-rename so a torn write is never visible to a concurrent
//! reader, keyed by `thread_ts` so a reply's parent `thread_ts` resolves to at
//! most one record with a direct read.

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One discuss thread's tracked state. Written by the chatops dispatcher when
/// `@<bot> discuss ...` is accepted; read + updated by the discuss handler as
/// the conversation proceeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscussionState {
    pub thread_ts: String,
    pub channel: String,
    pub repo_url: String,
    pub request_id: String,
    pub operator_user: String,
    pub status: DiscussionStatus,
    /// Resumable agent session id — persisted after each agent turn so a
    /// follow-up OR `send it` can resume the same cached session. Absent until
    /// the first `DiscussAction` turn completes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The change/spec slug the handler committed an auto-defer marker for
    /// (per `Auto-defer protects an existing spec under active discuss`).
    /// Cleared on `send it` PR open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred_slug: Option<String>,
    /// When the one-shot 7-day idle reminder fired. `None` until it fires;
    /// gates the reminder to once per stale discussion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reminded_at: Option<DateTime<Utc>>,
    /// Freshness anchor: updated on every operator/agent turn. Drives the
    /// 7-day idle reminder AND the 14-day prune.
    pub last_activity_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// Lifecycle states for a discussion. Transitions:
///   - Initial: `Active` (written by the chatops dispatcher on `discuss`).
///   - `Active` → `Executing` when `send it` is submitted (artifact job queued).
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

/// Default state directory: the daemon's resolved `state_dir`. Discussion
/// files survive reboot alongside revision-thread, audit-thread, and
/// proposal-request state — they belong to the same persistent category.
pub fn default_state_root(paths: &crate::paths::DaemonPaths) -> PathBuf {
    paths.state.clone()
}

/// Atomically write `state` to its canonical file. Parent directory is
/// created if absent.
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
/// dispatcher's fifth-context lookup AND the continuation lookup: the file is
/// keyed by `thread_ts`, so a reply's parent `thread_ts` resolves to at most
/// one record with a direct read — no scan required.
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
#[allow(dead_code)]
pub fn remove_state(state_dir_root: &Path, thread_ts: &str) -> Result<()> {
    let path = state_path(state_dir_root, thread_ts);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow!("removing {}: {e}", path.display())),
    }
}

/// Read every discussion state file. Unparseable/unreadable files are logged
/// at WARN and skipped so a single bad file cannot stall a scan. Used by the
/// background idle-reminder pass to find stale discussions holding a defer
/// marker.
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
                tracing::warn!("discussions scan: read_dir entry error: {e}");
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
                tracing::warn!(path = %path.display(), "discussions scan: skipping unreadable file: {e}");
                continue;
            }
        };
        match serde_json::from_str::<DiscussionState>(&raw) {
            Ok(s) => out.push(s),
            Err(e) => {
                tracing::warn!(path = %path.display(), "discussions scan: skipping unparseable file: {e}");
            }
        }
    }
    Ok(out)
}

/// Remove every state file whose `last_activity_at` is older than `max_age`,
/// regardless of `status`. Returns the number of entries removed. Unparseable
/// files and stat failures are logged at WARN and skipped; a single bad file
/// cannot stall the prune. Mirrors [`crate::revision_thread::prune_stale_entries`].
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
            request_id: "00000000-0000-0000-0000-000000000000".to_string(),
            operator_user: "U0RAB".to_string(),
            status: DiscussionStatus::Active,
            session_id: None,
            deferred_slug: None,
            reminded_at: None,
            last_activity_at: now,
            created_at: now,
        }
    }

    #[test]
    fn read_missing_state_file_returns_ok_none() {
        let tmp = TempDir::new().unwrap();
        assert!(read_state(tmp.path(), "1748293445.001234").unwrap().is_none());
    }

    #[test]
    fn write_then_read_round_trips_every_field() {
        let tmp = TempDir::new().unwrap();
        let mut state = fixture_state("1748.999");
        state.status = DiscussionStatus::Executing;
        state.session_id = Some("sess-abc".into());
        state.deferred_slug = Some("some-existing-change".into());
        write_state(tmp.path(), &state).unwrap();
        let got = read_state(tmp.path(), "1748.999").unwrap().unwrap();
        assert_eq!(got, state);
        assert_eq!(got.session_id.as_deref(), Some("sess-abc"));
        assert_eq!(got.deferred_slug.as_deref(), Some("some-existing-change"));
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
        assert_eq!(got.channel, initial.channel);
        assert_eq!(got.repo_url, initial.repo_url);
        assert_eq!(got.request_id, initial.request_id);
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
        assert_eq!(removed, 1);
        assert!(read_state(tmp.path(), "1700.old").unwrap().is_none());
        assert!(read_state(tmp.path(), "1700.young").unwrap().is_some());
    }

    #[test]
    fn prune_removes_regardless_of_status() {
        let tmp = TempDir::new().unwrap();
        let mut stale_completed = fixture_state("1700.done");
        stale_completed.status = DiscussionStatus::Completed;
        stale_completed.last_activity_at = Utc::now() - Duration::days(20);
        write_state(tmp.path(), &stale_completed).unwrap();
        assert_eq!(prune_stale_entries(tmp.path(), Duration::days(14)).unwrap(), 1);
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
    fn status_label_round_trips() {
        assert_eq!(DiscussionStatus::Active.label(), "active");
        assert_eq!(DiscussionStatus::Executing.label(), "executing");
        assert_eq!(DiscussionStatus::Completed.label(), "completed");
    }

    #[test]
    fn state_path_lives_under_discussions_subdir() {
        let p = state_path(Path::new("/var/lib/autocoder"), "1700.abc");
        let s = p.to_string_lossy();
        assert!(s.contains("discussions"), "{s}");
        assert!(s.ends_with("1700.abc.json"), "{s}");
    }
}
