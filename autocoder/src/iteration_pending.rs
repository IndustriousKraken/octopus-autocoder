//! Per-change iteration-pending marker (a27a1). When the polling
//! loop handles an `ExecutorOutcome::IterationRequested`, it writes
//! `<state>/iteration-pending/<workspace_basename>/<change>.json`
//! after committing + force-pushing the WIP to the agent branch. The
//! marker survives the gap between subprocess-exit AND next-poll-cycle
//! (including a daemon restart) AND carries the cumulative completed/
//! remaining task lists, the agent's stated reason, AND the upcoming
//! iteration number into the next prompt's continuation block. The
//! marker's presence ALSO front-inserts the change in `list_pending`.
//!
//! **State-dir location, not workspace.** Earlier a27a1 implementations
//! put the marker at `<workspace>/openspec/changes/<change>/.iteration-pending.json`
//! — that location caused `git clean -fd` on the next iteration's
//! dirty-workspace recovery to wipe the marker, breaking the cap +
//! continuation-context mechanics entirely. The marker is pure daemon
//! bookkeeping (never operator-edited), so per a16's "daemon bookkeeping
//! never appears in the managed repo's working tree" rule it lives
//! under `<state>/iteration-pending/` instead, resolved via
//! `DaemonPaths::iteration_pending_path`. No git-interaction surface.
//!
//! Lifecycle:
//! - `IterationRequested` arm: write/replace marker with the new state
//!   (after WIP commit + push succeed).
//! - `Completed` arm: delete the marker after commit + push completes.
//! - `SpecNeedsRevision` arm: delete the marker.
//! - `Failed` arm: leave the marker untouched (retry preserves context).
//! - `AskUser` arm: leave the marker untouched.
//!
//! A corrupt marker is treated as `iteration_number: 0` for ordering
//! AND as "no marker" for prompt-builder continuation; the corrupt file
//! is NOT deleted (operator can inspect).
//!
//! One-time migration: on workspace init, any legacy
//! `<workspace>/openspec/changes/<change>/.iteration-pending.json` marker
//! is moved to its state-dir location AND the workspace copy is deleted.
//! See [`migrate_legacy_workspace_markers`].

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::paths::DaemonPaths;

/// The on-disk filename. Kept public so the legacy-workspace-marker
/// migration AND any future tooling can reference the canonical name
/// from a single source of truth.
pub const MARKER_FILE: &str = ".iteration-pending.json";

/// On-disk shape of the iteration-pending marker. All fields are
/// required; the polling-loop's `IterationRequested` arm populates them
/// from the `ExecutorOutcome::IterationRequested` payload it consumed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IterationPendingMarker {
    pub completed_tasks: Vec<String>,
    pub remaining_tasks: Vec<String>,
    pub reason: String,
    pub iteration_number: u32,
}

/// True when the iteration-pending marker for `(workspace_basename, change)`
/// exists on disk. Pure filesystem check — no JSON parsing, so a corrupt
/// marker still returns true.
pub fn marker_exists(
    paths: &DaemonPaths,
    workspace_basename: &str,
    change: &str,
) -> bool {
    paths.iteration_pending_path(workspace_basename, change).is_file()
}

