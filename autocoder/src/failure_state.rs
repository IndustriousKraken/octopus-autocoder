//! Per-change persistence for the consecutive-failure counter that
//! drives perma-stuck change detection.
//!
//! In production, state lives at
//! `<state_dir>/failure-state/<repo-sanitized>/<change>.json`, where
//! the repo-sanitized fragment is the workspace's basename (already
//! URL-sanitized per `workspace::derive_path`). One file per change;
//! the in-memory [`FailureState`] aggregates them per-repo for the
//! polling-loop callers.
//!
//! In tests where the daemon-paths global has not been installed, the
//! module falls back to a single per-workspace `.failure-state.json`
//! file at the workspace root. The fallback preserves test isolation
//! (one TempDir-rooted workspace per test) without forcing every test
//! to install paths explicitly.
//!
//! Each Failed outcome increments the per-change counter; each Archived
//! outcome clears it. Reaching `executor.perma_stuck_after_failures` is
//! what flips a change into the perma-stuck state.

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const LEGACY_PER_WORKSPACE_FILE: &str = ".failure-state.json";

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

/// `true` when the production state-dir layout is active (i.e. the
/// daemon has installed its `DaemonPaths` global). When `false`, the
/// module uses the legacy single-file-per-workspace layout — keeps
/// tests that build workspaces in TempDirs working without each one
/// needing to install paths.
fn state_dir_layout_active() -> bool {
    crate::paths::get_global().is_some()
}

/// Per-repo directory under `<state_dir>/failure-state/`.
fn repo_dir(workspace: &Path) -> PathBuf {
    let basename = workspace
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string());
    crate::paths::current()
        .state
        .join("failure-state")
        .join(basename)
}

fn change_file(workspace: &Path, change: &str) -> PathBuf {
    repo_dir(workspace).join(format!("{change}.json"))
}

fn legacy_path(workspace: &Path) -> PathBuf {
    workspace.join(LEGACY_PER_WORKSPACE_FILE)
}

/// Load the aggregated failure state for `workspace`.
pub fn load(workspace: &Path) -> Result<FailureState> {
    if state_dir_layout_active() {
        load_from_state_dir(workspace)
    } else {
        load_from_legacy(workspace)
    }
}

fn load_from_state_dir(workspace: &Path) -> Result<FailureState> {
    let dir = repo_dir(workspace);
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

fn load_from_legacy(workspace: &Path) -> Result<FailureState> {
    let path = legacy_path(workspace);
    match std::fs::read_to_string(&path) {
        Ok(raw) => match serde_json::from_str::<FailureState>(&raw) {
            Ok(state) => Ok(state),
            Err(e) => {
                tracing::warn!(
                    "failure-state file at {} is corrupt; starting empty: {e:#}",
                    path.display()
                );
                Ok(FailureState::default())
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FailureState::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Increment the failure counter for `change`, recording the reason and
/// timestamp. Creates the entry if absent. Returns the new count.
pub fn record_failure(workspace: &Path, change: &str, reason: &str) -> Result<u32> {
    if state_dir_layout_active() {
        record_failure_state_dir(workspace, change, reason)
    } else {
        record_failure_legacy(workspace, change, reason)
    }
}

fn record_failure_state_dir(workspace: &Path, change: &str, reason: &str) -> Result<u32> {
    let path = change_file(workspace, change);
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
    save_entry_state_dir(workspace, change, &entry)?;
    Ok(new_count)
}

fn save_entry_state_dir(workspace: &Path, change: &str, entry: &FailureEntry) -> Result<()> {
    let path = change_file(workspace, change);
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

fn record_failure_legacy(workspace: &Path, change: &str, reason: &str) -> Result<u32> {
    let mut state = load_from_legacy(workspace)?;
    let entry = state
        .entries
        .entry(change.to_string())
        .or_insert(FailureEntry {
            count: 0,
            last_reason: String::new(),
            last_failed_at: Utc::now(),
        });
    entry.count = entry.count.saturating_add(1);
    entry.last_reason = reason.to_string();
    entry.last_failed_at = Utc::now();
    let new_count = entry.count;
    save_legacy(&state, workspace)?;
    Ok(new_count)
}

fn save_legacy(state: &FailureState, workspace: &Path) -> Result<()> {
    let path = legacy_path(workspace);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("destination path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating parent dir {}", parent.display()))?;
    let tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating tempfile in {}", parent.display()))?;
    serde_json::to_writer_pretty(&tmp, state)
        .with_context(|| format!("serializing failure state for {}", path.display()))?;
    tmp.persist(&path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

/// Remove `change`'s entry. Silent on "entry not present" — that's a no-op.
pub fn clear(workspace: &Path, change: &str) -> Result<()> {
    if state_dir_layout_active() {
        let path = change_file(workspace, change);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
        }
    } else {
        let mut state = load_from_legacy(workspace)?;
        if state.entries.remove(change).is_none() {
            return Ok(());
        }
        save_legacy(&state, workspace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_missing_returns_empty() {
        let dir = TempDir::new().unwrap();
        let state = load(dir.path()).unwrap();
        assert!(state.entries.is_empty());
    }

    #[test]
    fn record_failure_creates_entry() {
        let dir = TempDir::new().unwrap();
        let n = record_failure(dir.path(), "foo", "first failure").unwrap();
        assert_eq!(n, 1);
        let state = load(dir.path()).unwrap();
        let entry = state.entries.get("foo").expect("entry present");
        assert_eq!(entry.count, 1);
        assert_eq!(entry.last_reason, "first failure");
    }

    #[test]
    fn record_failure_increments_existing() {
        let dir = TempDir::new().unwrap();
        let n1 = record_failure(dir.path(), "foo", "first").unwrap();
        let n2 = record_failure(dir.path(), "foo", "second").unwrap();
        assert_eq!(n1, 1);
        assert_eq!(n2, 2);
        let state = load(dir.path()).unwrap();
        let entry = state.entries.get("foo").expect("entry present");
        assert_eq!(entry.count, 2);
        assert_eq!(entry.last_reason, "second");
    }

    #[test]
    fn clear_removes_entry() {
        let dir = TempDir::new().unwrap();
        let _ = record_failure(dir.path(), "foo", "x").unwrap();
        clear(dir.path(), "foo").unwrap();
        let state = load(dir.path()).unwrap();
        assert!(!state.entries.contains_key("foo"));
    }

    #[test]
    fn clear_is_idempotent_when_entry_absent() {
        let dir = TempDir::new().unwrap();
        clear(dir.path(), "never-existed").expect("clear of absent entry must succeed");
        clear(dir.path(), "still-absent").expect("second clear is also fine");
    }

    #[test]
    fn corrupt_file_treated_as_empty() {
        let dir = TempDir::new().unwrap();
        // Legacy-mode test: write a corrupt per-workspace file and
        // confirm load handles it. The state-dir layout's per-file
        // corruption is exercised by the spec scenarios.
        std::fs::write(dir.path().join(".failure-state.json"), "{not json").unwrap();
        let state = load(dir.path()).unwrap();
        assert!(
            state.entries.is_empty(),
            "corrupt file must be treated as fresh state"
        );
    }
}
