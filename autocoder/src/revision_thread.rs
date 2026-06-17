//! Spec-revision thread state IO for the interactive spec-revision flow (a03).
//!
//! When autocoder posts a `SpecNeedsRevision` chatops alert for a
//! CONTRADICTION marker (a `.needs-spec-revision.json` whose
//! `unimplementable_tasks` array is empty AND whose `gate_error` is empty — a
//! `[in]` / `[canon]` semantic finding, NOT the executor's unimplementable-
//! tasks flag NOR a gate-error hold), the alert poster captures the resulting
//! `channel` and `thread_ts` and stamps a [`RevisionThreadState`] keyed by the
//! posted message's `thread_ts`. The chatops listener consults this state when
//! an operator replies in the alert thread:
//!
//!   - `@<bot> send it` in the thread runs the spec-revision executor.
//!   - any other `@<bot>` reply runs the read-only revision advisor.
//!
//! Mirrors `crate::audits::threads`'s `AuditThreadState` storage: JSON files
//! under `<state_dir>/revision-threads/<thread_ts>.json`, atomically written
//! via tempfile-then-rename so a torn write is never visible to a concurrent
//! reader, keyed by `thread_ts` so a reply's parent `thread_ts` resolves to at
//! most one record with a direct read.

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One spec-revision alert thread's tracked state. Written by the contradiction
/// alert poster when a threaded `SpecNeedsRevision` alert posts; consulted by
/// the chatops dispatcher when an operator replies in the thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionThreadState {
    pub thread_ts: String,
    pub channel: String,
    pub repo_url: String,
    pub change_slug: String,
    pub status: RevisionThreadStatus,
    pub posted_at: DateTime<Utc>,
}

/// Lifecycle states for a revision-thread entry. Transitions:
///   - Initial: `Open` (written when the contradiction alert posts).
///   - `Open` → `Acted` when `send it` opens a PR for the revision, so a repeat
///     `send it` is handled gracefully (the dispatcher can word its reply
///     without re-running the executor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionThreadStatus {
    Open,
    Acted,
}

impl RevisionThreadStatus {
    /// Human-readable label for chatops replies and log lines.
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Acted => "acted",
        }
    }
}

/// Canonical state file path: `<state_dir>/revision-threads/<thread_ts>.json`.
pub fn state_path(state_dir_root: &Path, thread_ts: &str) -> PathBuf {
    state_dir(state_dir_root).join(format!("{thread_ts}.json"))
}

/// Directory holding every revision-thread state file. Created on first
/// `write_state` call; tests probe this directly.
pub fn state_dir(root: &Path) -> PathBuf {
    root.join("revision-threads")
}

/// Default state directory: the daemon's resolved `state_dir`. The
/// revision-threads files survive reboot alongside audit-thread state, audit
/// cadence, failure counters, and revision state — they belong to the same
/// persistent data category.
pub fn default_state_root(paths: &crate::paths::DaemonPaths) -> PathBuf {
    paths.state.clone()
}

/// Atomically write `state` to its canonical file. Parent directory is
/// created if absent.
pub fn write_state(state_dir_root: &Path, state: &RevisionThreadState) -> Result<()> {
    let dir = state_dir(state_dir_root);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating revision-threads dir {}", dir.display()))?;
    let path = state_path(state_dir_root, &state.thread_ts);
    let tmp = tempfile::NamedTempFile::new_in(&dir)
        .with_context(|| format!("creating tempfile in {}", dir.display()))?;
    serde_json::to_writer_pretty(&tmp, state)
        .with_context(|| format!("serializing revision-thread state for {}", path.display()))?;
    tmp.persist(&path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

/// Read the revision-thread state for `thread_ts`. Returns `Ok(None)` when no
/// file exists; surfaces an error on read or parse failure. This IS the
/// dispatcher's fourth-context lookup (task 1.4): the file is keyed by
/// `thread_ts`, so a reply's parent `thread_ts` resolves to at most one record
/// with a direct read — no scan required.
pub fn read_state(state_dir_root: &Path, thread_ts: &str) -> Result<Option<RevisionThreadState>> {
    let path = state_path(state_dir_root, thread_ts);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow!("reading {}: {e}", path.display())),
    };
    serde_json::from_str::<RevisionThreadState>(&raw)
        .map(Some)
        .with_context(|| format!("parsing {}", path.display()))
}