/// Read AND parse the marker. Returns `Ok(None)` when the file is
/// absent; `Err(...)` for any IO or parse failure. Callers that want
/// corrupt-as-absent semantics convert `Err(...)` to `None` themselves
/// (see classifier AND prompt-builder, both of which log a warning AND
/// fall through).
pub fn read_marker(
    paths: &DaemonPaths,
    workspace_basename: &str,
    change: &str,
) -> Result<Option<IterationPendingMarker>> {
    let path = paths.iteration_pending_path(workspace_basename, change);
    match std::fs::read_to_string(&path) {
        Ok(s) => {
            let marker: IterationPendingMarker = serde_json::from_str(&s)
                .with_context(|| format!("parsing iteration-pending marker {}", path.display()))?;
            Ok(Some(marker))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => {
            Err(e).with_context(|| format!("reading iteration-pending marker {}", path.display()))
        }
    }
}

/// Atomic write of the marker file (tempfile + rename). The per-workspace
/// state-dir directory `<state>/iteration-pending/<workspace_basename>/`
/// is created if absent — operators don't have to mkdir it manually.
pub fn write_marker(
    paths: &DaemonPaths,
    workspace_basename: &str,
    change: &str,
    marker: &IterationPendingMarker,
) -> Result<()> {
    let dir = paths.iteration_pending_basename_dir(workspace_basename);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating iteration-pending dir {}", dir.display()))?;
    let path = paths.iteration_pending_path(workspace_basename, change);
    let tmp = tempfile::NamedTempFile::new_in(&dir)
        .with_context(|| format!("creating tempfile in {}", dir.display()))?;
    serde_json::to_writer_pretty(&tmp, marker).with_context(|| {
        format!(
            "serializing iteration-pending marker for {}",
            path.display()
        )
    })?;
    tmp.persist(&path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

/// List every change in this workspace that has an iteration-pending
/// marker on disk. Returns change names in sorted order; an empty Vec
/// means no iteration-pending state is active. Used by the polling-
/// loop's audit-only-PR suppression rule (a38): when any marker is
/// present, the audit-only-PR path is suppressed for this iteration
/// so iteration_request WIP commits don't ship in a misleading
/// "0 change(s)" PR.
///
/// Marker-corruption (truncated JSON, missing fields) is treated as
/// "present" for suppression purposes — the marker file's existence is
/// what gates the rule, NOT its parseability.
pub fn list_pending_changes(
    paths: &DaemonPaths,
    workspace_basename: &str,
) -> Vec<String> {
    let dir = paths.iteration_pending_basename_dir(workspace_basename);
    let entries = match std::fs::read_dir(&dir) {
        Ok(it) => it,
        Err(_) => return Vec::new(),
    };
    let mut found: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            // Strip the .json suffix to get the change name. Skip
            // anything that doesn't end in .json (defensive — the
            // directory should only contain marker files).
            name.strip_suffix(".json").map(|s| s.to_string())
        })
        .collect();
    found.sort();
    found
}

/// Idempotent removal of the marker file. A missing file is success.
pub fn remove_marker(
    paths: &DaemonPaths,
    workspace_basename: &str,
    change: &str,
) -> Result<()> {
    let path = paths.iteration_pending_path(workspace_basename, change);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

/// One-time migration: move any legacy iteration-pending markers from
/// the workspace's change directories to their state-dir locations. The
/// legacy markers were written by earlier a27a1 implementations at
/// `<workspace>/openspec/changes/<change>/.iteration-pending.json`;
/// `git clean -fd` on the next dirty-workspace recovery wiped them.
/// This migration preserves any markers that survived an upgrade so
/// operators don't lose iteration state.
///
/// Called from `workspace::ensure_initialized` for every managed
/// workspace at daemon startup AND at the top of every polling
/// iteration. Idempotent: a workspace with no legacy markers is a no-op.
///
/// Behavior per marker found:
/// 1. Read the legacy file's contents.
/// 2. If parseable AND the state-dir target does NOT already exist:
///    write the marker to its state-dir location, then delete the
///    workspace copy. Log INFO naming the migration.
/// 3. If the state-dir target already exists: delete the workspace copy
///    WITHOUT overwriting state-dir (the state-dir copy is authoritative
///    post-migration; the workspace copy is stale). Log INFO.
/// 4. If the legacy file is unparseable: leave it in place; log WARN
///    naming the corruption. Operator can inspect AND delete manually.
pub fn migrate_legacy_workspace_markers(
    paths: &DaemonPaths,
    workspace: &Path,
    workspace_basename: &str,
) -> Result<()> {
    let changes_dir = workspace.join("openspec/changes");
    let entries = match std::fs::read_dir(&changes_dir) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(e).with_context(|| {
                format!("reading change directories at {}", changes_dir.display())
            });
        }
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let name = match entry.file_name().to_str() {
            Some(s) => s.to_string(),
            None => continue,
        };
        if name == "archive" || name.starts_with('.') {
            continue;
        }
        let legacy_path = entry.path().join(MARKER_FILE);
        if !legacy_path.is_file() {
            continue;
        }
        let contents = match std::fs::read_to_string(&legacy_path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    workspace = %workspace.display(),
                    change = %name,
                    "skipping legacy iteration-pending marker migration; could not read {}: {e:#}",
                    legacy_path.display()
                );
                continue;
            }
        };
        let marker: IterationPendingMarker = match serde_json::from_str(&contents) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    workspace = %workspace.display(),
                    change = %name,
                    "leaving corrupt legacy iteration-pending marker in workspace; operator can inspect AND delete {}: {e:#}",
                    legacy_path.display()
                );
                continue;
            }
        };
        let target = paths.iteration_pending_path(workspace_basename, &name);
        if target.is_file() {
            tracing::info!(
                workspace = %workspace.display(),
                change = %name,
                "legacy iteration-pending marker at {} superseded by state-dir copy at {}; deleting legacy copy",
                legacy_path.display(),
                target.display()
            );
        } else if let Err(e) = write_marker(paths, workspace_basename, &name, &marker) {
            tracing::warn!(
                workspace = %workspace.display(),
                change = %name,
                "could not migrate legacy iteration-pending marker {} → state-dir: {e:#}; leaving legacy file in place",
                legacy_path.display()
            );
            continue;
        } else {
            tracing::info!(
                workspace = %workspace.display(),
                change = %name,
                "migrated legacy iteration-pending marker from {} to state-dir",
                legacy_path.display()
            );
        }
        if let Err(e) = std::fs::remove_file(&legacy_path) {
            tracing::warn!(
                workspace = %workspace.display(),
                change = %name,
                "migrated iteration-pending marker but failed to delete legacy workspace copy at {}: {e:#}",
                legacy_path.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::test_daemon_paths;
    use tempfile::TempDir;

    fn sample_marker() -> IterationPendingMarker {
        IterationPendingMarker {
            completed_tasks: vec!["1".into(), "2".into()],
            remaining_tasks: vec!["3".into()],
            reason: "task 3 needs a refactor I want to plan more carefully".into(),
            iteration_number: 2,
        }
    }

    fn make_change_dir(workspace: &Path, name: &str) {
        std::fs::create_dir_all(workspace.join("openspec/changes").join(name)).unwrap();
    }

    fn write_legacy_marker(workspace: &Path, change: &str, marker: &IterationPendingMarker) {
        make_change_dir(workspace, change);
        let path = workspace
            .join("openspec/changes")
            .join(change)
            .join(MARKER_FILE);
        std::fs::write(&path, serde_json::to_string_pretty(marker).unwrap()).unwrap();
    }

    #[test]
    fn marker_lives_under_state_dir_not_workspace() {
        let (_temp, paths) = test_daemon_paths();
        write_marker(&paths, "ws1", "a35-foo", &sample_marker()).unwrap();
        let target = paths.iteration_pending_path("ws1", "a35-foo");
        assert!(target.is_file(), "marker should be at {}", target.display());
        assert!(
            target.starts_with(&paths.state),
            "marker must live under state_dir, got {}",
            target.display()
        );
    }

    #[test]
    fn read_round_trips_what_write_persisted() {
        let (_temp, paths) = test_daemon_paths();
        write_marker(&paths, "ws1", "a35-foo", &sample_marker()).unwrap();
        let got = read_marker(&paths, "ws1", "a35-foo").unwrap().unwrap();
        assert_eq!(got, sample_marker());
    }

    #[test]
    fn read_absent_returns_none() {
        let (_temp, paths) = test_daemon_paths();
        let got = read_marker(&paths, "ws1", "nonexistent").unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn read_corrupt_returns_err() {
        let (_temp, paths) = test_daemon_paths();
        let dir = paths.iteration_pending_basename_dir("ws1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(paths.iteration_pending_path("ws1", "corrupt"), "not-json{").unwrap();
        assert!(read_marker(&paths, "ws1", "corrupt").is_err());
    }

    #[test]
    fn write_replaces_existing_marker() {
        let (_temp, paths) = test_daemon_paths();
        write_marker(&paths, "ws1", "a35-foo", &sample_marker()).unwrap();
        let mut second = sample_marker();
        second.iteration_number = 3;
        second.reason = "updated reason".into();
        write_marker(&paths, "ws1", "a35-foo", &second).unwrap();
        let got = read_marker(&paths, "ws1", "a35-foo").unwrap().unwrap();
        assert_eq!(got, second);
    }

    #[test]
    fn remove_makes_exists_false() {
        let (_temp, paths) = test_daemon_paths();
        write_marker(&paths, "ws1", "a35-foo", &sample_marker()).unwrap();
        assert!(marker_exists(&paths, "ws1", "a35-foo"));
        remove_marker(&paths, "ws1", "a35-foo").unwrap();
        assert!(!marker_exists(&paths, "ws1", "a35-foo"));
    }

    #[test]
    fn remove_absent_is_ok() {
        let (_temp, paths) = test_daemon_paths();
        // No marker written yet; remove should be idempotent.
        assert!(remove_marker(&paths, "ws1", "nonexistent").is_ok());
    }

    #[test]
    fn workspaces_are_isolated_by_basename() {
        let (_temp, paths) = test_daemon_paths();
        write_marker(&paths, "ws-a", "a35-foo", &sample_marker()).unwrap();
        assert!(marker_exists(&paths, "ws-a", "a35-foo"));
        assert!(!marker_exists(&paths, "ws-b", "a35-foo"));
        let mut b_marker = sample_marker();
        b_marker.iteration_number = 99;
        write_marker(&paths, "ws-b", "a35-foo", &b_marker).unwrap();
        let a = read_marker(&paths, "ws-a", "a35-foo").unwrap().unwrap();
        let b = read_marker(&paths, "ws-b", "a35-foo").unwrap().unwrap();
        assert_eq!(a.iteration_number, 2);
        assert_eq!(b.iteration_number, 99);
    }

    #[test]
    fn list_pending_changes_empty_when_no_markers() {
        let (_temp, paths) = test_daemon_paths();
        assert_eq!(list_pending_changes(&paths, "ws1"), Vec::<String>::new());
    }

    #[test]
    fn list_pending_changes_returns_single_marked_change() {
        let (_temp, paths) = test_daemon_paths();
        write_marker(&paths, "ws1", "a35-foo", &sample_marker()).unwrap();
        assert_eq!(list_pending_changes(&paths, "ws1"), vec!["a35-foo".to_string()]);
    }

    #[test]
    fn list_pending_changes_returns_multiple_in_sorted_order() {
        let (_temp, paths) = test_daemon_paths();
        write_marker(&paths, "ws1", "a40-zeta", &sample_marker()).unwrap();
        write_marker(&paths, "ws1", "a35-alpha", &sample_marker()).unwrap();
        write_marker(&paths, "ws1", "a37-beta", &sample_marker()).unwrap();
        assert_eq!(
            list_pending_changes(&paths, "ws1"),
            vec![
                "a35-alpha".to_string(),
                "a37-beta".to_string(),
                "a40-zeta".to_string(),
            ]
        );
    }

    #[test]
    fn list_pending_changes_is_workspace_scoped() {
        let (_temp, paths) = test_daemon_paths();
        write_marker(&paths, "ws-a", "a35-foo", &sample_marker()).unwrap();
        write_marker(&paths, "ws-b", "a99-bar", &sample_marker()).unwrap();
        assert_eq!(list_pending_changes(&paths, "ws-a"), vec!["a35-foo".to_string()]);
        assert_eq!(list_pending_changes(&paths, "ws-b"), vec!["a99-bar".to_string()]);
    }

    // ============================================================
    // Legacy-workspace-marker migration
    // ============================================================

    #[test]
    fn migrate_legacy_no_op_when_changes_dir_absent() {
        let (_temp, paths) = test_daemon_paths();
        let ws_dir = TempDir::new().unwrap();
        migrate_legacy_workspace_markers(&paths, ws_dir.path(), "ws1").unwrap();
        assert_eq!(list_pending_changes(&paths, "ws1"), Vec::<String>::new());
    }

    #[test]
    fn migrate_legacy_moves_valid_marker_to_state_dir() {
        let (_temp, paths) = test_daemon_paths();
        let ws_dir = TempDir::new().unwrap();
        let ws = ws_dir.path();
        let marker = sample_marker();
        write_legacy_marker(ws, "a35-foo", &marker);
        let legacy_path = ws.join("openspec/changes/a35-foo").join(MARKER_FILE);
        assert!(legacy_path.is_file());

        migrate_legacy_workspace_markers(&paths, ws, "ws1").unwrap();

        // State-dir copy exists AND has the payload.
        let got = read_marker(&paths, "ws1", "a35-foo").unwrap().unwrap();
        assert_eq!(got, marker);
        // Legacy workspace copy was deleted.
        assert!(
            !legacy_path.exists(),
            "legacy marker should have been deleted post-migration: {}",
            legacy_path.display()
        );
    }

    #[test]
    fn migrate_legacy_handles_multiple_changes() {
        let (_temp, paths) = test_daemon_paths();
        let ws_dir = TempDir::new().unwrap();
        let ws = ws_dir.path();
        write_legacy_marker(ws, "a35-foo", &sample_marker());
        write_legacy_marker(ws, "a37-bar", &sample_marker());
        write_legacy_marker(ws, "a40-baz", &sample_marker());

        migrate_legacy_workspace_markers(&paths, ws, "ws1").unwrap();

        assert_eq!(
            list_pending_changes(&paths, "ws1"),
            vec![
                "a35-foo".to_string(),
                "a37-bar".to_string(),
                "a40-baz".to_string(),
            ]
        );
        for change in &["a35-foo", "a37-bar", "a40-baz"] {
            let legacy = ws.join("openspec/changes").join(change).join(MARKER_FILE);
            assert!(!legacy.exists(), "legacy {} should be gone", legacy.display());
        }
    }

    #[test]
    fn migrate_legacy_corrupt_file_left_in_place() {
        let (_temp, paths) = test_daemon_paths();
        let ws_dir = TempDir::new().unwrap();
        let ws = ws_dir.path();
        make_change_dir(ws, "a35-foo");
        let legacy_path = ws.join("openspec/changes/a35-foo").join(MARKER_FILE);
        std::fs::write(&legacy_path, "not-json{").unwrap();

        migrate_legacy_workspace_markers(&paths, ws, "ws1").unwrap();

        // Corrupt legacy file is left for operator inspection.
        assert!(legacy_path.is_file(), "corrupt legacy file must NOT be deleted");
        // No state-dir marker was created.
        assert!(!marker_exists(&paths, "ws1", "a35-foo"));
    }

    #[test]
    fn migrate_legacy_state_dir_copy_supersedes_legacy() {
        let (_temp, paths) = test_daemon_paths();
        let ws_dir = TempDir::new().unwrap();
        let ws = ws_dir.path();
        // State-dir copy already exists (from a prior run).
        let mut state_dir_marker = sample_marker();
        state_dir_marker.iteration_number = 5;
        write_marker(&paths, "ws1", "a35-foo", &state_dir_marker).unwrap();
        // Stale legacy workspace copy from before the migration.
        let mut legacy_marker = sample_marker();
        legacy_marker.iteration_number = 2;
        write_legacy_marker(ws, "a35-foo", &legacy_marker);
        let legacy_path = ws.join("openspec/changes/a35-foo").join(MARKER_FILE);

        migrate_legacy_workspace_markers(&paths, ws, "ws1").unwrap();

        // State-dir copy is preserved (NOT overwritten by the older legacy).
        let got = read_marker(&paths, "ws1", "a35-foo").unwrap().unwrap();
        assert_eq!(got.iteration_number, 5, "state-dir copy must remain authoritative");
        // Legacy workspace copy is deleted.
        assert!(!legacy_path.exists());
    }

    #[test]
    fn migrate_legacy_idempotent_on_second_call() {
        let (_temp, paths) = test_daemon_paths();
        let ws_dir = TempDir::new().unwrap();
        let ws = ws_dir.path();
        write_legacy_marker(ws, "a35-foo", &sample_marker());

        migrate_legacy_workspace_markers(&paths, ws, "ws1").unwrap();
        // Second call: nothing to migrate, no error.
        migrate_legacy_workspace_markers(&paths, ws, "ws1").unwrap();

        assert!(marker_exists(&paths, "ws1", "a35-foo"));
    }
}
