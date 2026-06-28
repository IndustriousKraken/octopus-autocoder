//! Per-workspace push-block marker. When a pass-level branch push fails AFTER
//! one or more changes were committed (and archived) on the agent branch, the
//! completed work is preserved on the branch and a marker is written to the
//! daemon STATE directory (keyed to the workspace, NOT a change directory — the
//! carried changes are already archived). The marker records the unpushed tip,
//! the carried change slugs, and the rejection reason. It anchors branch
//! preservation (a present marker whose tip still matches the agent branch tip
//! means "do not recreate the branch — retry the push"). Written only on a real
//! push failure, removed only on a successful push, so it never falsely triggers
//! on a stale branch.

use crate::paths::DaemonPaths;
use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushBlock {
    /// The unpushed agent-branch tip commit at the time of the failed push.
    /// Branch preservation requires the live tip to still match this.
    pub tip_commit: String,
    /// The change slug(s) whose commits the failed push was carrying.
    pub change_slugs: Vec<String>,
    /// The git push rejection reason (captured stderr).
    pub reason: String,
    pub blocked_at: DateTime<Utc>,
}

fn basename(workspace: &Path) -> String {
    workspace
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string())
}

/// True when a push-block marker file exists for the workspace.
pub fn exists(paths: &DaemonPaths, workspace: &Path) -> bool {
    paths.push_block_path(&basename(workspace)).exists()
}

/// Read the push-block marker, or None if absent/unparseable.
pub fn read(paths: &DaemonPaths, workspace: &Path) -> Option<PushBlock> {
    let path = paths.push_block_path(&basename(workspace));
    let raw = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Atomically write the push-block marker (temp-then-rename).
pub fn write(paths: &DaemonPaths, workspace: &Path, marker: &PushBlock) -> Result<()> {
    let dir = paths.push_block_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating push-block dir {}", dir.display()))?;
    let path = paths.push_block_path(&basename(workspace));
    let tmp = tempfile::NamedTempFile::new_in(&dir)
        .with_context(|| format!("creating tempfile in {}", dir.display()))?;
    serde_json::to_writer_pretty(&tmp, marker)
        .with_context(|| format!("serializing push-block marker {}", path.display()))?;
    tmp.persist(&path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

/// Idempotent removal — a missing marker is success.
pub fn clear(paths: &DaemonPaths, workspace: &Path) -> Result<()> {
    let path = paths.push_block_path(&basename(workspace));
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
    fn write_read_clear_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = DaemonPaths::under_root(tmp.path());
        let ws = tmp.path().join("workspaces").join("repo-x");
        std::fs::create_dir_all(&ws).unwrap();

        assert!(!exists(&paths, &ws));
        assert!(read(&paths, &ws).is_none());

        let marker = PushBlock {
            tip_commit: "deadbeef".into(),
            change_slugs: vec!["foo".into(), "bar".into()],
            reason: "remote: error: GH006 Protected branch update failed".into(),
            blocked_at: Utc::now(),
        };
        write(&paths, &ws, &marker).unwrap();
        assert!(exists(&paths, &ws));

        let got = read(&paths, &ws).unwrap();
        assert_eq!(got.tip_commit, "deadbeef");
        assert_eq!(got.change_slugs, vec!["foo", "bar"]);

        clear(&paths, &ws).unwrap();
        assert!(!exists(&paths, &ws));
        clear(&paths, &ws).unwrap(); // idempotent
    }
}
