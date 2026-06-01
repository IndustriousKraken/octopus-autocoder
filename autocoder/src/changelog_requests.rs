//! Changelog-request state IO for the chat-driven changelog flow.
//!
//! When an operator posts `@<bot> changelog <repo> [<args>]`, the chatops
//! dispatcher writes a `ChangelogRequestState` to disk and pushes a
//! `ChangelogRequest` onto the matching `RepoTaskHandle::pending_changelog_requests`
//! queue. The polling loop drains the queue at iteration start and
//! transitions the state file through `Pending → InFlight → (Acted |
//! Failed)`.
//!
//! State files are JSON, atomically written via tempfile-then-rename so a
//! torn write is never visible to a concurrent reader. They live at
//! `<state_dir>/changelog-requests/<repo-sanitized>/<request-id>.json`.

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One changelog-request's tracked state. Written by the chatops
/// dispatcher when `@<bot> changelog ...` is accepted; the polling loop
/// reads, transitions, and (on terminal status) leaves on disk so the
/// 7-day prune can sweep it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangelogRequestState {
    pub request_id: String,
    pub repo_url: String,
    pub raw_args: String,
    pub channel: String,
    /// Lifecycle-thread anchor — the bot's top-level ack `ts` so all
    /// subsequent status updates land in the same thread.
    pub lifecycle_thread_ts: String,
    pub status: ChangelogStatus,
    pub submitted_at: DateTime<Utc>,
    /// Populated for `Failed` so operators see why the prior attempt
    /// failed; also useful for any non-success terminal state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Lifecycle states for a changelog-request entry. Transitions:
///   - Initial: `Pending` (written by the chatops dispatcher).
///   - `Pending` → `InFlight` when the polling loop picks it up.
///   - `InFlight` → `Acted` on a successful PR open.
///   - `InFlight` → `Failed` when the executor or git plumbing errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangelogStatus {
    Pending,
    InFlight,
    Acted,
    Failed,
}

impl ChangelogStatus {
    #[allow(dead_code)] // human-readable label for future chatops reply formatters
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InFlight => "in-flight",
            Self::Acted => "acted",
            Self::Failed => "failed",
        }
    }
}

/// Per-repo subdirectory under `changelog-requests/`. Uses the same
/// URL-sanitization rule as workspace paths so an operator can correlate
/// changelog-request files with the workspace on disk.
fn repo_subdir(repo_url: &str) -> String {
    crate::workspace::sanitize_url(repo_url)
}

/// Canonical state file path:
/// `<state_dir>/changelog-requests/<repo-sanitized>/<request-id>.json`.
pub fn state_path(state_dir_root: &Path, repo_url: &str, request_id: &str) -> PathBuf {
    state_dir_root
        .join("changelog-requests")
        .join(repo_subdir(repo_url))
        .join(format!("{request_id}.json"))
}

/// Top-level directory holding every changelog-request state file.
pub fn state_dir(state_dir_root: &Path) -> PathBuf {
    state_dir_root.join("changelog-requests")
}

/// Default state directory: the daemon's resolved `state_dir`. Mirrors
/// `proposal_requests`' default-root convention so both flows persist
/// into the same `state_dir` tree.
pub fn default_state_root(paths: &crate::paths::DaemonPaths) -> PathBuf {
    paths.state.clone()
}

/// Atomically write `state` to its canonical file. Parent directories
/// are created if absent. Sets file mode to 0640 (owner-readable +
/// writable, group-readable) parallel to other state files.
pub fn write_state(state_dir_root: &Path, state: &ChangelogRequestState) -> Result<()> {
    let dir = state_dir(state_dir_root).join(repo_subdir(&state.repo_url));
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating changelog-requests dir {}", dir.display()))?;
    let path = state_path(state_dir_root, &state.repo_url, &state.request_id);
    let tmp = tempfile::NamedTempFile::new_in(&dir)
        .with_context(|| format!("creating tempfile in {}", dir.display()))?;
    serde_json::to_writer_pretty(&tmp, state)
        .with_context(|| format!("serializing changelog-request state for {}", path.display()))?;
    tmp.persist(&path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)) {
            tracing::warn!(
                path = %path.display(),
                "changelog-request: setting 0640 mode failed: {e}"
            );
        }
    }
    Ok(())
}

