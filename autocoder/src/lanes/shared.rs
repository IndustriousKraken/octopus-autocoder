//! Stateless leaf primitives composed by BOTH lane walkers (a009 §1).
//!
//! The changes walker and the issues walker share a set of leaf
//! operations: the busy-marker (the per-repo serializer), PR opening,
//! archiving, chatops notification, queue-state I/O, AND workspace
//! handling. This module is the single composition surface for them:
//! each primitive has ONE definition (here, or delegated to the one
//! canonical definition elsewhere — `crate::workspace`,
//! `crate::busy_marker`, `crate::chatops`) and is composed by callers
//! rather than copied per lane. A fault in one lane's walker cannot
//! reach the other lane's control flow because these functions are
//! stateless — they hold no lane state of their own.
//!
//! What lives where:
//!   - **workspace handling** — [`resolve_workspace`] → `crate::workspace`.
//!   - **busy-marker** — [`record_busy_unit`] → `crate::busy_marker`.
//!     Acquire/release of the per-repo busy guard is shared at the pass
//!     level (`crate::polling_loop::execute_one_pass` →
//!     `busy_marker::try_acquire`); the issues walker runs WITHIN that
//!     already-held guard AND only records which unit it is on.
//!   - **chatops notification** — [`notify`] → `crate::chatops`'s
//!     `post_notification`.
//!   - **queue-state I/O** — [`acquire_lock`] / [`release_lock`]: the
//!     `.in-progress` lock-file create/remove inside a unit directory.
//!   - **archiving** — [`archive_dir_with_postcondition`]: the dated
//!     move + postcondition check both `changes/archive/` (via openspec)
//!     AND `issues/archive/` mirror.
//!   - **PR opening** — composed at the pass level
//!     (`crate::polling_loop::open_pull_request`): both lanes' commits
//!     ride the SAME pass push + PR step, so the PR-open primitive is
//!     not re-invoked per lane.

use crate::busy_marker;
use crate::config::RepositoryConfig;
use crate::paths::DaemonPaths;
use crate::polling_loop::ChatOpsContext;
use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};

/// The `.in-progress` lock filename, shared by both lanes' unit
/// directories. The changes lane writes it under
/// `openspec/changes/<slug>/`; the issues lane under
/// `issues/<slug>/`.
pub const LOCK_FILE: &str = ".in-progress";

// ----- workspace handling -----

/// Resolve the on-disk workspace path for `repo`. The single definition
/// is `crate::workspace::resolve_path`; both lanes receive the resolved
/// `workspace` threaded down from the pass (which calls that one
/// definition). This wrapper is the shared module's named entry point for
/// the workspace-handling primitive — kept for symmetry with the other
/// five leaf primitives even though the pass resolves once upstream.
#[allow(dead_code)]
pub fn resolve_workspace(paths: &DaemonPaths, repo: &RepositoryConfig) -> PathBuf {
    crate::workspace::resolve_path(paths, repo)
}

// ----- busy-marker (per-repo serializer) -----

/// Record which unit of work the held busy-marker is currently on so the
/// chatops `status` reply renders `currently: working on <unit>`.
/// Best-effort; delegates to the single `busy_marker::update_change`
/// definition.
pub fn record_busy_unit(paths: &DaemonPaths, workspace: &Path, unit: &str) {
    busy_marker::update_change(paths, workspace, unit);
}

// ----- chatops notification -----

/// Post a one-line notification to the repo's configured channel when
/// chatops is wired. Best-effort: a failed post logs at WARN AND never
/// aborts the caller. The single underlying definition is
/// `ChatOpsBackend::post_notification`.
pub async fn notify(chatops_ctx: Option<&ChatOpsContext>, text: &str) {
    let Some(ctx) = chatops_ctx else { return };
    if let Err(e) = ctx.chatops.post_notification(&ctx.channel, text).await {
        tracing::warn!("lane chatops notification failed; continuing: {e:#}");
    }
}

// ----- queue-state I/O -----

