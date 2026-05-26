//! Legacy `/tmp` → standard-layout migration.
//!
//! Pre-`state-paths-out-of-tmp`, autocoder wrote everything under
//! `/tmp/`: per-repo workspaces at `/tmp/workspaces/<sanitized>/`,
//! per-change run logs at `/tmp/autocoder/logs/<basename>/`, and a few
//! ancillary state files. Tmpfs-on-`/tmp` distros wiped all of it on
//! reboot, with the downstream symptoms documented in the change
//! proposal (audit storms, lost markers, reset failure counters).
//!
//! On first startup after upgrade — detected by the absence of
//! `<state_dir>/.migration-from-tmp-done` — the daemon scans the
//! well-known legacy paths and moves their contents to the new
//! locations resolved from [`crate::paths::DaemonPaths`]. The
//! migration is idempotent (the marker is what gates the scan), per-
//! entry error-tolerant (one failing entry does not abort the rest),
//! and writes the marker only when every entry completed without
//! error. Cross-partition moves (tmpfs → disk is the common case) fall
//! back to recursive copy + delete-on-success when `fs::rename`
//! returns EXDEV.

use crate::paths::DaemonPaths;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Marker file written under `<state_dir>` after a clean migration
/// pass. Its presence is the signal "no legacy /tmp scan needed";
/// every subsequent daemon start is a no-op until the file is
/// manually removed (which an operator might do to force a re-scan
/// after restoring data from backup).
pub const MIGRATION_MARKER: &str = ".migration-from-tmp-done";

/// Summary returned by [`migrate_legacy_tmp_paths`]. Counts each
/// category of moved entries plus a per-entry error list. The error
/// list is non-empty iff the marker file was NOT written (and the
/// daemon will retry the migration on the next start).
#[derive(Debug, Default, Clone)]
pub struct MigrationReport {
    pub workspaces_moved: u32,
    pub state_files_moved: u32,
    pub log_files_moved: u32,
    pub errors: Vec<String>,
}

impl MigrationReport {
    /// Total non-error moves across all categories. Useful for the
    /// summary log line emitted by the daemon after migration.
    #[allow(dead_code)]
    pub fn total_moved(&self) -> u32 {
        self.workspaces_moved
            .saturating_add(self.state_files_moved)
            .saturating_add(self.log_files_moved)
    }

