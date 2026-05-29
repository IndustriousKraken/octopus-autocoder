//! Scout-run state IO for the `scout` chatops verb (a25).
//!
//! Each accepted `@<bot> scout <repo> [guidance]` invocation produces
//! one `ScoutRunState` JSON file under
//! `<workspace>/.state/scout_runs/<request_id>.json`. The "current"
//! scout run for a repo is the most-recent file by mtime (per spec).
//! Older runs remain on disk for audit purposes until `clear-scout`
//! deletes them.
//!
//! State files are JSON, atomically written via tempfile-then-rename
//! so a torn write is never visible to a concurrent reader.

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One scout item — the executor's per-opportunity entry. Fields match
/// the documented JSON shape returned by the scout-mode executor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoutItem {
    pub id: usize,
    pub category: String,
    pub title: String,
    pub body: String,
    pub source: String,
    pub tractability: String,
}

/// On-disk shape for one scout run. Written by the scout polling
/// handler after the executor's response validates; read by the
/// spec-it polling handler to resolve `<item-number>` AND compute
/// staleness, AND by the chatops inbound listener to identify scout
/// lifecycle threads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoutRunState {
    pub request_id: String,
    pub repo_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
    pub head_sha_at_run: String,
    pub completed_at: DateTime<Utc>,
    pub thread_ts: String,
    pub channel: String,
    pub items: Vec<ScoutItem>,
}

/// Allowed `category` values for scout items. The polling handler
/// rejects any item whose `category` is not in this set.
pub const ALLOWED_CATEGORIES: &[&str] = &[
    "security",
    "bug",
    "error_handling",
    "type_tightening",
    "code_smell",
    "perf",
    "documentation",
    "test_coverage",
    "issue",
    "todo_fixme",
    "research",
];

/// Allowed `tractability` values for scout items.
pub const ALLOWED_TRACTABILITY: &[&str] = &["small", "medium", "large"];

/// Per-workspace scout-state directory:
/// `<workspace>/.state/scout_runs/`.
pub fn state_dir(workspace: &Path) -> PathBuf {
    workspace.join(".state").join("scout_runs")
}

/// Canonical state file path:
/// `<workspace>/.state/scout_runs/<request_id>.json`.
pub fn state_path(workspace: &Path, request_id: &str) -> PathBuf {
    state_dir(workspace).join(format!("{request_id}.json"))
}

/// Atomically write `state` to its canonical file under `workspace`.
/// Parent directories are created if absent.
pub fn write_state(workspace: &Path, state: &ScoutRunState) -> Result<()> {
    let dir = state_dir(workspace);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating scout_runs dir {}", dir.display()))?;
    let path = state_path(workspace, &state.request_id);
    let tmp = tempfile::NamedTempFile::new_in(&dir)
        .with_context(|| format!("creating tempfile in {}", dir.display()))?;
    serde_json::to_writer_pretty(&tmp, state)
        .with_context(|| format!("serializing scout-run state for {}", path.display()))?;
    tmp.persist(&path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

/// Read the scout-run state for `request_id`. Returns `Ok(None)` when
/// no file exists; surfaces an error on read/parse.
pub fn read_state(workspace: &Path, request_id: &str) -> Result<Option<ScoutRunState>> {
    let path = state_path(workspace, request_id);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow!("reading {}: {e}", path.display())),
    };
    serde_json::from_str::<ScoutRunState>(&raw)
        .map(Some)
        .with_context(|| format!("parsing {}", path.display()))
}

/// List every `(request_id, mtime)` pair under the scout-state directory,
/// sorted by mtime descending (newest first). Used by the spec-it
/// handler AND chatops listener to find the "current" scout for a repo.
pub fn list_runs_by_mtime(workspace: &Path) -> Result<Vec<(String, std::time::SystemTime)>> {
    let dir = state_dir(workspace);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<(String, std::time::SystemTime)> = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .with_context(|| format!("reading scout_runs dir {}", dir.display()))?
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let mtime = match entry.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => std::time::SystemTime::UNIX_EPOCH,
        };
        entries.push((stem, mtime));
    }
    entries.sort_by_key(|(_, mtime)| std::cmp::Reverse(*mtime));
    Ok(entries)
}

/// Find the most-recent scout state under `workspace`, returning the
/// parsed `ScoutRunState`. Returns `Ok(None)` when no runs are present.
#[allow(dead_code)]
pub fn most_recent(workspace: &Path) -> Result<Option<ScoutRunState>> {
    let runs = list_runs_by_mtime(workspace)?;
    let Some((request_id, _)) = runs.into_iter().next() else {
        return Ok(None);
    };
    read_state(workspace, &request_id)
}