/// Create the `.in-progress` lock file inside a unit directory. The unit
/// directory is the change OR issue directory; the lock is the same
/// shape for both lanes.
pub fn acquire_lock(unit_dir: &Path) -> Result<()> {
    let path = unit_dir.join(LOCK_FILE);
    std::fs::File::create(&path)
        .with_context(|| format!("creating lock file {}", path.display()))?;
    Ok(())
}

/// Remove the `.in-progress` lock file inside a unit directory.
/// Idempotent: returns `Ok` if the lock is already absent.
pub fn release_lock(unit_dir: &Path) -> Result<()> {
    let path = unit_dir.join(LOCK_FILE);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing lock file {}", path.display())),
    }
}

// ----- archiving (dated move + postcondition) -----

/// Move `active_dir` into `archive_root/<dated_name>/` AND verify the
/// post-condition: the source directory is gone AND the dated archive
/// entry exists. Creates `archive_root` if absent. Errors when the dated
/// destination already exists (a same-day re-archive collision) OR the
/// post-condition does not hold after the rename.
///
/// This is the shared "archive-with-postcondition" leaf. The changes
/// lane reaches its dated move through `openspec archive` (which ALSO
/// applies the delta to canon); the issues lane reaches it directly here
/// (a pure move that touches NO canonical spec). Both verify the same
/// post-condition shape.
pub fn archive_dir_with_postcondition(
    active_dir: &Path,
    archive_root: &Path,
    dated_name: &str,
) -> Result<PathBuf> {
    if !active_dir.is_dir() {
        return Err(anyhow!(
            "cannot archive: source directory {} not found",
            active_dir.display()
        ));
    }
    std::fs::create_dir_all(archive_root)
        .with_context(|| format!("creating archive root {}", archive_root.display()))?;
    let dest = archive_root.join(dated_name);
    if dest.exists() {
        return Err(anyhow!(
            "archive destination already exists: {}",
            dest.display()
        ));
    }
    std::fs::rename(active_dir, &dest)
        .with_context(|| format!("renaming {} to {}", active_dir.display(), dest.display()))?;
    // Post-condition: source moved AND dated entry produced.
    if active_dir.exists() {
        return Err(anyhow!(
            "archive reported success but the source directory at {} still exists",
            active_dir.display()
        ));
    }
    if !dest.is_dir() {
        return Err(anyhow!(
            "archive reported success but the dated entry at {} does not exist",
            dest.display()
        ));
    }
    Ok(dest)
}

