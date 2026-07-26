//! Durable per-workspace iteration record (durable-iteration-record).
//!
//! Every polling iteration — success, idle, skipped, audit-only, or failed —
//! overwrites ONE file at `<state_dir>/iteration-record/<workspace-basename>.json`
//! at its end (atomic tempfile-then-rename). The status surfaces source their
//! last-iteration data EXCLUSIVELY from this record; the per-change
//! failure-counter store is no longer a proxy for iteration recency.
//!
//! No history is kept: the status block only ever shows the latest iteration,
//! and the unified daemon log already carries the full history. The record
//! lives under `<state_dir>/` (never the workspace) because it is daemon
//! bookkeeping that must not appear in the managed repo's working tree.
//!
//! Because idle iterations stamp the record too, a `finished_at` older than the
//! poll interval (plus in-flight time) is a TRUE signal that the polling task
//! has not completed an iteration since then — daemon liveness becomes
//! diagnosable from chatops.

use crate::paths::DaemonPaths;
use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The distinguishing kind of an iteration's terminal outcome. `Failed` is set
/// only by the driver when the pass returned `Err`; the `Ok` paths carry one of
/// the other four via [`IterationOutcome`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    /// Archived changes and/or processed issue units (real shipped work).
    SuccessWithWork,
    /// Empty queue, nothing to do.
    Idle,
    /// The pass short-circuited on a park (open-PR gate, waiting change,
    /// push-block resume, busy marker).
    Skipped,
    /// Commits produced by the audit phase only (no implementer change/issue).
    AuditOnly,
    /// The pass returned `Err`.
    Failed,
}

/// The persisted record. `outcome_summary` is a one-line human description
/// rendered verbatim on the status reply's `outcome:` line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IterationRecord {
    pub finished_at: DateTime<Utc>,
    pub outcome_kind: OutcomeKind,
    pub outcome_summary: String,
    pub duration_secs: u64,
}

/// In-memory result of one polling pass, returned by `execute_one_pass` and
/// mapped into the persisted record by the iteration driver. The driver maps a
/// pass `Err` to [`OutcomeKind::Failed`] separately, so there is no failed
/// variant here.
#[derive(Debug, Clone)]
pub enum IterationOutcome {
    /// Archived the named changes and/or processed the named issue units.
    SuccessWithWork {
        changes: Vec<String>,
        issues: Vec<String>,
    },
    /// Empty queue — the pass produced no commits.
    Idle,
    /// Skipped its normal queue walk on a park; `park` names which one.
    Skipped { park: String },
    /// Commits produced by the audit phase only (no implementer change/issue).
    AuditOnly,
}

impl IterationOutcome {
    fn kind(&self) -> OutcomeKind {
        match self {
            IterationOutcome::SuccessWithWork { .. } => OutcomeKind::SuccessWithWork,
            IterationOutcome::Idle => OutcomeKind::Idle,
            IterationOutcome::Skipped { .. } => OutcomeKind::Skipped,
            IterationOutcome::AuditOnly => OutcomeKind::AuditOnly,
        }
    }

    fn summary(&self) -> String {
        match self {
            IterationOutcome::SuccessWithWork { changes, issues } => {
                let mut parts = Vec::new();
                if !changes.is_empty() {
                    parts.push(format!("archived {}", changes.join(", ")));
                }
                if !issues.is_empty() {
                    parts.push(format!("issues {}", issues.join(", ")));
                }
                if parts.is_empty() {
                    // Defensive — success-with-work implies at least one unit.
                    "shipped work".to_string()
                } else {
                    truncate(&parts.join("; "), 200)
                }
            }
            IterationOutcome::Idle => "empty queue — nothing to do".to_string(),
            IterationOutcome::Skipped { park } => truncate(park, 200),
            IterationOutcome::AuditOnly => "audit-only (no queued changes)".to_string(),
        }
    }
}

/// Build the record to persist from an iteration's `Result`. A pass `Err`
/// becomes a `Failed` outcome carrying a truncated reason.
pub fn record_for(
    result: &Result<IterationOutcome>,
    finished_at: DateTime<Utc>,
    duration_secs: u64,
) -> IterationRecord {
    match result {
        Ok(outcome) => IterationRecord {
            finished_at,
            outcome_kind: outcome.kind(),
            outcome_summary: outcome.summary(),
            duration_secs,
        },
        Err(e) => IterationRecord {
            finished_at,
            outcome_kind: OutcomeKind::Failed,
            outcome_summary: truncate(&format!("{e:#}"), 200),
            duration_secs,
        },
    }
}

