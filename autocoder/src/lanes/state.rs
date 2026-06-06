//! The issues lane's OWN per-unit state file (a009 §3.2).
//!
//! Each lane reads AND writes only its own lane's state — fault
//! isolation between lanes. The changes walker persists its
//! consecutive-failure counters under `<state>/failure-state/...`
//! (`crate::failure_state`); the issues walker persists its OWN counters
//! under `<state>/issues-state/<repo-basename>/<slug>.json`. The two
//! directories are disjoint, so a fault in one walker cannot corrupt the
//! other lane's state.
//!
//! Shape mirrors `crate::failure_state`: one JSON file per issue slug,
//! incremented on a failed issue run AND cleared when the issue archives.

use crate::paths::DaemonPaths;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueFailureEntry {
    pub count: u32,
    pub last_reason: String,
    pub last_failed_at: DateTime<Utc>,
}

/// Per-repo issues-state directory under `<state>/issues-state/`.
fn repo_dir(paths: &DaemonPaths, workspace: &Path) -> PathBuf {
    let basename = workspace
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string());
    paths.issues_state_dir().join(basename)
}

fn slug_file(paths: &DaemonPaths, workspace: &Path, slug: &str) -> PathBuf {
    repo_dir(paths, workspace).join(format!("{slug}.json"))
}

/// Read the current failure count for `slug` (0 when no entry exists).
/// Forward-looking read API for an issues perma-stuck gate (the changes
/// lane's analogue); the walker records + clears counters today.
#[allow(dead_code)]
pub fn failure_count(paths: &DaemonPaths, workspace: &Path, slug: &str) -> u32 {
    let path = slug_file(paths, workspace, slug);
    match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str::<IssueFailureEntry>(&raw)
            .map(|e| e.count)
            .unwrap_or(0),
        Err(_) => 0,
    }
}

/// Increment the failure counter for `slug`, recording the reason AND
/// timestamp. Creates the entry if absent. Returns the new count.
pub fn record_failure(
    paths: &DaemonPaths,
    workspace: &Path,
    slug: &str,
    reason: &str,
) -> Result<u32> {
    let path = slug_file(paths, workspace, slug);
    let mut entry = match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str::<IssueFailureEntry>(&raw).unwrap_or(IssueFailureEntry {
            count: 0,
            last_reason: String::new(),
            last_failed_at: Utc::now(),
        }),
        Err(_) => IssueFailureEntry {
            count: 0,
            last_reason: String::new(),
            last_failed_at: Utc::now(),
        },
    };
    entry.count += 1;
    entry.last_reason = reason.to_string();
    entry.last_failed_at = Utc::now();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(&entry)?;
    std::fs::write(&path, raw).with_context(|| format!("writing {}", path.display()))?;
    Ok(entry.count)
}

/// Clear the failure entry for `slug`. Idempotent: absent file is fine.
pub fn clear(paths: &DaemonPaths, workspace: &Path, slug: &str) -> Result<()> {
    let path = slug_file(paths, workspace, slug);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_increments_and_clear_resets() {
        let (_td, paths) = crate::testing::test_daemon_paths();
        let ws = Path::new("/tmp/some-workspace-basename");
        assert_eq!(failure_count(&paths, ws, "iss"), 0);
        assert_eq!(record_failure(&paths, ws, "iss", "boom").unwrap(), 1);
        assert_eq!(record_failure(&paths, ws, "iss", "boom again").unwrap(), 2);
        assert_eq!(failure_count(&paths, ws, "iss"), 2);
        clear(&paths, ws, "iss").unwrap();
        assert_eq!(failure_count(&paths, ws, "iss"), 0);
        // Idempotent clear.
        clear(&paths, ws, "iss").unwrap();
    }

    #[test]
    fn issues_state_dir_is_disjoint_from_failure_state_dir() {
        let (_td, paths) = crate::testing::test_daemon_paths();
        // The issues lane's state directory must not be the changes
        // lane's failure-state directory — separate state per lane.
        assert_ne!(paths.issues_state_dir(), paths.failure_state_dir());
    }
}