    /// `true` when at least one entry failed. Drives the
    /// "marker NOT written → retry on next start" rule.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// Scan the well-known legacy `/tmp` paths and move each entry into
/// the new layout under `daemon_paths`. Returns a [`MigrationReport`]
/// with per-category counts and any per-entry errors.
///
/// If `<state_dir>/.migration-from-tmp-done` already exists, the
/// function returns immediately with an empty report. Operators who
/// want to re-run the migration (e.g. after restoring legacy data
/// from backup) remove the marker manually.
pub fn migrate_legacy_tmp_paths(daemon_paths: &DaemonPaths) -> Result<MigrationReport> {
    migrate_at(daemon_paths, &legacy_roots_default())
}

/// Aggregated source-path constants. Exposed (private) so the tests
/// can swap the legacy roots out to a tempdir.
#[derive(Debug, Clone)]
pub(crate) struct LegacyRoots {
    pub workspaces: PathBuf,
    pub audit_state: PathBuf,
    pub failure_state: PathBuf,
    pub revisions: PathBuf,
    pub logs: PathBuf,
}

fn legacy_roots_default() -> LegacyRoots {
    LegacyRoots {
        workspaces: PathBuf::from("/tmp/workspaces"),
        audit_state: PathBuf::from("/tmp/autocoder/audit-state"),
        failure_state: PathBuf::from("/tmp/autocoder/failure-state"),
        revisions: PathBuf::from("/tmp/autocoder/revisions"),
        logs: PathBuf::from("/tmp/autocoder/logs"),
    }
}

pub(crate) fn migrate_at(
    daemon_paths: &DaemonPaths,
    legacy: &LegacyRoots,
) -> Result<MigrationReport> {
    let marker = daemon_paths.state.join(MIGRATION_MARKER);
    if marker.exists() {
        tracing::debug!(
            marker = %marker.display(),
            "legacy /tmp migration: marker present, skipping scan"
        );
        return Ok(MigrationReport::default());
    }

    let mut report = MigrationReport::default();

    // 1. Audit-state: per-file moves into <state_dir>/audit-state/.
    move_flat_files(
        &legacy.audit_state,
        &daemon_paths.state.join("audit-state"),
        &mut report,
        "json",
        Counter::State,
    );

    // 2. Failure-state: recursive (per-repo subdirs + per-change files).
    move_recursive(
        &legacy.failure_state,
        &daemon_paths.state.join("failure-state"),
        &mut report,
        Counter::State,
    );

    // 3. Revisions: same recursive shape (per-repo subdirs + per-PR files).
    move_recursive(
        &legacy.revisions,
        &daemon_paths.state.join("revisions"),
        &mut report,
        Counter::State,
    );

    // 4. Per-change run logs: move /tmp/autocoder/logs → <logs_dir>/runs/.
    move_recursive(
        &legacy.logs,
        &daemon_paths.logs.join("runs"),
        &mut report,
        Counter::Logs,
    );

    // 5. Workspaces: per-entry (top-level directories under
    //    /tmp/workspaces). Cross-partition rename falls back to copy
    //    + delete inside `move_one_dir`.
    move_workspaces(
        &legacy.workspaces,
        &daemon_paths.cache.join("workspaces"),
        &mut report,
    );

    // 6. Marker: only written when every entry succeeded. A partial
    //    migration keeps the marker absent so the next start retries.
    if report.errors.is_empty() {
        if let Err(e) = ensure_state_dir(&daemon_paths.state) {
            tracing::error!(
                state = %daemon_paths.state.display(),
                "could not create state dir to write migration marker: {e:#}"
            );
            report.errors.push(format!(
                "creating state dir for marker {}: {e}",
                daemon_paths.state.display()
            ));
        } else if let Err(e) = std::fs::write(&marker, "ok\n") {
            tracing::error!(
                marker = %marker.display(),
                "writing migration marker failed: {e}"
            );
            report.errors.push(format!(
                "writing marker {}: {e}",
                marker.display()
            ));
        }
    }

    tracing::info!(
        workspaces_moved = report.workspaces_moved,
        state_files_moved = report.state_files_moved,
        log_files_moved = report.log_files_moved,
        errors = report.errors.len(),
        marker_written = !report.has_errors(),
        "legacy /tmp migration complete"
    );

    Ok(report)
}

fn ensure_state_dir(state_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(state_dir)
        .with_context(|| format!("creating {}", state_dir.display()))
}

/// Categorizes a moved entry for the [`MigrationReport`] counters.
#[derive(Copy, Clone)]
enum Counter {
    State,
    Logs,
}

/// Move every flat file matching `extension` from `src_dir` to
/// `dst_dir`. Non-matching files are ignored. Subdirectories are
/// ignored at this layer — see [`move_recursive`] for the nested
/// variant.
fn move_flat_files(
    src_dir: &Path,
    dst_dir: &Path,
    report: &mut MigrationReport,
    extension: &str,
    counter: Counter,
) {
    let read = match std::fs::read_dir(src_dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::error!(
                src = %src_dir.display(),
                "migration: read_dir failed: {e}"
            );
            report
                .errors
                .push(format!("read_dir {}: {e}", src_dir.display()));
            return;
        }
    };
    for entry in read {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                report
                    .errors
                    .push(format!("read_dir entry in {}: {e}", src_dir.display()));
                continue;
            }
        };
        let src = entry.path();
        if !src.is_file() {
            continue;
        }
        if src.extension().and_then(|s| s.to_str()) != Some(extension) {
            continue;
        }
        let name = entry.file_name();
        let dst = dst_dir.join(&name);
        move_one(&src, &dst, report, counter);
    }
}

