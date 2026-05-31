//! Per-change persistence for the consecutive-failure counter that
//! drives perma-stuck change detection.
//!
//! State lives at
//! `<state_dir>/failure-state/<repo-sanitized>/<change>.json`, where
//! the repo-sanitized fragment is the workspace's basename (already
//! URL-sanitized per `workspace::derive_path`). One file per change;
//! the in-memory [`FailureState`] aggregates them per-repo for the
//! polling-loop callers.
//!
//! The `DaemonPaths` value is threaded explicitly through every public
//! API. Tests construct one via [`crate::testing::test_daemon_paths`]
//! AND pass it explicitly — see the canonical orchestrator-cli
//! "Production paths SHALL be threaded" requirement.
//!
//! Each Failed outcome increments the per-change counter; each Archived
//! outcome clears it. Reaching `executor.perma_stuck_after_failures` is
//! what flips a change into the perma-stuck state.

use crate::paths::DaemonPaths;
use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureEntry {
    pub count: u32,
    pub last_reason: String,
    pub last_failed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FailureState {
    #[serde(flatten)]
    pub entries: HashMap<String, FailureEntry>,
}

/// Per-repo directory under `<state_dir>/failure-state/`.
fn repo_dir(workspace: &Path, paths: &DaemonPaths) -> PathBuf {
    let basename = workspace
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string());
    paths
        .state
        .join("failure-state")
        .join(basename)
}

fn change_file(workspace: &Path, change: &str, paths: &DaemonPaths) -> PathBuf {
    repo_dir(workspace, paths).join(format!("{change}.json"))
}

/// Load the aggregated failure state for `workspace`.
pub fn load(workspace: &Path, paths: &DaemonPaths) -> Result<FailureState> {
    let dir = repo_dir(workspace, paths);
    let mut state = FailureState::default();
    let read = match std::fs::read_dir(&dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(state),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    for entry in read {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(
                    dir = %dir.display(),
                    "failure-state: read_dir entry error: {e}"
                );
                continue;
            }
        };
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let change = match name.strip_suffix(".json") {
            Some(s) => s.to_string(),
            None => continue,
        };
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    "failure-state: read failed; skipping entry: {e}"
                );
                continue;
            }
        };
        match serde_json::from_str::<FailureEntry>(&raw) {
            Ok(e) => {
                state.entries.insert(change, e);
            }
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    "failure-state file is corrupt; treating change as no-history: {e:#}"
                );
            }
        }
    }
    Ok(state)
}

/// Increment the failure counter for `change`, recording the reason and
/// timestamp. Creates the entry if absent. Returns the new count.
pub fn record_failure(
    workspace: &Path,
    change: &str,
    reason: &str,
    paths: &DaemonPaths,
) -> Result<u32> {
    let path = change_file(workspace, change, paths);
    let mut entry = match std::fs::read_to_string(&path) {
        Ok(raw) => match serde_json::from_str::<FailureEntry>(&raw) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    "failure-state file is corrupt; starting fresh counter: {e:#}"
                );
                FailureEntry {
                    count: 0,
                    last_reason: String::new(),
                    last_failed_at: Utc::now(),
                }
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => FailureEntry {
            count: 0,
            last_reason: String::new(),
            last_failed_at: Utc::now(),
        },
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    entry.count = entry.count.saturating_add(1);
    entry.last_reason = reason.to_string();
    entry.last_failed_at = Utc::now();
    let new_count = entry.count;
    save_entry(workspace, change, &entry, paths)?;
    Ok(new_count)
}

fn save_entry(
    workspace: &Path,
    change: &str,
    entry: &FailureEntry,
    paths: &DaemonPaths,
) -> Result<()> {
    let path = change_file(workspace, change, paths);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("destination path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating parent dir {}", parent.display()))?;
    let tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating tempfile in {}", parent.display()))?;
    serde_json::to_writer_pretty(&tmp, entry)
        .with_context(|| format!("serializing failure state for {}", path.display()))?;
    tmp.persist(&path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

/// Remove `change`'s entry. Silent on "entry not present" — that's a no-op.
pub fn clear(workspace: &Path, change: &str, paths: &DaemonPaths) -> Result<()> {
    let path = change_file(workspace, change, paths);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::test_daemon_paths;
    use tempfile::TempDir;

    #[test]
    fn load_missing_returns_empty() {
        let (_t, paths) = test_daemon_paths();
        let dir = TempDir::new().unwrap();
        let state = load(dir.path(), &paths).unwrap();
        assert!(state.entries.is_empty());
    }

    #[test]
    fn record_failure_creates_entry() {
        let (_t, paths) = test_daemon_paths();
        let dir = TempDir::new().unwrap();
        let n = record_failure(dir.path(), "foo", "first failure", &paths).unwrap();
        assert_eq!(n, 1);
        let state = load(dir.path(), &paths).unwrap();
        let entry = state.entries.get("foo").expect("entry present");
        assert_eq!(entry.count, 1);
        assert_eq!(entry.last_reason, "first failure");
    }

    #[test]
    fn record_failure_increments_existing() {
        let (_t, paths) = test_daemon_paths();
        let dir = TempDir::new().unwrap();
        let n1 = record_failure(dir.path(), "foo", "first", &paths).unwrap();
        let n2 = record_failure(dir.path(), "foo", "second", &paths).unwrap();
        assert_eq!(n1, 1);
        assert_eq!(n2, 2);
        let state = load(dir.path(), &paths).unwrap();
        let entry = state.entries.get("foo").expect("entry present");
        assert_eq!(entry.count, 2);
        assert_eq!(entry.last_reason, "second");
    }

    #[test]
    fn clear_removes_entry() {
        let (_t, paths) = test_daemon_paths();
        let dir = TempDir::new().unwrap();
        let _ = record_failure(dir.path(), "foo", "x", &paths).unwrap();
        clear(dir.path(), "foo", &paths).unwrap();
        let state = load(dir.path(), &paths).unwrap();
        assert!(!state.entries.contains_key("foo"));
    }

    #[test]
    fn clear_is_idempotent_when_entry_absent() {
        let (_t, paths) = test_daemon_paths();
        let dir = TempDir::new().unwrap();
        clear(dir.path(), "never-existed", &paths).expect("clear of absent entry must succeed");
        clear(dir.path(), "still-absent", &paths).expect("second clear is also fine");
    }

    #[test]
    fn corrupt_per_file_is_treated_as_no_history() {
        let (_t, paths) = test_daemon_paths();
        let workspace = TempDir::new().unwrap();
        // Write a corrupt per-change file under the per-repo failure-state dir.
        let change_path = change_file(workspace.path(), "foo", &paths);
        std::fs::create_dir_all(change_path.parent().unwrap()).unwrap();
        std::fs::write(&change_path, "{not json").unwrap();
        let state = load(workspace.path(), &paths).unwrap();
        assert!(
            !state.entries.contains_key("foo"),
            "corrupt per-change file must be treated as no-history (entry skipped)"
        );
    }
}
