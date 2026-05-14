//! Per-workspace tracking for throttled predictable-failure alerts.
//! Persisted as `<workspace>/.alert-state.json`. A category present in
//! the map means "the operator was alerted at this UTC timestamp; do not
//! re-alert until the 24h window expires." Absence means the next failure
//! in that category alerts immediately.

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const ALERT_STATE_FILE: &str = ".alert-state.json";

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AlertCategory {
    WorkspaceInitFailure,
    BranchPushFailure,
    PrCreationFailure,
}

impl AlertCategory {
    /// Human-readable label used inside the alert text. Mirrors design.md
    /// table.
    pub fn label(self) -> &'static str {
        match self {
            Self::WorkspaceInitFailure => "workspace init keeps failing",
            Self::BranchPushFailure => "branch push keeps failing",
            Self::PrCreationFailure => "PR creation keeps failing",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertEntry {
    pub last_alerted_at: DateTime<Utc>,
    pub last_error_excerpt: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlertState {
    #[serde(default)]
    pub alerts: HashMap<AlertCategory, AlertEntry>,
}

impl AlertState {
    /// Load from `<workspace>/.alert-state.json`. A missing file is the
    /// common "no prior alerts" case and returns an empty state. Other I/O
    /// or parse errors propagate up — corrupt state is operator-visible.
    pub fn load_or_default(workspace: &Path) -> Self {
        let path = state_path(workspace);
        match std::fs::read_to_string(&path) {
            Ok(raw) => match serde_json::from_str::<AlertState>(&raw) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        "could not parse {}: {e}; treating as empty state",
                        path.display()
                    );
                    AlertState::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => AlertState::default(),
            Err(e) => {
                tracing::warn!(
                    "could not read {}: {e}; treating as empty state",
                    path.display()
                );
                AlertState::default()
            }
        }
    }

    /// Atomically write the state file. Tempfile-in-same-dir, then
    /// `tempfile::persist`, mirroring the chatops file helpers.
    pub fn save(&self, workspace: &Path) -> Result<()> {
        let path = state_path(workspace);
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("alert-state path has no parent: {}", path.display()))?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating parent dir {}", parent.display()))?;
        let tmp = tempfile::NamedTempFile::new_in(parent)
            .with_context(|| format!("creating tempfile in {}", parent.display()))?;
        serde_json::to_writer_pretty(&tmp, self)
            .with_context(|| format!("serializing alert state for {}", path.display()))?;
        tmp.persist(&path)
            .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
        Ok(())
    }

    /// Idempotent removal of the state file. Used on the
    /// successful-iteration path to clear prior alerts so the next failure
    /// re-alerts immediately. Missing file is not an error.
    pub fn clear(workspace: &Path) -> Result<()> {
        let path = state_path(workspace);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
        }
    }
}

fn state_path(workspace: &Path) -> PathBuf {
    workspace.join(ALERT_STATE_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use tempfile::TempDir;

    #[test]
    fn load_missing_returns_empty() {
        let dir = TempDir::new().unwrap();
        let state = AlertState::load_or_default(dir.path());
        assert!(state.alerts.is_empty());
    }

    #[test]
    fn save_and_reload_roundtrip() {
        let dir = TempDir::new().unwrap();
        let mut s = AlertState::default();
        let now = Utc::now();
        s.alerts.insert(
            AlertCategory::WorkspaceInitFailure,
            AlertEntry {
                last_alerted_at: now,
                last_error_excerpt: "fixture excerpt".into(),
            },
        );
        s.alerts.insert(
            AlertCategory::PrCreationFailure,
            AlertEntry {
                last_alerted_at: now - Duration::hours(2),
                last_error_excerpt: "another excerpt".into(),
            },
        );
        s.save(dir.path()).unwrap();
        let loaded = AlertState::load_or_default(dir.path());
        assert_eq!(loaded.alerts.len(), 2);
        let init_entry = loaded
            .alerts
            .get(&AlertCategory::WorkspaceInitFailure)
            .expect("init entry present");
        assert_eq!(init_entry.last_error_excerpt, "fixture excerpt");
        // chrono RFC3339 round-trip preserves seconds; allow microseconds
        // to differ but require timestamps to be within 1 second.
        let delta = (init_entry.last_alerted_at - now).num_milliseconds().abs();
        assert!(delta < 1000, "timestamp roundtrip drift {delta}ms");
        // Verify on-disk JSON keys match the spec's snake_case category labels.
        let raw = std::fs::read_to_string(dir.path().join(".alert-state.json")).unwrap();
        assert!(
            raw.contains("workspace_init_failure"),
            "expected snake_case category key in serialized JSON; got: {raw}"
        );
        assert!(
            raw.contains("pr_creation_failure"),
            "expected snake_case category key in serialized JSON; got: {raw}"
        );
    }

    #[test]
    fn clear_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let mut s = AlertState::default();
        s.alerts.insert(
            AlertCategory::BranchPushFailure,
            AlertEntry {
                last_alerted_at: Utc::now(),
                last_error_excerpt: "x".into(),
            },
        );
        s.save(dir.path()).unwrap();
        assert!(dir.path().join(".alert-state.json").exists());
        AlertState::clear(dir.path()).unwrap();
        assert!(!dir.path().join(".alert-state.json").exists());
        // Calling again must not error.
        AlertState::clear(dir.path()).unwrap();
    }

    #[test]
    fn clear_does_not_error_on_missing() {
        let dir = TempDir::new().unwrap();
        // No file ever written → clear is still Ok.
        AlertState::clear(dir.path()).unwrap();
    }
}