/// Recursive variant: copy `<src>/**/*` to the same relative path
/// under `dst`. Files only — directories are created on demand.
fn move_recursive(
    src_root: &Path,
    dst_root: &Path,
    report: &mut MigrationReport,
    counter: Counter,
) {
    if !src_root.exists() {
        return;
    }
    walk_files(src_root, src_root, dst_root, report, counter);
    // After the file moves, prune any empty source subdirectories.
    // Best-effort: ignore errors (the source root may be left in
    // place on partial migrations, which is fine).
    let _ = prune_empty_dirs(src_root);
}

fn walk_files(
    src_root: &Path,
    cursor: &Path,
    dst_root: &Path,
    report: &mut MigrationReport,
    counter: Counter,
) {
    let read = match std::fs::read_dir(cursor) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::error!(
                src = %cursor.display(),
                "migration: read_dir failed: {e}"
            );
            report
                .errors
                .push(format!("read_dir {}: {e}", cursor.display()));
            return;
        }
    };
    for entry in read {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                report
                    .errors
                    .push(format!("read_dir entry in {}: {e}", cursor.display()));
                continue;
            }
        };
        let src = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(e) => {
                report
                    .errors
                    .push(format!("file_type {}: {e}", src.display()));
                continue;
            }
        };
        if file_type.is_dir() {
            walk_files(src_root, &src, dst_root, report, counter);
        } else if file_type.is_file() {
            let rel = match src.strip_prefix(src_root) {
                Ok(r) => r.to_path_buf(),
                Err(_) => {
                    // strip_prefix should always succeed here; skip
                    // defensively rather than panic.
                    continue;
                }
            };
            let dst = dst_root.join(rel);
            move_one(&src, &dst, report, counter);
        }
    }
}

fn move_workspaces(
    src_root: &Path,
    dst_root: &Path,
    report: &mut MigrationReport,
) {
    let read = match std::fs::read_dir(src_root) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::error!(
                src = %src_root.display(),
                "migration: read_dir failed: {e}"
            );
            report
                .errors
                .push(format!("read_dir {}: {e}", src_root.display()));
            return;
        }
    };
    for entry in read {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                report
                    .errors
                    .push(format!("read_dir entry in {}: {e}", src_root.display()));
                continue;
            }
        };
        let src = entry.path();
        if !src.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let dst = dst_root.join(&name);
        move_one_dir(&src, &dst, report);
    }
}

fn move_one(src: &Path, dst: &Path, report: &mut MigrationReport, counter: Counter) {
    if dst.exists() {
        tracing::info!(
            src = %src.display(),
            dst = %dst.display(),
            "migration: target already exists, skipping (source left in place)"
        );
        return;
    }
    if let Some(parent) = dst.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::error!(
            parent = %parent.display(),
            "migration: failed to create destination parent: {e}"
        );
        report.errors.push(format!(
            "creating dst parent {}: {e}",
            parent.display()
        ));
        return;
    }
    tracing::info!(
        src = %src.display(),
        dst = %dst.display(),
        "migration: moving file"
    );
    match std::fs::rename(src, dst) {
        Ok(()) => bump_counter(report, counter, 1),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => match copy_then_remove_file(src, dst) {
            Ok(()) => bump_counter(report, counter, 1),
            Err(e2) => {
                tracing::error!(
                    src = %src.display(),
                    dst = %dst.display(),
                    "migration: cross-partition copy+delete failed: {e2:#}"
                );
                report.errors.push(format!(
                    "copy+delete {} → {}: {e2}",
                    src.display(),
                    dst.display()
                ));
            }
        },
        Err(e) => {
            tracing::error!(
                src = %src.display(),
                dst = %dst.display(),
                "migration: rename failed: {e}"
            );
            report.errors.push(format!(
                "rename {} → {}: {e}",
                src.display(),
                dst.display()
            ));
        }
    }
}

