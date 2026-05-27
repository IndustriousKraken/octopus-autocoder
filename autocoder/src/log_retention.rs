//! Per-change run-log retention pass.
//!
//! The daemon writes one log file per (workspace, change) tuple at
//! `<logs>/runs/<workspace-basename>/<change>.log`. JSON-streaming
//! mode produces ~100x more bytes per run than legacy text mode, so
//! without a retention policy the log directory grows unbounded over
//! months of operation.
//!
//! Retention rules:
//!   - A log file is **eligible** for deletion when its mtime is older
//!     than `now - days * 86400` seconds.
//!   - Eligible logs are **preserved** when their corresponding change
//!     directory at `<workspaces_root>/<workspace>/openspec/changes/<change>/`
//!     still exists. Operators investigating long-running stuck
//!     changes want their logs even if old.
//!   - Eligible logs whose change directory has been archived OR
//!     deleted are **removed**.
//!
//! The pass is run at daemon startup AND every 24 hours via a
//! periodic tokio task. A `PruneReport` is logged after each pass.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Configuration for one retention pass.
#[derive(Debug, Clone, Copy)]
pub struct RetentionConfig {
    pub days: u32,
}

/// One-pass tally returned by `prune_stale_logs`.
#[derive(Debug, Default, Clone)]
pub struct PruneReport {
    pub files_deleted: u32,
    pub bytes_freed: u64,
    pub files_preserved: u32,
}

/// Walk `<logs_root>/runs/<workspace>/<change>.log` and delete files
/// whose mtime is older than the retention window AND whose change
/// directory at `<workspaces_root>/<workspace>/openspec/changes/<change>/`
/// no longer exists. Returns a `PruneReport` describing the outcome.
///
/// `logs_root` is typically `<daemon_paths>.logs`; `workspaces_root` is
/// typically `<daemon_paths>.cache.join("workspaces")`. Per-file
/// failures are logged at WARN but never abort the walk — a permission
/// error on one file should not stall the rest of the pass.
pub fn prune_stale_logs(
    logs_root: &Path,
    workspaces_root: &Path,
    config: &RetentionConfig,
) -> Result<PruneReport> {
    let runs_root = logs_root.join("runs");
    if !runs_root.exists() {
        return Ok(PruneReport::default());
    }
    let now = SystemTime::now();
    let window = Duration::from_secs(u64::from(config.days) * 86_400);
    let mut report = PruneReport::default();

    let workspace_dirs = std::fs::read_dir(&runs_root)
        .with_context(|| format!("listing log runs directory {}", runs_root.display()))?;
    for ws_entry in workspace_dirs.flatten() {
        let ws_path = ws_entry.path();
        if !ws_path.is_dir() {
            continue;
        }
        let workspace_basename = match ws_path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let workspace_changes_dir = workspaces_root
            .join(&workspace_basename)
            .join("openspec")
            .join("changes");
        let log_files = match std::fs::read_dir(&ws_path) {
            Ok(it) => it,
            Err(e) => {
                tracing::warn!(
                    path = %ws_path.display(),
                    "log-retention: skipping workspace directory: {e}"
                );
                continue;
            }
        };
        for log_entry in log_files.flatten() {
            let log_path = log_entry.path();
            if log_path.extension().and_then(|s| s.to_str()) != Some("log") {
                continue;
            }
            // Resolve the change name from the .log filename.
            let change_name = match log_path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
            {
                Some(s) => s,
                None => continue,
            };
            let metadata = match log_entry.metadata() {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(
                        path = %log_path.display(),
                        "log-retention: cannot stat: {e}"
                    );
                    continue;
                }
            };
            let mtime = match metadata.modified() {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(
                        path = %log_path.display(),
                        "log-retention: cannot read mtime: {e}"
                    );
                    continue;
                }
            };
            let age = now.duration_since(mtime).unwrap_or(Duration::ZERO);
            if age < window {
                continue;
            }
            // Eligible by age. Check active-change preservation.
            let change_dir = workspace_changes_dir.join(&change_name);
            if change_dir.exists() {
                report.files_preserved += 1;
                continue;
            }
            // Delete.
            let size = metadata.len();
            match std::fs::remove_file(&log_path) {
                Ok(()) => {
                    report.files_deleted += 1;
                    report.bytes_freed += size;
                    tracing::debug!(
                        path = %log_path.display(),
                        size_bytes = size,
                        "log-retention: deleted stale log"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        path = %log_path.display(),
                        "log-retention: failed to delete: {e}"
                    );
                }
            }
        }
    }
    Ok(report)
}

