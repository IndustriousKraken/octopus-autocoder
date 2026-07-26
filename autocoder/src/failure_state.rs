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
//! Each Failed outcome increments the per-change counter; each Archived
//! outcome clears it. Reaching `executor.perma_stuck_after_failures` is
//! what flips a change into the perma-stuck state.
//!
//! The `DaemonPaths` reference is threaded explicitly into every public
//! function (function-parameter pattern per the canonical
//! `Production paths SHALL be threaded` requirement).

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
fn repo_dir(paths: &DaemonPaths, workspace: &Path) -> PathBuf {
    let basename = workspace
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string());
    paths.failure_state_dir().join(basename)
}

fn change_file(paths: &DaemonPaths, workspace: &Path, change: &str) -> PathBuf {
    repo_dir(paths, workspace).join(format!("{change}.json"))
}

/// Load the aggregated failure state for `workspace`.
pub fn load(paths: &DaemonPaths, workspace: &Path) -> Result<FailureState> {
    let dir = repo_dir(paths, workspace);
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
    paths: &DaemonPaths,
    workspace: &Path,
    change: &str,
    reason: &str,
) -> Result<u32> {
    let path = change_file(paths, workspace, change);
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
    save_entry(paths, workspace, change, &entry)?;
    Ok(new_count)
}