/// Read the changelog-request state for `(repo_url, request_id)`. Returns
/// `Ok(None)` when no file exists; surfaces an error on read/parse.
pub fn read_state(
    state_dir_root: &Path,
    repo_url: &str,
    request_id: &str,
) -> Result<Option<ChangelogRequestState>> {
    let path = state_path(state_dir_root, repo_url, request_id);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow!("reading {}: {e}", path.display())),
    };
    serde_json::from_str::<ChangelogRequestState>(&raw)
        .map(Some)
        .with_context(|| format!("parsing {}", path.display()))
}

/// Remove the state file at `(repo_url, request_id)`. Missing file is
/// a no-op; other errors propagate.
#[allow(dead_code)]
pub fn remove_state(
    state_dir_root: &Path,
    repo_url: &str,
    request_id: &str,
) -> Result<()> {
    let path = state_path(state_dir_root, repo_url, request_id);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow!("removing {}: {e}", path.display())),
    }
}

/// Remove every state file whose `submitted_at` is older than `max_age`,
/// regardless of `status`. Returns the number of entries removed.
/// Pruned files emit one INFO log line per request_id.
pub fn prune_stale_entries(state_dir_root: &Path, max_age: Duration) -> Result<usize> {
    let dir = state_dir(state_dir_root);
    if !dir.is_dir() {
        return Ok(0);
    }
    let now = Utc::now();
    let mut removed = 0usize;
    let outer = std::fs::read_dir(&dir)
        .with_context(|| format!("reading changelog-requests dir {}", dir.display()))?;
    for repo_entry in outer {
        let repo_entry = match repo_entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("changelog-requests prune: outer read_dir entry error: {e}");
                continue;
            }
        };
        let repo_path = repo_entry.path();
        if !repo_path.is_dir() {
            continue;
        }
        let inner = match std::fs::read_dir(&repo_path) {
            Ok(rd) => rd,
            Err(e) => {
                tracing::warn!(
                    path = %repo_path.display(),
                    "changelog-requests prune: inner read_dir error: {e}"
                );
                continue;
            }
        };
        for entry in inner {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!("changelog-requests prune: inner entry error: {e}");
                    continue;
                }
            };
            let path = entry.path();
            let raw = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        "changelog-requests prune: skipping unreadable file: {e}"
                    );
                    continue;
                }
            };
            let state: ChangelogRequestState = match serde_json::from_str(&raw) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        "changelog-requests prune: skipping unparseable file: {e}"
                    );
                    continue;
                }
            };
            if now - state.submitted_at > max_age {
                match std::fs::remove_file(&path) {
                    Ok(()) => {
                        tracing::info!(
                            request_id = %state.request_id,
                            "changelog-requests prune: removed stale entry"
                        );
                        removed += 1;
                    }
                    Err(e) => tracing::warn!(
                        path = %path.display(),
                        "changelog-requests prune: remove failed: {e}"
                    ),
                }
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture_state(request_id: &str, repo_url: &str) -> ChangelogRequestState {
        ChangelogRequestState {
            request_id: request_id.to_string(),
            repo_url: repo_url.to_string(),
            raw_args: "--since v0.1.0".to_string(),
            channel: "C_OPS".to_string(),
            lifecycle_thread_ts: "1748399999.001234".to_string(),
            status: ChangelogStatus::Pending,
            submitted_at: Utc::now(),
            reason: None,
        }
    }

    #[test]
    fn read_missing_state_file_returns_ok_none() {
        let tmp = TempDir::new().unwrap();
        let got = read_state(
            tmp.path(),
            "git@github.com:owner/repo.git",
            "00000000-0000-0000-0000-000000000000",
        )
        .unwrap();
        assert!(got.is_none(), "missing file must surface as None");
    }

    #[test]
    fn write_then_read_round_trips_every_field() {
        let tmp = TempDir::new().unwrap();
        let mut state = fixture_state("req-1", "git@github.com:acme/myrepo.git");
        state.status = ChangelogStatus::Failed;
        state.reason = Some("executor timeout".into());
        write_state(tmp.path(), &state).unwrap();
        let got = read_state(tmp.path(), &state.repo_url, &state.request_id)
            .unwrap()
            .unwrap();
        assert_eq!(got, state);
        assert_eq!(got.status, ChangelogStatus::Failed);
        assert_eq!(got.reason.as_deref(), Some("executor timeout"));
    }

    #[test]
    fn status_transition_preserves_other_fields() {
        let tmp = TempDir::new().unwrap();
        let initial = fixture_state("req-2", "git@github.com:owner/repo.git");
        write_state(tmp.path(), &initial).unwrap();
        let mut updated = initial.clone();
        updated.status = ChangelogStatus::InFlight;
        write_state(tmp.path(), &updated).unwrap();
        let got = read_state(tmp.path(), &initial.repo_url, &initial.request_id)
            .unwrap()
            .unwrap();
        assert_eq!(got.status, ChangelogStatus::InFlight);
        assert_eq!(got.raw_args, initial.raw_args);
        assert_eq!(got.channel, initial.channel);
        assert_eq!(got.lifecycle_thread_ts, initial.lifecycle_thread_ts);
    }

    #[test]
    fn prune_removes_old_entries_and_keeps_fresh() {
        let tmp = TempDir::new().unwrap();
        let mut old = fixture_state("req-old", "git@github.com:owner/repo.git");
        old.submitted_at = Utc::now() - Duration::days(8);
        write_state(tmp.path(), &old).unwrap();
        let young = fixture_state("req-young", "git@github.com:owner/repo.git");
        // Force the young one to 6 days, well under the 7-day cap.
        let mut young_aged = young.clone();
        young_aged.submitted_at = Utc::now() - Duration::days(6);
        write_state(tmp.path(), &young_aged).unwrap();

        let removed = prune_stale_entries(tmp.path(), Duration::days(7)).unwrap();
        assert_eq!(removed, 1, "exactly the 8-day-old entry must be removed");
        assert!(
            read_state(tmp.path(), &old.repo_url, &old.request_id)
                .unwrap()
                .is_none(),
            "8-day-old entry must be gone"
        );
        assert!(
            read_state(tmp.path(), &young.repo_url, &young.request_id)
                .unwrap()
                .is_some(),
            "6-day-old entry must be preserved"
        );
    }

    #[test]
    fn prune_preserves_entries_regardless_of_status() {
        let tmp = TempDir::new().unwrap();
        let mut fresh_acted = fixture_state("req-acted", "git@github.com:owner/repo.git");
        fresh_acted.status = ChangelogStatus::Acted;
        write_state(tmp.path(), &fresh_acted).unwrap();
        let mut fresh_failed = fixture_state("req-failed", "git@github.com:owner/repo.git");
        fresh_failed.status = ChangelogStatus::Failed;
        write_state(tmp.path(), &fresh_failed).unwrap();
        let removed = prune_stale_entries(tmp.path(), Duration::days(7)).unwrap();
        assert_eq!(removed, 0);
    }

    #[test]
    fn prune_on_empty_or_missing_dir_returns_zero() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(prune_stale_entries(tmp.path(), Duration::days(7)).unwrap(), 0);
        std::fs::create_dir_all(state_dir(tmp.path())).unwrap();
        assert_eq!(prune_stale_entries(tmp.path(), Duration::days(7)).unwrap(), 0);
    }

    #[test]
    fn status_label_round_trips() {
        assert_eq!(ChangelogStatus::Pending.label(), "pending");
        assert_eq!(ChangelogStatus::InFlight.label(), "in-flight");
        assert_eq!(ChangelogStatus::Acted.label(), "acted");
        assert_eq!(ChangelogStatus::Failed.label(), "failed");
    }

    #[test]
    fn state_path_under_per_repo_subdir() {
        let p = state_path(
            Path::new("/var/lib/autocoder"),
            "git@github.com:owner/repo.git",
            "req-xyz",
        );
        let s = p.to_string_lossy();
        assert!(s.contains("changelog-requests"), "{s}");
        assert!(s.contains("owner_repo"), "per-repo subdir present: {s}");
        assert!(s.ends_with("req-xyz.json"), "{s}");
    }

    #[test]
    fn remove_state_missing_is_noop() {
        let tmp = TempDir::new().unwrap();
        remove_state(tmp.path(), "git@github.com:owner/repo.git", "nope").unwrap();
    }

    #[test]
    fn distinct_repos_get_distinct_subdirs() {
        let tmp = TempDir::new().unwrap();
        let s_a = fixture_state("req-shared", "git@github.com:owner/a.git");
        let s_b = fixture_state("req-shared", "git@github.com:owner/b.git");
        write_state(tmp.path(), &s_a).unwrap();
        write_state(tmp.path(), &s_b).unwrap();
        let got_a = read_state(tmp.path(), &s_a.repo_url, "req-shared")
            .unwrap()
            .unwrap();
        let got_b = read_state(tmp.path(), &s_b.repo_url, "req-shared")
            .unwrap()
            .unwrap();
        assert_eq!(got_a.repo_url, "git@github.com:owner/a.git");
        assert_eq!(got_b.repo_url, "git@github.com:owner/b.git");
    }
}