/// Move `active_file` into `archive_root/<dated_name>` AND verify the
/// post-condition: the source file is gone AND the dated archive entry
/// exists as a FILE. Creates `archive_root` if absent. Errors when the
/// dated destination already exists (a same-day re-archive collision) OR
/// the post-condition does not hold after the rename.
///
/// This is the file-unit sibling of [`archive_dir_with_postcondition`]:
/// the issues lane's single-file form (`issues/<slug>.md`) archives to
/// `issues/archive/<UTC-date>-<slug>.md` (a file), where the directory
/// primitive's `is_dir()` assertions would reject both the source AND the
/// destination. A pure move that touches NO canonical spec.
pub fn archive_file_with_postcondition(
    active_file: &Path,
    archive_root: &Path,
    dated_name: &str,
) -> Result<PathBuf> {
    if !active_file.is_file() {
        return Err(anyhow!(
            "cannot archive: source file {} not found",
            active_file.display()
        ));
    }
    std::fs::create_dir_all(archive_root)
        .with_context(|| format!("creating archive root {}", archive_root.display()))?;
    let dest = archive_root.join(dated_name);
    if dest.exists() {
        return Err(anyhow!(
            "archive destination already exists: {}",
            dest.display()
        ));
    }
    std::fs::rename(active_file, &dest)
        .with_context(|| format!("renaming {} to {}", active_file.display(), dest.display()))?;
    // Post-condition: source moved AND dated entry produced.
    if active_file.exists() {
        return Err(anyhow!(
            "archive reported success but the source file at {} still exists",
            active_file.display()
        ));
    }
    if !dest.is_file() {
        return Err(anyhow!(
            "archive reported success but the dated entry at {} does not exist as a file",
            dest.display()
        ));
    }
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn lock_acquire_release_round_trip() {
        let td = TempDir::new().unwrap();
        let unit = td.path().join("unit");
        std::fs::create_dir_all(&unit).unwrap();
        let lock = unit.join(LOCK_FILE);
        assert!(!lock.exists());
        acquire_lock(&unit).unwrap();
        assert!(lock.exists());
        release_lock(&unit).unwrap();
        assert!(!lock.exists());
        // Idempotent second release.
        release_lock(&unit).unwrap();
    }

    #[test]
    fn archive_moves_dir_and_checks_postcondition() {
        let td = TempDir::new().unwrap();
        let active = td.path().join("my-unit");
        std::fs::create_dir_all(&active).unwrap();
        std::fs::write(active.join("file.txt"), "x").unwrap();
        let archive_root = td.path().join("archive");

        let dest = archive_dir_with_postcondition(&active, &archive_root, "2026-06-05-my-unit")
            .unwrap();

        assert!(!active.exists(), "source must be gone");
        assert!(dest.is_dir(), "dated entry must exist");
        assert_eq!(dest, archive_root.join("2026-06-05-my-unit"));
        assert!(dest.join("file.txt").exists(), "contents preserved");
    }

    #[test]
    fn archive_errors_on_missing_source() {
        let td = TempDir::new().unwrap();
        let err =
            archive_dir_with_postcondition(&td.path().join("nope"), &td.path().join("a"), "x-nope")
                .expect_err("missing source must error");
        assert!(format!("{err:#}").contains("not found"));
    }

    #[test]
    fn archive_errors_on_collision() {
        let td = TempDir::new().unwrap();
        let active = td.path().join("u");
        std::fs::create_dir_all(&active).unwrap();
        let archive_root = td.path().join("archive");
        std::fs::create_dir_all(archive_root.join("2026-06-05-u")).unwrap();
        let err = archive_dir_with_postcondition(&active, &archive_root, "2026-06-05-u")
            .expect_err("collision must error");
        assert!(format!("{err:#}").contains("already exists"));
        // Source untouched on the error path.
        assert!(active.exists());
    }

    #[test]
    fn archive_file_moves_file_and_checks_postcondition() {
        let td = TempDir::new().unwrap();
        let active = td.path().join("my-unit.md");
        std::fs::write(&active, "body").unwrap();
        let archive_root = td.path().join("archive");

        let dest =
            archive_file_with_postcondition(&active, &archive_root, "2026-06-05-my-unit.md")
                .unwrap();

        assert!(!active.exists(), "source must be gone");
        assert!(dest.is_file(), "dated entry must exist as a file");
        assert_eq!(dest, archive_root.join("2026-06-05-my-unit.md"));
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "body");
    }

    #[test]
    fn archive_file_errors_on_missing_source() {
        let td = TempDir::new().unwrap();
        let err = archive_file_with_postcondition(
            &td.path().join("nope.md"),
            &td.path().join("a"),
            "x-nope.md",
        )
        .expect_err("missing source must error");
        assert!(format!("{err:#}").contains("not found"));
    }

    #[test]
    fn archive_file_errors_on_collision() {
        let td = TempDir::new().unwrap();
        let active = td.path().join("u.md");
        std::fs::write(&active, "x").unwrap();
        let archive_root = td.path().join("archive");
        std::fs::create_dir_all(&archive_root).unwrap();
        std::fs::write(archive_root.join("2026-06-05-u.md"), "old").unwrap();
        let err = archive_file_with_postcondition(&active, &archive_root, "2026-06-05-u.md")
            .expect_err("collision must error");
        assert!(format!("{err:#}").contains("already exists"));
        assert!(active.exists());
    }
}