fn move_one_dir(src: &Path, dst: &Path, report: &mut MigrationReport) {
    if dst.exists() {
        tracing::info!(
            src = %src.display(),
            dst = %dst.display(),
            "migration: workspace target already exists, skipping (source left in place)"
        );
        return;
    }
    if let Some(parent) = dst.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::error!(
            parent = %parent.display(),
            "migration: failed to create workspace dst parent: {e}"
        );
        report.errors.push(format!(
            "creating workspace dst parent {}: {e}",
            parent.display()
        ));
        return;
    }
    tracing::info!(
        src = %src.display(),
        dst = %dst.display(),
        "migration: moving workspace"
    );
    match std::fs::rename(src, dst) {
        Ok(()) => {
            report.workspaces_moved = report.workspaces_moved.saturating_add(1);
        }
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => match copy_dir_recursive(src, dst) {
            Ok(()) => {
                if let Err(rm) = std::fs::remove_dir_all(src) {
                    tracing::error!(
                        src = %src.display(),
                        "migration: cross-partition copy succeeded but remove_dir_all failed: {rm}"
                    );
                    report.errors.push(format!(
                        "remove_dir_all {} after copy: {rm}",
                        src.display()
                    ));
                } else {
                    report.workspaces_moved = report.workspaces_moved.saturating_add(1);
                }
            }
            Err(e2) => {
                tracing::error!(
                    src = %src.display(),
                    dst = %dst.display(),
                    "migration: workspace cross-partition copy failed: {e2:#}"
                );
                report.errors.push(format!(
                    "copy_dir_recursive {} → {}: {e2}",
                    src.display(),
                    dst.display()
                ));
            }
        },
        Err(e) => {
            tracing::error!(
                src = %src.display(),
                dst = %dst.display(),
                "migration: workspace rename failed: {e}"
            );
            report.errors.push(format!(
                "rename {} → {}: {e}",
                src.display(),
                dst.display()
            ));
        }
    }
}

fn bump_counter(report: &mut MigrationReport, counter: Counter, n: u32) {
    match counter {
        Counter::State => report.state_files_moved = report.state_files_moved.saturating_add(n),
        Counter::Logs => report.log_files_moved = report.log_files_moved.saturating_add(n),
    }
}

fn copy_then_remove_file(src: &Path, dst: &Path) -> Result<()> {
    std::fs::copy(src, dst)
        .with_context(|| format!("copy {} → {}", src.display(), dst.display()))?;
    std::fs::remove_file(src)
        .with_context(|| format!("remove {}", src.display()))?;
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)
        .with_context(|| format!("create_dir_all {}", dst.display()))?;
    for entry in std::fs::read_dir(src)
        .with_context(|| format!("read_dir {}", src.display()))?
    {
        let entry = entry.with_context(|| format!("read_dir entry in {}", src.display()))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry
            .file_type()
            .with_context(|| format!("file_type {}", from.display()))?;
        if ft.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to)
                .with_context(|| format!("copy {} → {}", from.display(), to.display()))?;
        } else if ft.is_symlink() {
            let target = std::fs::read_link(&from)
                .with_context(|| format!("read_link {}", from.display()))?;
            std::os::unix::fs::symlink(target, &to)
                .with_context(|| format!("symlink {}", to.display()))?;
        }
    }
    Ok(())
}

