//! Per-workspace push-block marker. When a pass-level branch push fails — OR the
//! pass-level PR creation fails after a successful push — AFTER one or more
//! changes or issue units were committed (and archived) on the agent branch, the
//! completed work is preserved on the branch and a marker is written to the
//! daemon STATE directory (keyed to the workspace, NOT a change directory — the
//! carried units are already archived). The marker records the unpushed-or-
//! unannounced tip, the carried change AND issue slugs, the rejection reason, and
//! which delivery step failed. It anchors branch preservation (a present marker
//! whose tip still matches the agent branch tip means "do not recreate the branch
//! — retry the remaining delivery step"). Written only on a real push or
//! PR-creation failure, removed only when the held work completes, so it never
//! falsely triggers on a stale branch.

use crate::paths::DaemonPaths;
use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Which delivery step failed when the hold was written. Diagnostic only — the
/// resume path retries the remaining delivery steps regardless of this value
/// (for a `PrCreation` hold the tip is already on the remote, so the push retry
/// is a no-op and PR creation is the effective retry). Serde-defaults to `Push`
/// so markers written before this field existed deserialize unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FailedStep {
    #[default]
    Push,
    PrCreation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushBlock {
    /// The unpushed-or-unannounced agent-branch tip commit at the time of the
    /// failure. Branch preservation requires the live tip to still match this.
    pub tip_commit: String,
    /// The change slug(s) whose commits the failed push was carrying.
    pub change_slugs: Vec<String>,
    /// The issue slug(s) whose commits the pass was carrying, alongside
    /// `change_slugs`. Serde-defaults to empty so legacy markers deserialize
    /// unchanged.
    #[serde(default)]
    pub issue_slugs: Vec<String>,
    /// The git push rejection reason (captured stderr).
    pub reason: String,
    pub blocked_at: DateTime<Utc>,
    /// Which delivery step failed (the push, or PR creation after a successful
    /// push). Diagnostic; the resume path is identical for both. Serde-defaults
    /// to `Push` so legacy markers deserialize unchanged.
    #[serde(default)]
    pub failed_step: FailedStep,
    /// Code-review report from the original pass. Preserved so the resumed
    /// open_pull_request call can include the review in the PR body instead
    /// of silently dropping it.
    #[serde(default)]
    pub review_report: Option<crate::code_reviewer::ReviewReport>,
    /// Rendered `## Spec Verification` PR body section from the original pass.
    #[serde(default)]
    pub spec_verification_section: Option<String>,
    /// Rendered `## Gate verdicts` PR body section from the original pass.
    #[serde(default)]
    pub gate_verdicts_section: Option<String>,
    /// app-under-test-e2e: rendered `## End-to-end verification` PR body
    /// section from the original pass.
    ///
    /// Carried on the marker for the same reason as the sections above: the
    /// held work is delivered by a LATER pass, which will not re-run the
    /// suite (the application for that earlier pass is long gone). Re-deriving
    /// it later would be impossible; omitting it would silently drop the
    /// verification record from the eventual PR. `#[serde(default)]` keeps
    /// markers written before this field readable.
    #[serde(default)]
    pub e2e_section: Option<String>,
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
            issue_slugs: vec!["fix-baz".into()],
            reason: "remote: error: GH006 Protected branch update failed".into(),
            blocked_at: Utc::now(),
            failed_step: FailedStep::PrCreation,
            review_report: None,
            spec_verification_section: None,
            gate_verdicts_section: None,
            e2e_section: None,
        };
        write(&paths, &ws, &marker).unwrap();
        assert!(exists(&paths, &ws));

        let got = read(&paths, &ws).unwrap();
        assert_eq!(got.tip_commit, "deadbeef");
        assert_eq!(got.change_slugs, vec!["foo", "bar"]);
        assert_eq!(got.issue_slugs, vec!["fix-baz"]);
        assert_eq!(got.failed_step, FailedStep::PrCreation);

        clear(&paths, &ws).unwrap();
        assert!(!exists(&paths, &ws));
        clear(&paths, &ws).unwrap(); // idempotent
    }

    /// Task 4.3: a marker written before `failed_step` / `issue_slugs` existed
    /// (neither key present in the JSON) still deserializes — `failed_step`
    /// defaults to `Push` and `issue_slugs` to empty — so it resumes as a plain
    /// push hold.
    #[test]
    fn legacy_marker_without_new_fields_deserializes_as_push_hold() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = DaemonPaths::under_root(tmp.path());
        let ws = tmp.path().join("workspaces").join("repo-legacy");
        std::fs::create_dir_all(&ws).unwrap();

        // Hand-write the on-disk shape a pre-change daemon produced: no
        // `failed_step`, no `issue_slugs`.
        let legacy = serde_json::json!({
            "tip_commit": "cafebabe",
            "change_slugs": ["foo"],
            "reason": "remote rejected",
            "blocked_at": Utc::now(),
        });
        let dir = paths.push_block_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            paths.push_block_path(&basename(&ws)),
            serde_json::to_string_pretty(&legacy).unwrap(),
        )
        .unwrap();

        let got = read(&paths, &ws).expect("legacy marker must deserialize");
        assert_eq!(got.tip_commit, "cafebabe");
        assert_eq!(got.change_slugs, vec!["foo"]);
        assert_eq!(
            got.failed_step,
            FailedStep::Push,
            "missing failed_step must default to Push (a legacy push hold)"
        );
        assert!(
            got.issue_slugs.is_empty(),
            "missing issue_slugs must default to empty"
        );
    }
}
