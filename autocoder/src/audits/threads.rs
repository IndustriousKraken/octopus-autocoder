//! Audit-thread state IO for the `audit-reply-acts` flow.
//!
//! When an audit's findings post via the threaded chatops path
//! (`chatops-audit-findings-in-threads`), the scheduler captures the
//! resulting `thread_ts` and stamps an `AuditThreadState` to the
//! audit-threads directory. The chatops listener consults this state
//! when an operator posts `@<bot> send it` to decide whether the request
//! is valid, stale, or already-acted.
//!
//! State files are JSON, atomically written via tempfile-then-rename so a
//! torn write is never visible to a concurrent reader.

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Cap on the `findings_excerpt` field. Mirrors
/// `AUDIT_THREAD_BODY_CHAR_CAP` (35,000) from the threaded-notification
/// path so the triage prompt can ship the full content the operator saw.
pub const FINDINGS_EXCERPT_CAP: usize = 35_000;

// The excerpt cap MUST equal the thread-body cap so the stamped excerpt and the
// posted thread body truncate at the same boundary (the
// `audit-triage-carries-full-findings` requirement). Enforced at compile time.
const _: () = assert!(FINDINGS_EXCERPT_CAP == crate::audits::AUDIT_THREAD_BODY_CHAR_CAP);

/// One audit-notification thread's tracked state. Written by the
/// scheduler when a threaded audit notification posts; consulted by the
/// chatops dispatcher when `@<bot> send it` arrives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditThreadState {
    pub thread_ts: String,
    pub channel: String,
    pub repo_url: String,
    pub audit_type: String,
    pub findings_excerpt: String,
    pub posted_at: DateTime<Utc>,
    pub status: AuditThreadStatus,
    /// Populated for `TriageFailed` so operators see why the prior
    /// attempt failed when they retry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Lifecycle states for an audit-thread entry. Transitions:
///   - Initial: `Open` (written by the scheduler).
///   - `Open` → `TriagePending` when `send it` is accepted.
///   - `TriagePending` → `Acted` when triage completes (with or without
///     PRs).
///   - `TriagePending` → `TriageFailed` when triage errors out.
///   - `TriageFailed` → `TriagePending` when the operator retries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditThreadStatus {
    Open,
    TriagePending,
    Acted,
    TriageFailed,
}

impl AuditThreadStatus {
    /// Human-readable label for chatops replies and log lines.
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::TriagePending => "triage-pending",
            Self::Acted => "acted",
            Self::TriageFailed => "triage-failed",
        }
    }
}

/// Canonical state file path: `<state_dir>/audit-threads/<thread_ts>.json`.
pub fn state_path(state_dir: &Path, thread_ts: &str) -> PathBuf {
    state_dir.join("audit-threads").join(format!("{thread_ts}.json"))
}

/// Directory holding every audit-thread state file. Created on first
/// `write_state` call; tests probe this directly.
pub fn state_dir(root: &Path) -> PathBuf {
    root.join("audit-threads")
}

/// Default state directory: the daemon's resolved `state_dir`. The
/// audit-threads files survive reboot alongside audit cadence, failure
/// counters, and revision state — they belong to the same persistent
/// data category.
///
/// The `DaemonPaths` reference is threaded explicitly per the canonical
/// `Production paths SHALL be threaded` requirement (function-parameter
/// pattern).
pub fn default_state_root(paths: &crate::paths::DaemonPaths) -> PathBuf {
    paths.state.clone()
}

/// Atomically write `state` to its canonical file. Parent directory is
/// created if absent.
pub fn write_state(state_dir_root: &Path, state: &AuditThreadState) -> Result<()> {
    let dir = state_dir(state_dir_root);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating audit-threads dir {}", dir.display()))?;
    let path = state_path(state_dir_root, &state.thread_ts);
    let tmp = tempfile::NamedTempFile::new_in(&dir)
        .with_context(|| format!("creating tempfile in {}", dir.display()))?;
    serde_json::to_writer_pretty(&tmp, state)
        .with_context(|| format!("serializing audit-thread state for {}", path.display()))?;
    tmp.persist(&path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

/// Read the audit-thread state for `thread_ts`. Returns `Ok(None)` when
/// no file exists; surfaces an error on read or parse failure.
pub fn read_state(state_dir_root: &Path, thread_ts: &str) -> Result<Option<AuditThreadState>> {
    let path = state_path(state_dir_root, thread_ts);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow!("reading {}: {e}", path.display())),
    };
    serde_json::from_str::<AuditThreadState>(&raw)
        .map(Some)
        .with_context(|| format!("parsing {}", path.display()))
}