/// Best-effort: walk `root` bottom-up, removing any empty
/// subdirectory. The root itself is preserved (so a partially-failed
/// migration's source path stays observable to operators).
fn prune_empty_dirs(root: &Path) -> Result<()> {
    fn recurse(path: &Path) -> Result<()> {
        if !path.is_dir() {
            return Ok(());
        }
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let p = entry.path();
            if p.is_dir() {
                recurse(&p)?;
                let _ = std::fs::remove_dir(&p); // ignore non-empty
            }
        }
        Ok(())
    }
    recurse(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build a `DaemonPaths` rooted under a fresh tempdir and create
    /// the four directories on disk so the migration can move into
    /// them.
    fn fixture_daemon_paths() -> (TempDir, DaemonPaths) {
        let dir = TempDir::new().unwrap();
        let paths = DaemonPaths::under_root(dir.path());
        crate::paths::ensure_directories(&paths).unwrap();
        (dir, paths)
    }

    /// Build a legacy roots structure rooted under a fresh tempdir
    /// (so the migration's source-paths constants don't reach into
    /// the real `/tmp/`).
    fn fixture_legacy_roots() -> (TempDir, LegacyRoots) {
        let dir = TempDir::new().unwrap();
        let legacy = LegacyRoots {
            workspaces: dir.path().join("workspaces"),
            audit_state: dir.path().join("autocoder/audit-state"),
            failure_state: dir.path().join("autocoder/failure-state"),
            revisions: dir.path().join("autocoder/revisions"),
            logs: dir.path().join("autocoder/logs"),
        };
        (dir, legacy)
    }

    #[test]
    fn empty_legacy_paths_writes_marker_with_no_moves() {
        let (_paths_dir, paths) = fixture_daemon_paths();
        let (_legacy_dir, legacy) = fixture_legacy_roots();
        let report = migrate_at(&paths, &legacy).unwrap();
        assert_eq!(report.total_moved(), 0);
        assert!(!report.has_errors());
        // Marker present.
        assert!(paths.state.join(MIGRATION_MARKER).exists());
    }

    #[test]
    fn marker_present_skips_scan() {
        let (_paths_dir, paths) = fixture_daemon_paths();
        std::fs::write(paths.state.join(MIGRATION_MARKER), "prior").unwrap();
        let (_legacy_dir, legacy) = fixture_legacy_roots();
        // Populate legacy with content; migration should NOT move it.
        std::fs::create_dir_all(&legacy.workspaces).unwrap();
        let dummy = legacy.workspaces.join("repo");
        std::fs::create_dir_all(&dummy).unwrap();
        std::fs::write(dummy.join("README"), "hi").unwrap();

        let report = migrate_at(&paths, &legacy).unwrap();
        assert_eq!(report.total_moved(), 0);
        assert!(!report.has_errors());
        // Source NOT touched.
        assert!(dummy.exists());
        assert!(dummy.join("README").exists());
    }

    #[test]
    fn workspaces_are_moved_to_cache_subdir() {
        let (_paths_dir, paths) = fixture_daemon_paths();
        let (_legacy_dir, legacy) = fixture_legacy_roots();
        std::fs::create_dir_all(&legacy.workspaces).unwrap();
        let repo = legacy.workspaces.join("github_com_owner_repo");
        std::fs::create_dir_all(repo.join("nested/inner")).unwrap();
        std::fs::write(repo.join("README"), "hi").unwrap();
        std::fs::write(repo.join("nested/inner/file.txt"), "x").unwrap();

        let report = migrate_at(&paths, &legacy).unwrap();
        assert_eq!(report.workspaces_moved, 1);
        assert!(!report.has_errors());

        let dst = paths.cache.join("workspaces/github_com_owner_repo");
        assert!(dst.exists(), "target dir should exist: {}", dst.display());
        assert!(dst.join("README").is_file());
        assert!(dst.join("nested/inner/file.txt").is_file());
        assert!(!repo.exists(), "source must be removed after successful move");
    }

    #[test]
    fn state_files_moved_into_categorized_subdirs() {
        let (_paths_dir, paths) = fixture_daemon_paths();
        let (_legacy_dir, legacy) = fixture_legacy_roots();
        std::fs::create_dir_all(&legacy.audit_state).unwrap();
        std::fs::write(legacy.audit_state.join("a1.json"), "{}").unwrap();
        std::fs::write(legacy.audit_state.join("a2.json"), "{}").unwrap();
        std::fs::write(legacy.audit_state.join("README.txt"), "ignored").unwrap();
        std::fs::create_dir_all(legacy.failure_state.join("repo-x")).unwrap();
        std::fs::write(
            legacy.failure_state.join("repo-x/change1.json"),
            "{}",
        )
        .unwrap();

        let report = migrate_at(&paths, &legacy).unwrap();
        assert!(!report.has_errors(), "errors: {:?}", report.errors);
        // 2 audit-state json + 1 failure-state json = 3
        assert_eq!(report.state_files_moved, 3);

        assert!(paths.state.join("audit-state/a1.json").exists());
        assert!(paths.state.join("audit-state/a2.json").exists());
        assert!(paths.state.join("failure-state/repo-x/change1.json").exists());
        // README.txt was non-matching and stays at source.
        assert!(legacy.audit_state.join("README.txt").exists());
    }

    #[test]
    fn target_exists_skips_and_leaves_source() {
        let (_paths_dir, paths) = fixture_daemon_paths();
        let (_legacy_dir, legacy) = fixture_legacy_roots();
        std::fs::create_dir_all(&legacy.audit_state).unwrap();
        std::fs::write(legacy.audit_state.join("a1.json"), "src-content").unwrap();
        // Pre-populate target.
        let target_dir = paths.state.join("audit-state");
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::write(target_dir.join("a1.json"), "existing-content").unwrap();

        let report = migrate_at(&paths, &legacy).unwrap();
        assert!(!report.has_errors());
        // Source preserved.
        assert!(legacy.audit_state.join("a1.json").exists());
        // Target preserved (not overwritten).
        let contents = std::fs::read_to_string(target_dir.join("a1.json")).unwrap();
        assert_eq!(contents, "existing-content");
    }

    #[test]
    fn idempotent_second_run_does_no_work() {
        let (_paths_dir, paths) = fixture_daemon_paths();
        let (_legacy_dir, legacy) = fixture_legacy_roots();
        std::fs::create_dir_all(&legacy.audit_state).unwrap();
        std::fs::write(legacy.audit_state.join("a1.json"), "{}").unwrap();
        // First run: moves the file + writes marker.
        let r1 = migrate_at(&paths, &legacy).unwrap();
        assert_eq!(r1.state_files_moved, 1);
        // Second run: marker present, no work.
        let r2 = migrate_at(&paths, &legacy).unwrap();
        assert_eq!(r2.total_moved(), 0);
    }

    #[test]
    fn log_files_moved_under_runs_subdir() {
        let (_paths_dir, paths) = fixture_daemon_paths();
        let (_legacy_dir, legacy) = fixture_legacy_roots();
        std::fs::create_dir_all(legacy.logs.join("github_com_owner_repo")).unwrap();
        std::fs::write(
            legacy.logs.join("github_com_owner_repo/my-change.log"),
            "log content",
        )
        .unwrap();
        let report = migrate_at(&paths, &legacy).unwrap();
        assert!(!report.has_errors());
        assert_eq!(report.log_files_moved, 1);
        let target = paths
            .logs
            .join("runs/github_com_owner_repo/my-change.log");
        assert!(target.exists(), "target must be created: {}", target.display());
        let content = std::fs::read_to_string(&target).unwrap();
        assert_eq!(content, "log content");
    }

    #[test]
    fn partial_failure_does_not_write_marker() {
        let (_paths_dir, paths) = fixture_daemon_paths();
        let (_legacy_dir, legacy) = fixture_legacy_roots();
        // Populate the legacy audit-state dir with a file that should
        // succeed.
        std::fs::create_dir_all(&legacy.audit_state).unwrap();
        std::fs::write(legacy.audit_state.join("good.json"), "{}").unwrap();
        // Pre-create the destination of the audit-state dir as a
        // FILE (not a directory) so create_dir_all on the parent
        // fails when migration tries to make the audit-state target
        // dir. The good.json move fails as a result, recording an
        // error.
        std::fs::write(paths.state.join("audit-state"), "blocking-file").unwrap();
        let report = migrate_at(&paths, &legacy).unwrap();
        // Marker NOT written because there was an error.
        assert!(report.has_errors(), "failed move must record an error");
        assert!(
            !paths.state.join(MIGRATION_MARKER).exists(),
            "marker must NOT be written when errors occurred"
        );
        // The source is left in place for the operator to inspect.
        assert!(
            legacy.audit_state.join("good.json").exists(),
            "source preserved on failure"
        );
    }
}