/// Derive the record file's workspace basename (final path component), matching
/// the convention used by the failure-state / alert-state stores.
fn basename(workspace: &Path) -> String {
    workspace
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Overwrite the iteration record for `workspace` via atomic
/// tempfile-then-rename in the record directory (so a torn write is never
/// observed by a concurrent reader). Best-effort at the call site: the driver
/// logs a write failure at WARN and does NOT alter the iteration's outcome.
pub fn write(paths: &DaemonPaths, workspace: &Path, record: &IterationRecord) -> Result<()> {
    let path = paths.iteration_record_path(&basename(workspace));
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("destination path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating parent dir {}", parent.display()))?;
    let tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating tempfile in {}", parent.display()))?;
    serde_json::to_writer_pretty(&tmp, record)
        .with_context(|| format!("serializing iteration record for {}", path.display()))?;
    tmp.persist(&path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

/// Read the latest iteration record for `workspace`. Best-effort: a missing
/// file (fresh install, or a daemon that has never completed an iteration for
/// this repo) is `None`; an unreadable or corrupt file logs a WARN and degrades
/// to `None` so a bad file never breaks the status reply.
pub fn read(paths: &DaemonPaths, workspace: &Path) -> Option<IterationRecord> {
    let path = paths.iteration_record_path(&basename(workspace));
    match std::fs::read_to_string(&path) {
        Ok(raw) => match serde_json::from_str::<IterationRecord>(&raw) {
            Ok(rec) => Some(rec),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    "iteration-record file is corrupt; treating as no record: {e:#}"
                );
                None
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                "iteration-record file unreadable; treating as no record: {e:#}"
            );
            None
        }
    }
}

/// Truncate `s` to at most `n` chars, appending an ellipsis when clipped.
fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::test_daemon_paths;

    #[test]
    fn read_missing_returns_none() {
        let (_temp, paths) = test_daemon_paths();
        let workspace = paths.cache.join("workspaces").join("ws");
        assert!(read(&paths, &workspace).is_none());
    }

    #[test]
    fn write_then_read_roundtrips_and_overwrites() {
        let (_temp, paths) = test_daemon_paths();
        let workspace = paths.cache.join("workspaces").join("ws");
        let first = record_for(&Ok(IterationOutcome::Idle), Utc::now(), 3);
        write(&paths, &workspace, &first).unwrap();
        let got = read(&paths, &workspace).expect("record present");
        assert_eq!(got.outcome_kind, OutcomeKind::Idle);
        assert_eq!(got.duration_secs, 3);

        // A second write for the SAME workspace overwrites (no history kept).
        let later = Utc::now() + chrono::Duration::seconds(10);
        let second = record_for(
            &Ok(IterationOutcome::SuccessWithWork {
                changes: vec!["a05-foo".into()],
                issues: vec![],
            }),
            later,
            7,
        );
        write(&paths, &workspace, &second).unwrap();
        let got = read(&paths, &workspace).expect("record present");
        assert_eq!(got.outcome_kind, OutcomeKind::SuccessWithWork);
        assert_eq!(got.finished_at, later);
        assert!(got.outcome_summary.contains("a05-foo"));
    }

    #[test]
    fn outcome_kinds_and_summaries_map_as_expected() {
        let now = Utc::now();
        let idle = record_for(&Ok(IterationOutcome::Idle), now, 0);
        assert_eq!(idle.outcome_kind, OutcomeKind::Idle);

        let work = record_for(
            &Ok(IterationOutcome::SuccessWithWork {
                changes: vec!["a05-foo".into(), "a06-bar".into()],
                issues: vec!["fix-x".into()],
            }),
            now,
            0,
        );
        assert_eq!(work.outcome_kind, OutcomeKind::SuccessWithWork);
        assert!(work.outcome_summary.contains("a05-foo"));
        assert!(work.outcome_summary.contains("a06-bar"));
        assert!(work.outcome_summary.contains("fix-x"));

        let skipped = record_for(
            &Ok(IterationOutcome::Skipped {
                park: "open agent-branch PR — skip-iteration gate active".into(),
            }),
            now,
            0,
        );
        assert_eq!(skipped.outcome_kind, OutcomeKind::Skipped);
        assert!(skipped.outcome_summary.contains("open agent-branch PR"));

        let audit = record_for(&Ok(IterationOutcome::AuditOnly), now, 0);
        assert_eq!(audit.outcome_kind, OutcomeKind::AuditOnly);

        let failed: Result<IterationOutcome> = Err(anyhow!("boom: remote push rejected"));
        let rec = record_for(&failed, now, 0);
        assert_eq!(rec.outcome_kind, OutcomeKind::Failed);
        assert!(rec.outcome_summary.contains("boom"));
    }

    #[test]
    fn failed_reason_is_truncated() {
        let long = "x".repeat(500);
        let failed: Result<IterationOutcome> = Err(anyhow!("{long}"));
        let rec = record_for(&failed, Utc::now(), 0);
        assert!(rec.outcome_summary.chars().count() <= 201, "truncated to ~200");
        assert!(rec.outcome_summary.ends_with('…'));
    }
}