/// Remove every state file whose `posted_at` is older than `max_age`.
/// Returns the number of entries removed. Unparseable files and stat
/// failures are logged and skipped; the function never propagates such
/// errors so a single bad file cannot stall the prune.
pub fn prune_stale_entries(state_dir_root: &Path, max_age: Duration) -> Result<usize> {
    let dir = state_dir(state_dir_root);
    if !dir.is_dir() {
        return Ok(0);
    }
    let now = Utc::now();
    let mut removed = 0usize;
    for entry in std::fs::read_dir(&dir)
        .with_context(|| format!("reading audit-threads dir {}", dir.display()))?
    {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                // no-url: daemon-global thread-state prune, not scoped to one repo
                tracing::warn!("audit-threads prune: read_dir entry error: {e}");
                continue;
            }
        };
        let path = entry.path();
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                // no-url: daemon-global thread-state prune, not scoped to one repo
                tracing::warn!(
                    path = %path.display(),
                    "audit-threads prune: skipping unreadable file: {e}"
                );
                continue;
            }
        };
        let state: AuditThreadState = match serde_json::from_str(&raw) {
            Ok(s) => s,
            Err(e) => {
                // no-url: daemon-global thread-state prune, not scoped to one repo
                tracing::warn!(
                    path = %path.display(),
                    "audit-threads prune: skipping unparseable file: {e}"
                );
                continue;
            }
        };
        if now - state.posted_at > max_age {
            match std::fs::remove_file(&path) {
                Ok(()) => removed += 1,
                // no-url: daemon-global thread-state prune, not scoped to one repo
                Err(e) => tracing::warn!(
                    path = %path.display(),
                    "audit-threads prune: remove failed: {e}"
                ),
            }
        }
    }
    Ok(removed)
}