/// Remove every state file whose `posted_at` is older than `max_age`.
/// Returns the number of entries removed. Unparseable files and stat failures
/// are logged and skipped; the function never propagates such errors so a
/// single bad file cannot stall the prune. Mirrors
/// `crate::audits::threads::prune_stale_entries`.
#[allow(dead_code)] // wired by the daemon's periodic prune in a follow-up
pub fn prune_stale_entries(state_dir_root: &Path, max_age: Duration) -> Result<usize> {
    let dir = state_dir(state_dir_root);
    if !dir.is_dir() {
        return Ok(0);
    }
    let now = Utc::now();
    let mut removed = 0usize;
    for entry in std::fs::read_dir(&dir)
        .with_context(|| format!("reading revision-threads dir {}", dir.display()))?
    {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                // no-url: daemon-global thread-state prune, not scoped to one repo
                tracing::warn!("revision-threads prune: read_dir entry error: {e}");
                continue;
            }
        };
        let path = entry.path();
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                // no-url: daemon-global thread-state prune, not scoped to one repo
                tracing::warn!(
                    path = %path.display(),
                    "revision-threads prune: skipping unreadable file: {e}"
                );
                continue;
            }
        };
        let state: RevisionThreadState = match serde_json::from_str(&raw) {
            Ok(s) => s,
            Err(e) => {
                // no-url: daemon-global thread-state prune, not scoped to one repo
                tracing::warn!(
                    path = %path.display(),
                    "revision-threads prune: skipping unparseable file: {e}"
                );
                continue;
            }
        };
        if now - state.posted_at > max_age {
            match std::fs::remove_file(&path) {
                Ok(()) => removed += 1,
                // no-url: daemon-global thread-state prune, not scoped to one repo
                Err(e) => tracing::warn!(
                    path = %path.display(),
                    "revision-threads prune: remove failed: {e}"
                ),
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture_state(thread_ts: &str) -> RevisionThreadState {
        RevisionThreadState {
            thread_ts: thread_ts.to_string(),
            channel: "C_OPS".to_string(),
            repo_url: "git@github.com:owner/repo.git".to_string(),
            change_slug: "a03-spec-revision-thread".to_string(),
            status: RevisionThreadStatus::Open,
            posted_at: Utc::now(),
        }
    }

    #[test]
    fn read_missing_state_file_returns_ok_none() {
        let tmp = TempDir::new().unwrap();
        let got = read_state(tmp.path(), "1748293445.001234").unwrap();
        assert!(got.is_none(), "missing file must surface as None");
    }

    #[test]
    fn write_then_read_round_trips_every_field() {
        let tmp = TempDir::new().unwrap();
        let mut state = fixture_state("1748.999");
        state.status = RevisionThreadStatus::Acted;
        write_state(tmp.path(), &state).unwrap();
        let got = read_state(tmp.path(), "1748.999").unwrap().unwrap();
        assert_eq!(got, state);
        assert_eq!(got.status, RevisionThreadStatus::Acted);
        assert_eq!(got.change_slug, "a03-spec-revision-thread");
        assert_eq!(got.channel, "C_OPS");
    }

    #[test]
    fn status_transition_preserves_other_fields() {
        let tmp = TempDir::new().unwrap();
        let initial = fixture_state("1748.t1");
        write_state(tmp.path(), &initial).unwrap();

        let mut updated = initial.clone();
        updated.status = RevisionThreadStatus::Acted;
        write_state(tmp.path(), &updated).unwrap();
        let got = read_state(tmp.path(), "1748.t1").unwrap().unwrap();
        assert_eq!(got.status, RevisionThreadStatus::Acted);
        assert_eq!(got.channel, initial.channel);
        assert_eq!(got.repo_url, initial.repo_url);
        assert_eq!(got.change_slug, initial.change_slug);
    }

    #[test]
    fn prune_removes_old_entries_and_keeps_fresh() {
        let tmp = TempDir::new().unwrap();
        let mut old = fixture_state("1700.old");
        old.posted_at = Utc::now() - Duration::days(8);
        write_state(tmp.path(), &old).unwrap();
        let young = fixture_state("1700.young");
        write_state(tmp.path(), &young).unwrap();

        let removed = prune_stale_entries(tmp.path(), Duration::days(7)).unwrap();
        assert_eq!(removed, 1);
        assert!(read_state(tmp.path(), "1700.old").unwrap().is_none());
        assert!(read_state(tmp.path(), "1700.young").unwrap().is_some());
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
        assert_eq!(RevisionThreadStatus::Open.label(), "open");
        assert_eq!(RevisionThreadStatus::Acted.label(), "acted");
    }

    #[test]
    fn state_path_lives_under_revision_threads_subdir() {
        let p = state_path(Path::new("/var/lib/autocoder"), "1700.abc");
        let s = p.to_string_lossy();
        assert!(s.contains("revision-threads"), "{s}");
        assert!(s.ends_with("1700.abc.json"), "{s}");
    }
}