fn save_entry(
    paths: &DaemonPaths,
    workspace: &Path,
    change: &str,
    entry: &FailureEntry,
) -> Result<()> {
    let path = change_file(paths, workspace, change);
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

/// Prune failure-state entries whose change directory no longer exists in the
/// workspace's active changes (durable-iteration-record). Runs at pass start
/// AFTER branch sync, so the freshly-pulled base state decides existence.
///
/// A change can complete outside the server's own queue walk (implemented on
/// another machine and pushed, or merged by another host), in which case the
/// server's clear-on-archive never runs for it AND the orphaned counter lingers
/// forever. Pruning is safe because the counter's only consumer is perma-stuck
/// detection, which is meaningless for a change that no longer exists. Entries
/// for changes whose `openspec/changes/<change>/` directory still exists —
/// including marker-excluded ones (perma-stuck, needs-revision) — are retained.
///
/// Best-effort: a missing failure-state dir is an empty prune (not an error).
/// Returns the pruned change names (for logging + tests); one INFO line is
/// logged per removal.
pub fn prune_orphans(paths: &DaemonPaths, workspace: &Path) -> Result<Vec<String>> {
    let dir = repo_dir(paths, workspace);
    let changes_root = workspace.join("openspec").join("changes");
    let read = match std::fs::read_dir(&dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    let mut pruned = Vec::new();
    for entry in read {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(dir = %dir.display(), "failure-state: prune read_dir entry error: {e}");
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let change = match name.strip_suffix(".json") {
            Some(s) => s.to_string(),
            None => continue,
        };
        // A change directory that still exists (pending, waiting, or
        // marker-excluded) keeps its counter. Only truly-gone changes prune.
        if changes_root.join(&change).is_dir() {
            continue;
        }
        let path = entry.path();
        match std::fs::remove_file(&path) {
            Ok(()) => {
                tracing::info!(
                    workspace = %workspace.display(),
                    change = %change,
                    "failure-state: pruned orphaned entry for change no longer in workspace"
                );
                pruned.push(change);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(path = %path.display(), "failure-state: failed to prune orphan: {e:#}");
            }
        }
    }
    Ok(pruned)
}

/// Remove `change`'s entry. Silent on "entry not present" — that's a no-op.
pub fn clear(paths: &DaemonPaths, workspace: &Path, change: &str) -> Result<()> {
    let path = change_file(paths, workspace, change);
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

    #[test]
    fn load_missing_returns_empty() {
        let (_temp, paths) = test_daemon_paths();
        let workspace = paths.cache.join("workspaces").join("ws");
        let state = load(&paths, &workspace).unwrap();
        assert!(state.entries.is_empty());
    }

    #[test]
    fn record_failure_creates_entry() {
        let (_temp, paths) = test_daemon_paths();
        let workspace = paths.cache.join("workspaces").join("ws");
        let n = record_failure(&paths, &workspace, "foo", "first failure").unwrap();
        assert_eq!(n, 1);
        let state = load(&paths, &workspace).unwrap();
        let entry = state.entries.get("foo").expect("entry present");
        assert_eq!(entry.count, 1);
        assert_eq!(entry.last_reason, "first failure");
    }

    #[test]
    fn record_failure_increments_existing() {
        let (_temp, paths) = test_daemon_paths();
        let workspace = paths.cache.join("workspaces").join("ws");
        let n1 = record_failure(&paths, &workspace, "foo", "first").unwrap();
        let n2 = record_failure(&paths, &workspace, "foo", "second").unwrap();
        assert_eq!(n1, 1);
        assert_eq!(n2, 2);
        let state = load(&paths, &workspace).unwrap();
        let entry = state.entries.get("foo").expect("entry present");
        assert_eq!(entry.count, 2);
        assert_eq!(entry.last_reason, "second");
    }

    #[test]
    fn clear_removes_entry() {
        let (_temp, paths) = test_daemon_paths();
        let workspace = paths.cache.join("workspaces").join("ws");
        let _ = record_failure(&paths, &workspace, "foo", "x").unwrap();
        clear(&paths, &workspace, "foo").unwrap();
        let state = load(&paths, &workspace).unwrap();
        assert!(!state.entries.contains_key("foo"));
    }

    #[test]
    fn clear_is_idempotent_when_entry_absent() {
        let (_temp, paths) = test_daemon_paths();
        let workspace = paths.cache.join("workspaces").join("ws");
        clear(&paths, &workspace, "never-existed").expect("clear of absent entry must succeed");
        clear(&paths, &workspace, "still-absent").expect("second clear is also fine");
    }

    #[test]
    fn prune_orphans_removes_absent_retains_present() {
        let (_temp, paths) = test_daemon_paths();
        let workspace = paths.cache.join("workspaces").join("ws");

        // Three failure entries: `present` has a live change dir, `excluded`
        // has a live (marker-excluded) change dir, `gone` has no dir.
        for change in ["present", "excluded", "gone"] {
            record_failure(&paths, &workspace, change, "x").unwrap();
        }
        // `present` is an ordinary active change; `excluded` is marker-excluded
        // (perma-stuck) but its DIRECTORY still exists → both retained.
        for change in ["present", "excluded"] {
            std::fs::create_dir_all(workspace.join("openspec/changes").join(change)).unwrap();
        }
        std::fs::write(
            workspace.join("openspec/changes/excluded/.perma-stuck.json"),
            "{}",
        )
        .unwrap();

        let pruned = prune_orphans(&paths, &workspace).unwrap();
        assert_eq!(pruned, vec!["gone".to_string()], "only the absent change is pruned");

        let state = load(&paths, &workspace).unwrap();
        assert!(state.entries.contains_key("present"), "present change retained");
        assert!(
            state.entries.contains_key("excluded"),
            "marker-excluded change (dir still exists) retained"
        );
        assert!(!state.entries.contains_key("gone"), "vanished change pruned");
    }

    #[test]
    fn prune_orphans_missing_dir_is_empty() {
        let (_temp, paths) = test_daemon_paths();
        let workspace = paths.cache.join("workspaces").join("ws");
        assert!(prune_orphans(&paths, &workspace).unwrap().is_empty());
    }

    #[test]
    fn corrupt_file_treated_as_empty() {
        let (_temp, paths) = test_daemon_paths();
        let workspace = paths.cache.join("workspaces").join("ws");
        let dir = repo_dir(&paths, &workspace);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("foo.json"), "{not json").unwrap();
        let state = load(&paths, &workspace).unwrap();
        assert!(
            state.entries.is_empty(),
            "corrupt file must be treated as fresh state"
        );
    }
}