/// Truncate `findings` to the audit-thread excerpt cap so the stored state
/// file remains bounded, appending the same pointer-to-daemon-log tail (naming
/// `audit_id`) the posted thread body uses when truncation occurs — so the
/// stamped excerpt and the thread body cap identically (the
/// `audit-triage-carries-full-findings` requirement). Delegates to the shared
/// [`crate::audits::cap_audit_findings_body`] so the cap value AND the tail
/// text are a single source. A body within the cap is returned verbatim.
///
/// The delegate is idempotent: the usual input here is the posted thread body,
/// which the formatter already capped, so an already-capped body is returned
/// unchanged — the excerpt is byte-identical to the thread body and never gains
/// a nested second tail. Callers MUST still funnel the full findings body
/// through this before constructing an `AuditThreadState`.
pub fn cap_findings_excerpt(findings: &str, audit_id: &str) -> String {
    crate::audits::cap_audit_findings_body(findings, audit_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture_state(thread_ts: &str) -> AuditThreadState {
        AuditThreadState {
            thread_ts: thread_ts.to_string(),
            channel: "C_OPS".to_string(),
            repo_url: "git@github.com:owner/repo.git".to_string(),
            audit_type: "architecture_advisor".to_string(),
            findings_excerpt: "  • file foo.rs is 1234 lines".to_string(),
            posted_at: Utc::now(),
            status: AuditThreadStatus::Open,
            reason: None,
        }
    }

    #[test]
    fn read_missing_state_file_returns_ok_none() {
        let tmp = TempDir::new().unwrap();
        let got = read_state(tmp.path(), "1748293445.001234").unwrap();
        assert!(got.is_none(), "missing file must surface as None");
    }

    #[test]
    fn write_then_read_round_trips_every_field() {
        let tmp = TempDir::new().unwrap();
        let mut state = fixture_state("1748.999");
        state.status = AuditThreadStatus::TriageFailed;
        state.reason = Some("executor returned Failed: timeout".into());
        write_state(tmp.path(), &state).unwrap();
        let got = read_state(tmp.path(), "1748.999").unwrap().unwrap();
        assert_eq!(got, state);
        assert_eq!(got.status, AuditThreadStatus::TriageFailed);
        assert_eq!(got.reason.as_deref(), Some("executor returned Failed: timeout"));
    }

    #[test]
    fn status_transition_preserves_other_fields() {
        let tmp = TempDir::new().unwrap();
        let initial = fixture_state("1748.t1");
        write_state(tmp.path(), &initial).unwrap();

        let mut updated = initial.clone();
        updated.status = AuditThreadStatus::TriagePending;
        write_state(tmp.path(), &updated).unwrap();
        let got = read_state(tmp.path(), "1748.t1").unwrap().unwrap();
        assert_eq!(got.status, AuditThreadStatus::TriagePending);
        assert_eq!(got.findings_excerpt, initial.findings_excerpt);
        assert_eq!(got.channel, initial.channel);
        assert_eq!(got.repo_url, initial.repo_url);
        assert_eq!(got.audit_type, initial.audit_type);
    }

    #[test]
    fn prune_removes_old_entries_and_keeps_fresh() {
        let tmp = TempDir::new().unwrap();
        let mut old = fixture_state("1700.old");
        old.posted_at = Utc::now() - Duration::days(8);
        write_state(tmp.path(), &old).unwrap();
        let young = fixture_state("1700.young");
        write_state(tmp.path(), &young).unwrap();

        let removed = prune_stale_entries(tmp.path(), Duration::days(7)).unwrap();
        assert_eq!(removed, 1);
        assert!(read_state(tmp.path(), "1700.old").unwrap().is_none());
        assert!(read_state(tmp.path(), "1700.young").unwrap().is_some());
    }

    #[test]
    fn prune_on_empty_or_missing_dir_returns_zero() {
        let tmp = TempDir::new().unwrap();
        // No audit-threads/ subdirectory yet.
        assert_eq!(prune_stale_entries(tmp.path(), Duration::days(7)).unwrap(), 0);
        // Create the dir but leave it empty.
        std::fs::create_dir_all(state_dir(tmp.path())).unwrap();
        assert_eq!(prune_stale_entries(tmp.path(), Duration::days(7)).unwrap(), 0);
    }

    #[test]
    fn cap_findings_excerpt_truncates_at_cap_with_pointer() {
        let s: String = std::iter::repeat_n('x', FINDINGS_EXCERPT_CAP + 100).collect();
        let audit_id = "owner_repo:drift_audit:2026-06-25T00:00:00Z";
        let capped = cap_findings_excerpt(&s, audit_id);
        // Truncated to the cap AND carrying the pointer-to-daemon-log tail, so
        // the result runs past the bare cap by exactly the tail length.
        assert!(capped.chars().count() > FINDINGS_EXCERPT_CAP);
        assert!(capped.starts_with(&"x".repeat(FINDINGS_EXCERPT_CAP)));
        assert!(
            capped.contains(&format!(
                "[truncated; full findings at journalctl -u autocoder | grep audit_id={audit_id}]"
            )),
            "capped excerpt must carry the documented pointer tail: {capped}"
        );
        // Short strings pass through verbatim (no pointer appended).
        assert_eq!(cap_findings_excerpt("hello", "id"), "hello");
    }

    #[test]
    fn status_label_round_trips() {
        assert_eq!(AuditThreadStatus::Open.label(), "open");
        assert_eq!(AuditThreadStatus::TriagePending.label(), "triage-pending");
        assert_eq!(AuditThreadStatus::Acted.label(), "acted");
        assert_eq!(AuditThreadStatus::TriageFailed.label(), "triage-failed");
    }

    #[test]
    fn state_path_lives_under_audit_threads_subdir() {
        let p = state_path(Path::new("/var/lib/autocoder"), "1700.abc");
        let s = p.to_string_lossy();
        assert!(s.contains("audit-threads"), "{s}");
        assert!(s.ends_with("1700.abc.json"), "{s}");
    }
}