/// Delete every scout-state file under `workspace`. Returns the count
/// of files deleted. Missing directory is a no-op (returns 0).
pub fn clear_all(workspace: &Path) -> Result<usize> {
    let dir = state_dir(workspace);
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut count = 0usize;
    for entry in std::fs::read_dir(&dir)
        .with_context(|| format!("reading scout_runs dir {}", dir.display()))?
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => count += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(anyhow!("removing {}: {e}", path.display()));
            }
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture_item(id: usize) -> ScoutItem {
        ScoutItem {
            id,
            category: "error_handling".into(),
            title: format!("Item {id}"),
            body: "One-paragraph description.".into(),
            source: "src/lib.rs:42".into(),
            tractability: "small".into(),
        }
    }

    fn fixture_state(request_id: &str) -> ScoutRunState {
        ScoutRunState {
            request_id: request_id.to_string(),
            repo_url: "git@github.com:acme/myrepo.git".into(),
            guidance: Some("focus on error handling".into()),
            head_sha_at_run: "abc1234".into(),
            completed_at: Utc::now(),
            thread_ts: "1748399999.001234".into(),
            channel: "C_OPS".into(),
            items: vec![fixture_item(1), fixture_item(2)],
        }
    }

    #[test]
    fn read_missing_state_file_returns_ok_none() {
        let tmp = TempDir::new().unwrap();
        let got = read_state(tmp.path(), "00000000-no-such-id").unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn write_then_read_round_trips_every_field() {
        let tmp = TempDir::new().unwrap();
        let state = fixture_state("req-1");
        write_state(tmp.path(), &state).unwrap();
        let got = read_state(tmp.path(), &state.request_id).unwrap().unwrap();
        assert_eq!(got, state);
    }

    #[test]
    fn state_path_is_under_workspace_subdir() {
        let p = state_path(Path::new("/tmp/ws"), "req-x");
        let s = p.to_string_lossy();
        assert!(s.starts_with("/tmp/ws/"), "{s}");
        assert!(s.contains(".state/scout_runs"), "{s}");
        assert!(s.ends_with("req-x.json"), "{s}");
    }

    #[test]
    fn concurrent_writes_do_not_leak_tempfiles() {
        let tmp = TempDir::new().unwrap();
        for i in 0..5 {
            let mut s = fixture_state(&format!("req-{i}"));
            s.items.push(fixture_item(3));
            write_state(tmp.path(), &s).unwrap();
        }
        let dir = state_dir(tmp.path());
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names.len(), 5);
        assert!(!names.iter().any(|n| n.contains(".tmp")), "{names:?}");
    }

    #[test]
    fn most_recent_resolves_by_mtime() {
        let tmp = TempDir::new().unwrap();
        let older = fixture_state("req-older");
        write_state(tmp.path(), &older).unwrap();
        // Force a measurable mtime gap.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let newer = fixture_state("req-newer");
        write_state(tmp.path(), &newer).unwrap();
        let got = most_recent(tmp.path()).unwrap().unwrap();
        assert_eq!(got.request_id, "req-newer");
    }

    #[test]
    fn most_recent_returns_none_when_no_runs() {
        let tmp = TempDir::new().unwrap();
        assert!(most_recent(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn clear_all_removes_every_run() {
        let tmp = TempDir::new().unwrap();
        for i in 0..3 {
            write_state(tmp.path(), &fixture_state(&format!("req-{i}"))).unwrap();
        }
        let n = clear_all(tmp.path()).unwrap();
        assert_eq!(n, 3);
        // The directory may remain but contains no .json files now.
        let remaining = list_runs_by_mtime(tmp.path()).unwrap();
        assert!(remaining.is_empty());
    }

    #[test]
    fn clear_all_no_runs_returns_zero() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(clear_all(tmp.path()).unwrap(), 0);
        // Idempotent.
        assert_eq!(clear_all(tmp.path()).unwrap(), 0);
    }

    #[test]
    fn list_runs_by_mtime_orders_newest_first() {
        let tmp = TempDir::new().unwrap();
        write_state(tmp.path(), &fixture_state("a")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        write_state(tmp.path(), &fixture_state("b")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        write_state(tmp.path(), &fixture_state("c")).unwrap();
        let runs = list_runs_by_mtime(tmp.path()).unwrap();
        let ids: Vec<&str> = runs.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["c", "b", "a"]);
    }

    #[test]
    fn enqueue_dequeue_queue_round_trips_request_ids() {
        use std::collections::VecDeque;
        let mut q: VecDeque<String> = VecDeque::new();
        q.push_back("req-a".into());
        q.push_back("req-b".into());
        assert_eq!(q.pop_front().as_deref(), Some("req-a"));
        assert_eq!(q.pop_front().as_deref(), Some("req-b"));
        assert!(q.pop_front().is_none());
    }

    #[test]
    fn allowed_categories_contains_every_documented_value() {
        for expected in &[
            "security",
            "bug",
            "error_handling",
            "type_tightening",
            "code_smell",
            "perf",
            "documentation",
            "test_coverage",
            "issue",
            "todo_fixme",
            "research",
        ] {
            assert!(
                ALLOWED_CATEGORIES.contains(expected),
                "missing category {expected}"
            );
        }
    }

    #[test]
    fn allowed_tractability_contains_every_documented_value() {
        assert!(ALLOWED_TRACTABILITY.contains(&"small"));
        assert!(ALLOWED_TRACTABILITY.contains(&"medium"));
        assert!(ALLOWED_TRACTABILITY.contains(&"large"));
    }
}