/// Spawn the periodic retention task. Runs immediately at startup,
/// then every 24 hours until cancelled. Logs each pass's report.
pub fn spawn_periodic(
    logs_root: PathBuf,
    workspaces_root: PathBuf,
    config: RetentionConfig,
    cancel: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match prune_stale_logs(&logs_root, &workspaces_root, &config) {
                Ok(report) => {
                    if report.files_deleted > 0
                        || report.files_preserved > 0
                    {
                        tracing::info!(
                            files_deleted = report.files_deleted,
                            bytes_freed = report.bytes_freed,
                            files_preserved = report.files_preserved,
                            "log-retention: pass complete"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("log-retention: pass failed (daemon continues): {e:#}");
                }
            }
            let sleeper = tokio::time::sleep(Duration::from_secs(86_400));
            tokio::select! {
                () = sleeper => {}
                () = cancel.cancelled() => return,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_fixture() -> (TempDir, PathBuf, PathBuf) {
        let dir = TempDir::new().unwrap();
        let logs = dir.path().join("logs");
        let workspaces = dir.path().join("workspaces");
        fs::create_dir_all(&logs).unwrap();
        fs::create_dir_all(&workspaces).unwrap();
        (dir, logs, workspaces)
    }

    fn write_log_with_mtime(path: &Path, content: &str, age_seconds: u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
        let mtime = SystemTime::now()
            .checked_sub(Duration::from_secs(age_seconds))
            .unwrap_or_else(SystemTime::now);
        // filetime crate isn't in deps; use the `utimes` syscall via libc.
        let ftime = filetime_from(mtime);
        set_mtime(path, ftime);
    }

    fn filetime_from(t: SystemTime) -> libc::timespec {
        let dur = t
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        libc::timespec {
            tv_sec: dur.as_secs() as libc::time_t,
            tv_nsec: i64::from(dur.subsec_nanos()),
        }
    }

    fn set_mtime(path: &Path, ts: libc::timespec) {
        use std::ffi::CString;
        let c = CString::new(path.as_os_str().to_string_lossy().as_bytes()).unwrap();
        // Two timespec values: [0] = atime, [1] = mtime. Use the same
        // value for both so atime doesn't drift in a way that confuses
        // a future audit.
        let times: [libc::timespec; 2] = [ts, ts];
        unsafe {
            let r = libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), 0);
            assert!(r == 0, "utimensat failed: {}", std::io::Error::last_os_error());
        }
    }

    #[test]
    fn stale_log_for_archived_change_is_deleted() {
        let (_dir, logs, workspaces) = setup_fixture();
        let ws_basename = "fixture_repo";
        let change = "old-archived-change";
        // Active workspace exists but the change dir is GONE (archived).
        fs::create_dir_all(workspaces.join(ws_basename).join("openspec").join("changes"))
            .unwrap();
        // Log file: 60 days old.
        let log_path = logs.join("runs").join(ws_basename).join(format!("{change}.log"));
        write_log_with_mtime(&log_path, "old content", 60 * 86_400);

        let report = prune_stale_logs(&logs, &workspaces, &RetentionConfig { days: 30 }).unwrap();
        assert_eq!(report.files_deleted, 1, "report: {report:?}");
        assert_eq!(report.files_preserved, 0);
        assert!(report.bytes_freed > 0);
        assert!(!log_path.exists(), "log file must be deleted");
    }

    #[test]
    fn stale_log_for_active_change_is_preserved() {
        let (_dir, logs, workspaces) = setup_fixture();
        let ws_basename = "fixture_repo";
        let change = "still-active-change";
        // Change directory STILL exists under openspec/changes.
        fs::create_dir_all(
            workspaces
                .join(ws_basename)
                .join("openspec")
                .join("changes")
                .join(change),
        )
        .unwrap();
        let log_path = logs.join("runs").join(ws_basename).join(format!("{change}.log"));
        write_log_with_mtime(&log_path, "old content", 60 * 86_400);

        let report = prune_stale_logs(&logs, &workspaces, &RetentionConfig { days: 30 }).unwrap();
        assert_eq!(report.files_deleted, 0);
        assert_eq!(report.files_preserved, 1, "report: {report:?}");
        assert!(log_path.exists(), "active change's log must be preserved");
    }

    #[test]
    fn recent_log_is_preserved_regardless_of_change_state() {
        let (_dir, logs, workspaces) = setup_fixture();
        let ws_basename = "fixture_repo";
        let change = "recent-archived-change";
        // Workspace exists but change is gone — and yet the log is recent.
        fs::create_dir_all(workspaces.join(ws_basename).join("openspec").join("changes"))
            .unwrap();
        let log_path = logs.join("runs").join(ws_basename).join(format!("{change}.log"));
        write_log_with_mtime(&log_path, "fresh content", 10 * 86_400);

        let report = prune_stale_logs(&logs, &workspaces, &RetentionConfig { days: 30 }).unwrap();
        assert_eq!(report.files_deleted, 0);
        // The file is within the retention window so it does NOT count
        // as preserved by the active-change rule either.
        assert_eq!(report.files_preserved, 0);
        assert!(log_path.exists());
    }

    #[test]
    fn missing_logs_root_is_a_noop() {
        let (_dir, logs, workspaces) = setup_fixture();
        // logs/runs does NOT exist.
        let report = prune_stale_logs(&logs, &workspaces, &RetentionConfig { days: 30 }).unwrap();
        assert_eq!(report.files_deleted, 0);
        assert_eq!(report.files_preserved, 0);
    }

    #[test]
    fn report_aggregates_across_multiple_workspaces() {
        let (_dir, logs, workspaces) = setup_fixture();
        for ws in ["repo_a", "repo_b"] {
            fs::create_dir_all(workspaces.join(ws).join("openspec").join("changes")).unwrap();
            let log = logs.join("runs").join(ws).join("ch.log");
            write_log_with_mtime(&log, "old", 60 * 86_400);
        }
        let report = prune_stale_logs(&logs, &workspaces, &RetentionConfig { days: 30 }).unwrap();
        assert_eq!(report.files_deleted, 2);
    }
}
