//! Brownfield-request state IO for the `brownfield` chatops verb.
//!
//! When an operator posts `@<bot> brownfield <repo> <capability>
//! [guidance]`, the chatops dispatcher writes a
//! `BrownfieldRequestState` to disk and submits a control-socket action
//! that pushes a `BrownfieldRequest` onto the repo's
//! `pending_brownfield_requests` queue. The polling loop drains the
//! queue at iteration start, invokes the executor in brownfield-draft
//! mode, and transitions the state file through
//! `Pending → InProgress → (Acted | Failed | Aborted)`.
//!
//! State files are JSON, atomically written via tempfile-then-rename so a
//! torn write is never visible to a concurrent reader. They live at
//! `<workspace>/.state/brownfield_requests/<request-id>.json` — per the
//! a23 spec, brownfield state is per-workspace (not under the daemon's
//! central `state_dir`) so the file travels with the workspace.
//!
//! Cap on the operator's free-form guidance text. The spec disallows
//! guidance over 10,000 characters; the parser enforces the cap on the
//! inbound message before constructing a `BrownfieldRequestState`.

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Maximum length (in chars) of the operator-supplied guidance text. The
/// chatops parser enforces this cap before constructing a state.
/// Re-exported for callers that need to assert the cap without
/// importing the chatops module.
#[allow(dead_code)] // the parser-facing constant lives in `chatops::operator_commands`; this is a mirror for external callers
pub const GUIDANCE_CAP: usize = 10_000;

/// One brownfield-request's tracked state. Written by the chatops
/// dispatcher when `@<bot> brownfield ...` is accepted; the polling loop
/// reads, transitions, and leaves on disk after the terminal state so
/// operators have an observable record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrownfieldRequestState {
    pub request_id: String,
    pub repo_url: String,
    pub capability_name: String,
    /// Operator-supplied guidance text. Empty when no guidance was given;
    /// trimmed and capped at `GUIDANCE_CAP` chars by the parser.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
    pub channel: String,
    /// Bot's ack-message ts; the request's lifecycle thread.
    pub thread_ts: String,
    pub submitted_at: DateTime<Utc>,
    pub status: BrownfieldRequestStatus,
    /// Populated for `Failed` and `Aborted` so operators see why the
    /// request did not produce a PR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// URL of the spec PR opened when the request reaches `Acted`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
}

/// Lifecycle states for a brownfield-request entry. Transitions:
///   - Initial: `Pending` (written by the chatops dispatcher).
///   - `Pending` → `InProgress` when the polling loop picks it up.
///   - `InProgress` → `Acted` when the executor returns Completed AND
///     the iteration creates the spec PR.
///   - `InProgress` → `Failed` on any executor / verification failure.
///   - `Pending` or `InProgress` → `Aborted` when the
///     `openspec/specs/<capability>/spec.md` file appears between
///     dispatch and processing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrownfieldRequestStatus {
    Pending,
    InProgress,
    Acted,
    Failed,
    Aborted,
}

impl BrownfieldRequestStatus {
    /// Human-readable label for chatops replies and log lines.
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in-progress",
            Self::Acted => "acted",
            Self::Failed => "failed",
            Self::Aborted => "aborted",
        }
    }
}

/// Per-workspace state directory: `<workspace>/.state/brownfield_requests/`.
pub fn state_dir(workspace: &Path) -> PathBuf {
    workspace.join(".state").join("brownfield_requests")
}

/// Canonical state file path:
/// `<workspace>/.state/brownfield_requests/<request_id>.json`.
pub fn state_path(workspace: &Path, request_id: &str) -> PathBuf {
    state_dir(workspace).join(format!("{request_id}.json"))
}

/// Atomically write `state` to its canonical file under `workspace`.
/// Parent directories are created if absent.
pub fn write_state(workspace: &Path, state: &BrownfieldRequestState) -> Result<()> {
    let dir = state_dir(workspace);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating brownfield_requests dir {}", dir.display()))?;
    let path = state_path(workspace, &state.request_id);
    let tmp = tempfile::NamedTempFile::new_in(&dir)
        .with_context(|| format!("creating tempfile in {}", dir.display()))?;
    serde_json::to_writer_pretty(&tmp, state)
        .with_context(|| format!("serializing brownfield-request state for {}", path.display()))?;
    tmp.persist(&path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

/// Read the brownfield-request state for `request_id`. Returns
/// `Ok(None)` when no file exists; surfaces an error on read/parse.
pub fn read_state(
    workspace: &Path,
    request_id: &str,
) -> Result<Option<BrownfieldRequestState>> {
    let path = state_path(workspace, request_id);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow!("reading {}: {e}", path.display())),
    };
    serde_json::from_str::<BrownfieldRequestState>(&raw)
        .map(Some)
        .with_context(|| format!("parsing {}", path.display()))
}

