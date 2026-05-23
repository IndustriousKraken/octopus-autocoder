//! Per-change spec-needs-revision marker. When the executor returns
//! `ExecutorOutcome::SpecNeedsRevision` for a change, autocoder writes
//! `<workspace>/openspec/changes/<change>/.needs-spec-revision.json`. The
//! marker's presence is a presence-only flag consulted by
//! `queue::list_pending` — the change is excluded from the queue until the
//! operator removes the marker manually (typically after editing tasks.md
//! to remove or revise the flagged tasks).

use crate::executor::UnimplementableTask;
use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MARKER_FILE: &str = ".needs-spec-revision.json";
const OPERATOR_ACTION: &str = "Edit openspec/changes/<change>/tasks.md to remove or revise the flagged tasks, commit + push, then delete this marker file.";

/// Outcome details captured at the moment the executor returned
/// `SpecNeedsRevision`. Used as input to `write_marker`.
#[derive(Debug, Clone)]
pub struct SpecNeedsRevisionDetail {
    pub unimplementable_tasks: Vec<UnimplementableTask>,
    pub revision_suggestion: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecNeedsRevisionMarker {
    pub change: String,
    pub marked_at: DateTime<Utc>,
    pub unimplementable_tasks: Vec<UnimplementableTask>,
    pub revision_suggestion: String,
    pub operator_action: String,
}

fn marker_path(workspace: &Path, change: &str) -> PathBuf {
    workspace
        .join("openspec/changes")
        .join(change)
        .join(MARKER_FILE)
}

/// True when `<workspace>/openspec/changes/<change>/.needs-spec-revision.json`
/// exists. Pure filesystem check — no JSON parsing.
pub fn marker_exists(workspace: &Path, change: &str) -> bool {
    marker_path(workspace, change).exists()
}

/// Write the marker file atomically (tempfile + rename in the change
/// directory). The change directory must already exist.
pub fn write_marker(
    workspace: &Path,
    change: &str,
    detail: &SpecNeedsRevisionDetail,
) -> Result<()> {
    let path = marker_path(workspace, change);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("destination path has no parent: {}", path.display()))?;
    if !parent.is_dir() {
        return Err(anyhow!(
            "change directory does not exist: {}",
            parent.display()
        ));
    }
    let marker = SpecNeedsRevisionMarker {
        change: change.to_string(),
        marked_at: Utc::now(),
        unimplementable_tasks: detail.unimplementable_tasks.clone(),
        revision_suggestion: detail.revision_suggestion.clone(),
        operator_action: OPERATOR_ACTION.to_string(),
    };
    let tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating tempfile in {}", parent.display()))?;
    serde_json::to_writer_pretty(&tmp, &marker).with_context(|| {
        format!("serializing spec-needs-revision marker for {}", path.display())
    })?;
    tmp.persist(&path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

/// Idempotent removal of the marker. A missing file is success.
pub fn remove_marker(workspace: &Path, change: &str) -> Result<()> {
    let path = marker_path(workspace, change);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_change_dir(workspace: &Path, name: &str) {
        std::fs::create_dir_all(workspace.join("openspec/changes").join(name)).unwrap();
    }

    fn fixture_detail() -> SpecNeedsRevisionDetail {
        SpecNeedsRevisionDetail {
            unimplementable_tasks: vec![
                UnimplementableTask {
                    task_id: "5.2".into(),
                    task_text: "install actionlint locally".into(),
                    reason: "no apt access in sandbox".into(),
                },
                UnimplementableTask {
                    task_id: "15.3".into(),
                    task_text: "smoke-test on macOS".into(),
                    reason: "no macOS host available".into(),
                },
            ],
            revision_suggestion:
                "Replace 5.2 with a CI gate. Drop 15.3 — the workflow's own first real run is the integration test.".into(),
        }
    }

    #[test]
    fn write_then_exists_returns_true() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        make_change_dir(ws, "foo");
        assert!(!marker_exists(ws, "foo"));
        write_marker(ws, "foo", &fixture_detail()).unwrap();
        assert!(marker_exists(ws, "foo"));
    }

    #[test]
    fn write_marker_roundtrips_all_fields() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        make_change_dir(ws, "foo");
        let detail = fixture_detail();
        write_marker(ws, "foo", &detail).unwrap();

        let raw = std::fs::read_to_string(
            ws.join("openspec/changes/foo/.needs-spec-revision.json"),
        )
        .unwrap();
        let parsed: SpecNeedsRevisionMarker = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.change, "foo");
        assert_eq!(parsed.unimplementable_tasks, detail.unimplementable_tasks);
        assert_eq!(parsed.revision_suggestion, detail.revision_suggestion);
        assert!(parsed
            .operator_action
            .contains("delete this marker file"));
        // marked_at is recent.
        let age = (Utc::now() - parsed.marked_at).num_seconds().abs();
        assert!(age < 5, "marked_at must be ~now; age = {age}s");
    }

    #[test]
    fn write_marker_errors_when_change_directory_absent() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let detail = fixture_detail();
        let err = write_marker(ws, "missing", &detail)
            .expect_err("write_marker should fail when change dir is absent");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("change directory does not exist"),
            "error must mention missing change dir: {msg}"
        );
    }
}
