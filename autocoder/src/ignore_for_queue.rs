//! Per-change ignore-for-queue marker. When an operator runs
//! `@<bot> ignore-and-continue <repo> <change>`, autocoder writes
//! `<workspace>/openspec/changes/<change>/.ignore-for-queue.json`. The
//! marker's presence downgrades any sibling operator-action marker
//! (`.perma-stuck.json`, `.needs-spec-revision.json`) from "blocks
//! subsequent queue processing" to "still excludes this change, but
//! doesn't block siblings." It is the operator's explicit "I know this
//! change is broken; skip it AND proceed with the rest" signal.

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::spec_root::SpecRoot;

const MARKER_FILE: &str = ".ignore-for-queue.json";
const DEFAULT_REASON: &str = "operator-driven skip; original marker(s) preserved";
const DEFAULT_OPERATOR_ACTION: &str =
    "Delete this file (or use @<bot> clear-ignore) to re-block the queue on the original marker.";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IgnoreForQueueMarker {
    pub change: String,
    pub marked_at: DateTime<Utc>,
    pub marked_by: String,
    #[serde(default = "default_reason")]
    pub reason: String,
    #[serde(default = "default_operator_action")]
    pub operator_action: String,
}

fn default_reason() -> String {
    DEFAULT_REASON.to_string()
}

fn default_operator_action() -> String {
    DEFAULT_OPERATOR_ACTION.to_string()
}

fn marker_path(spec_root: &SpecRoot, change: &str) -> PathBuf {
    spec_root.changes_dir().join(change).join(MARKER_FILE)
}

/// True when `<spec_root>/changes/<change>/.ignore-for-queue.json`
/// exists. Pure filesystem check — no JSON parsing.
pub fn marker_exists(spec_root: &SpecRoot, change: &str) -> bool {
    marker_path(spec_root, change).exists()
}

/// Write the marker file atomically (tempfile + rename in the change
/// directory). The change directory must already exist.
pub fn write_marker(spec_root: &SpecRoot, change: &str, marked_by: &str) -> Result<()> {
    let path = marker_path(spec_root, change);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("destination path has no parent: {}", path.display()))?;
    if !parent.is_dir() {
        return Err(anyhow!(
            "change directory does not exist: {}",
            parent.display()
        ));
    }
    let marker = IgnoreForQueueMarker {
        change: change.to_string(),
        marked_at: Utc::now(),
        marked_by: marked_by.to_string(),
        reason: DEFAULT_REASON.to_string(),
        operator_action: DEFAULT_OPERATOR_ACTION.to_string(),
    };
    let tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating tempfile in {}", parent.display()))?;
    serde_json::to_writer_pretty(&tmp, &marker)
        .with_context(|| format!("serializing ignore-for-queue marker for {}", path.display()))?;
    tmp.persist(&path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    fn ws_spec_root(workspace: &Path) -> SpecRoot {
        SpecRoot::from_parts(
            workspace.to_path_buf(),
            workspace.join("openspec"),
            false,
        )
    }

    fn make_change_dir(workspace: &Path, name: &str) {
        std::fs::create_dir_all(workspace.join("openspec/changes").join(name)).unwrap();
    }

    #[test]
    fn write_then_exists_returns_true() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let sr = ws_spec_root(ws);
        make_change_dir(ws, "foo");
        assert!(!marker_exists(&sr, "foo"));
        write_marker(&sr, "foo", "U_OP").unwrap();
        assert!(marker_exists(&sr, "foo"));
        let raw = std::fs::read_to_string(
            ws.join("openspec/changes/foo/.ignore-for-queue.json"),
        )
        .unwrap();
        assert!(raw.contains("\"change\""));
        assert!(raw.contains("\"foo\""));
        assert!(raw.contains("\"marked_at\""));
        assert!(raw.contains("\"marked_by\""));
        assert!(raw.contains("\"U_OP\""));
        assert!(raw.contains("\"reason\""));
        assert!(raw.contains("\"operator_action\""));
        assert!(raw.contains("clear-ignore"));
    }

    #[test]
    fn write_marker_errors_when_change_directory_absent() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let sr = ws_spec_root(ws);
        let err = write_marker(&sr, "missing", "U")
            .expect_err("write_marker should fail when change dir is absent");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("change directory does not exist"),
            "error must mention missing change dir: {msg}"
        );
    }

    #[test]
    fn round_trip_marker_struct() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let sr = ws_spec_root(ws);
        make_change_dir(ws, "foo");
        write_marker(&sr, "foo", "U_OP_42").unwrap();
        let raw = std::fs::read_to_string(
            ws.join("openspec/changes/foo/.ignore-for-queue.json"),
        )
        .unwrap();
        let parsed: IgnoreForQueueMarker = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.change, "foo");
        assert_eq!(parsed.marked_by, "U_OP_42");
        assert_eq!(parsed.reason, DEFAULT_REASON);
        assert_eq!(parsed.operator_action, DEFAULT_OPERATOR_ACTION);
        let age = (Utc::now() - parsed.marked_at).num_seconds().abs();
        assert!(age < 5, "marked_at must be ~now; age = {age}s");
    }

    #[test]
    fn deserialize_with_missing_defaults_uses_sensible_defaults() {
        // Minimal JSON missing the optional fields `reason` AND
        // `operator_action`. Both should fall back to the default
        // constants via `#[serde(default)]`.
        let raw = r#"{
            "change": "foo",
            "marked_at": "2026-05-27T20:30:00Z",
            "marked_by": "U_OP"
        }"#;
        let parsed: IgnoreForQueueMarker = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.change, "foo");
        assert_eq!(parsed.marked_by, "U_OP");
        assert_eq!(parsed.reason, DEFAULT_REASON);
        assert_eq!(parsed.operator_action, DEFAULT_OPERATOR_ACTION);
    }
}