/// Remove the state file for `request_id`. Missing file is a no-op.
#[allow(dead_code)]
pub fn remove_state(workspace: &Path, request_id: &str) -> Result<()> {
    let path = state_path(workspace, request_id);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow!("removing {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture_state(request_id: &str, capability: &str) -> BrownfieldRequestState {
        BrownfieldRequestState {
            request_id: request_id.to_string(),
            repo_url: "git@github.com:acme/myrepo.git".to_string(),
            capability_name: capability.to_string(),
            guidance: None,
            channel: "C_OPS".to_string(),
            thread_ts: "1748399999.001234".to_string(),
            submitted_at: Utc::now(),
            status: BrownfieldRequestStatus::Pending,
            reason: None,
            pr_url: None,
        }
    }

    #[test]
    fn read_missing_state_file_returns_ok_none() {
        let tmp = TempDir::new().unwrap();
        let got = read_state(tmp.path(), "00000000-0000-0000-0000-000000000000")
            .unwrap();
        assert!(got.is_none(), "missing file must surface as None");
    }

    #[test]
    fn write_then_read_round_trips_every_field() {
        let tmp = TempDir::new().unwrap();
        let mut state = fixture_state("req-1", "scheduler");
        state.guidance = Some("focus on the cron-trigger lifecycle".to_string());
        state.status = BrownfieldRequestStatus::Acted;
        state.pr_url = Some("https://github.com/acme/myrepo/pull/42".to_string());
        write_state(tmp.path(), &state).unwrap();
        let got = read_state(tmp.path(), &state.request_id).unwrap().unwrap();
        assert_eq!(got, state);
        assert_eq!(got.capability_name, "scheduler");
        assert_eq!(got.status, BrownfieldRequestStatus::Acted);
        assert_eq!(got.pr_url.as_deref(), Some("https://github.com/acme/myrepo/pull/42"));
    }

    #[test]
    fn status_transition_preserves_other_fields() {
        let tmp = TempDir::new().unwrap();
        let initial = fixture_state("req-2", "scheduler");
        write_state(tmp.path(), &initial).unwrap();
        let mut updated = initial.clone();
        updated.status = BrownfieldRequestStatus::Failed;
        updated.reason = Some("executor timeout".into());
        write_state(tmp.path(), &updated).unwrap();
        let got = read_state(tmp.path(), &initial.request_id).unwrap().unwrap();
        assert_eq!(got.status, BrownfieldRequestStatus::Failed);
        assert_eq!(got.reason.as_deref(), Some("executor timeout"));
        assert_eq!(got.capability_name, initial.capability_name);
        assert_eq!(got.channel, initial.channel);
        assert_eq!(got.thread_ts, initial.thread_ts);
    }

    #[test]
    fn state_path_is_under_workspace_subdir() {
        let p = state_path(Path::new("/tmp/ws"), "req-xyz");
        let s = p.to_string_lossy();
        assert!(s.starts_with("/tmp/ws/"), "{s}");
        assert!(s.contains(".state/brownfield_requests"), "{s}");
        assert!(s.ends_with("req-xyz.json"), "{s}");
    }

    #[test]
    fn remove_state_missing_is_noop() {
        let tmp = TempDir::new().unwrap();
        remove_state(tmp.path(), "nope").unwrap();
    }

    #[test]
    fn concurrent_writes_do_not_leak_tempfiles() {
        // Multiple sequential writes (simulating concurrent updates) must
        // leave no `.tmp` files in the directory.
        let tmp = TempDir::new().unwrap();
        for i in 0..5 {
            let mut state = fixture_state(&format!("req-{i}"), "cap");
            state.status = BrownfieldRequestStatus::InProgress;
            write_state(tmp.path(), &state).unwrap();
        }
        let dir = state_dir(tmp.path());
        let entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(entries.len(), 5);
        assert!(
            !entries.iter().any(|n| n.contains(".tmp")),
            "no .tmp files should leak: {entries:?}"
        );
    }

    #[test]
    fn enqueue_dequeue_via_vecdeque_preserves_order() {
        use std::collections::VecDeque;
        let mut q: VecDeque<String> = VecDeque::new();
        q.push_back("req-a".to_string());
        q.push_back("req-b".to_string());
        q.push_back("req-c".to_string());
        assert_eq!(q.pop_front().as_deref(), Some("req-a"));
        assert_eq!(q.pop_front().as_deref(), Some("req-b"));
        assert_eq!(q.pop_front().as_deref(), Some("req-c"));
        assert!(q.pop_front().is_none());
    }

    #[test]
    fn status_label_round_trips() {
        assert_eq!(BrownfieldRequestStatus::Pending.label(), "pending");
        assert_eq!(BrownfieldRequestStatus::InProgress.label(), "in-progress");
        assert_eq!(BrownfieldRequestStatus::Acted.label(), "acted");
        assert_eq!(BrownfieldRequestStatus::Failed.label(), "failed");
        assert_eq!(BrownfieldRequestStatus::Aborted.label(), "aborted");
    }
}
