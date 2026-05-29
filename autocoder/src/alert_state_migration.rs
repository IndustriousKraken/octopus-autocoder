//! First-startup migration: move legacy workspace-rooted
//! `.alert-state.json` files into the standard
//! `<state_dir>/alert-state/<workspace-basename>.json` layout introduced
//! by `a16-consolidate-workspace-bookkeeping-to-state-dir`.
//!
//! Idempotency: a single daemon-wide marker file
//! (`<state_dir>/alert-state/.migration-from-workspace-done`) records
//! that the scan ran. The migration is a no-op once the marker is
//! present. Per-repository errors (notably: `git push` rejected by
//! branch protection on a tracked-in-git file) do NOT abort the batch
//! but DO suppress the marker — the next startup re-attempts every
//! repository, with already-migrated repos becoming idempotent no-ops
//! (file absent on subsequent re-check).
//!
//! The migration runs at daemon startup BEFORE any polling task. The
//! workspace-init invariant check in `workspace::ensure_initialized`
//! complements this by sweeping any transient `.alert-state.json` that
//! reappears in a workspace AFTER the marker is set (e.g. a fresh
//! re-clone of a repo whose history transiently committed the file).

use crate::config::{GithubConfig, RepositoryConfig};
use crate::paths::DaemonPaths;
use crate::workspace;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Marker file written under `<state_dir>/alert-state/` once a clean
/// migration pass completes (zero per-repo errors). Operators can force
/// a re-scan by removing it.
pub const MIGRATION_MARKER: &str = ".migration-from-workspace-done";

/// Commit subject pinned by `a16`'s spec for the rare git-tracked case.
const REMOVAL_COMMIT_SUBJECT: &str =
    "chore: untrack .alert-state.json (now stored in daemon state dir per a16)";

/// Per-workspace outcome, used to decide whether to write the marker.
#[derive(Debug, Clone)]
enum RepoOutcome {
    /// No workspace file present (already migrated or never existed).
    NoOp,
    /// Workspace file was migrated cleanly.
    Migrated,
    /// State-dir version already present; deleted the workspace copy.
    PreferredStateDir,
    /// Git-tracked workspace file untracked + committed + pushed.
    UntrackedAndPushed,
    /// Per-repo failure (push rejected, etc.). Marker NOT set.
    Failed(String),
}

/// Scan every configured repository's workspace and migrate any
/// pre-existing `.alert-state.json` files into the state-dir layout.
/// Writes the migration marker only when every repo's outcome was
/// non-`Failed`. Tolerates per-repo errors so a single misconfigured
/// repo does not block migration of the others.
pub fn migrate_alert_state_from_workspace(
    paths: &DaemonPaths,
    repos: &[RepositoryConfig],
    github_cfg: &GithubConfig,
) -> Result<()> {
    let marker = paths.alert_state_dir().join(MIGRATION_MARKER);
    if marker.exists() {
        tracing::debug!(
            marker = %marker.display(),
            "alert-state migration: marker present, skipping scan"
        );
        return Ok(());
    }

    let mut any_failed = false;
    let mut migrated = 0u32;
    let mut preferred_state_dir = 0u32;
    let mut untracked_and_pushed = 0u32;
    let mut no_op = 0u32;
    let mut failed = 0u32;

    for repo in repos {
        let workspace_path = workspace::resolve_path(repo);
        let basename = workspace_basename(&workspace_path);
        let outcome = migrate_one_repo(paths, repo, github_cfg, &workspace_path, &basename);
        match &outcome {
            RepoOutcome::NoOp => no_op += 1,
            RepoOutcome::Migrated => migrated += 1,
            RepoOutcome::PreferredStateDir => preferred_state_dir += 1,
            RepoOutcome::UntrackedAndPushed => untracked_and_pushed += 1,
            RepoOutcome::Failed(reason) => {
                any_failed = true;
                failed += 1;
                tracing::error!(
                    url = %repo.url,
                    workspace = %workspace_path.display(),
                    "alert-state migration: repo failed: {reason}; \
                     operator action: manual `git rm --cached .alert-state.json` + PR. \
                     Daemon continues; marker NOT set; next startup retries."
                );
            }
        }
    }

    if any_failed {
        tracing::error!(
            migrated,
            preferred_state_dir,
            untracked_and_pushed,
            no_op,
            failed,
            "alert-state migration: completed with errors; marker NOT written (next startup retries)"
        );
        return Ok(());
    }

    // All repos succeeded (including the all-no-op case): write the
    // marker so the next startup skips the scan.
    if let Err(e) = std::fs::create_dir_all(paths.alert_state_dir()) {
        tracing::error!(
            dir = %paths.alert_state_dir().display(),
            "alert-state migration: could not create alert-state dir for marker: {e}"
        );
        return Ok(());
    }
    if let Err(e) = std::fs::write(&marker, "ok\n") {
        tracing::error!(
            marker = %marker.display(),
            "alert-state migration: writing marker failed: {e}"
        );
        return Ok(());
    }
    if migrated + preferred_state_dir + untracked_and_pushed == 0 {
        tracing::info!(
            "alert-state migration: no workspace files needed migration; marker written"
        );
    } else {
        tracing::info!(
            migrated,
            preferred_state_dir,
            untracked_and_pushed,
            no_op,
            "alert-state migration: complete; marker written"
        );
    }
    Ok(())
}

/// Derive the sanitized workspace-basename for a workspace path. Falls
/// back to `"unknown"` if `file_name()` is missing (should never happen
/// for derived paths but stays defensive).
fn workspace_basename(workspace_path: &Path) -> String {
    workspace_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string())
}

fn migrate_one_repo(
    paths: &DaemonPaths,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    workspace_path: &Path,
    basename: &str,
) -> RepoOutcome {
    let in_workspace = workspace_path.join(crate::alert_state::LEGACY_ALERT_STATE_FILE);
    if !in_workspace.exists() {
        return RepoOutcome::NoOp;
    }
    let in_state_dir = paths.alert_state_path(basename);

    let tracked = is_tracked_in_git(workspace_path);

    if in_state_dir.exists() {
        // State-dir wins; remove the workspace copy.
        if let Err(e) = std::fs::remove_file(&in_workspace) {
            return RepoOutcome::Failed(format!(
                "removing workspace file {} after preferring state-dir copy: {e}",
                in_workspace.display()
            ));
        }
        tracing::info!(
            url = %repo.url,
            workspace = %workspace_path.display(),
            state_dir_path = %in_state_dir.display(),
            "alert-state migration: state-dir version present; deleted workspace copy"
        );
        if tracked {
            if let Err(e) = git_rm_cached_commit_and_push(repo, github_cfg, workspace_path) {
                return RepoOutcome::Failed(format!(
                    "git rm --cached / commit / push for tracked .alert-state.json: {e:#}"
                ));
            }
            return RepoOutcome::UntrackedAndPushed;
        }
        return RepoOutcome::PreferredStateDir;
    }

    // Only the workspace version exists. Move it.
    if let Err(e) = std::fs::create_dir_all(paths.alert_state_dir()) {
        return RepoOutcome::Failed(format!(
            "creating alert-state dir {}: {e}",
            paths.alert_state_dir().display()
        ));
    }
    match std::fs::rename(&in_workspace, &in_state_dir) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            if let Err(e2) = copy_then_remove_file(&in_workspace, &in_state_dir) {
                return RepoOutcome::Failed(format!(
                    "cross-partition copy+delete {} → {}: {e2:#}",
                    in_workspace.display(),
                    in_state_dir.display()
                ));
            }
        }
        Err(e) => {
            return RepoOutcome::Failed(format!(
                "rename {} → {}: {e}",
                in_workspace.display(),
                in_state_dir.display()
            ));
        }
    }
    tracing::info!(
        url = %repo.url,
        workspace = %workspace_path.display(),
        from = %in_workspace.display(),
        to = %in_state_dir.display(),
        "alert-state migration: moved workspace file to state-dir"
    );

    if tracked {
        if let Err(e) = git_rm_cached_commit_and_push(repo, github_cfg, workspace_path) {
            return RepoOutcome::Failed(format!(
                "git rm --cached / commit / push for tracked .alert-state.json: {e:#}"
            ));
        }
        return RepoOutcome::UntrackedAndPushed;
    }
    RepoOutcome::Migrated
}

fn is_tracked_in_git(workspace: &Path) -> bool {
    // `git ls-files --error-unmatch <path>` exits 0 if tracked, non-zero
    // otherwise. The check is workspace-local; a missing `.git` directory
    // is treated as not-tracked.
    if !workspace.join(".git").exists() {
        return false;
    }
    let output = Command::new("git")
        .args([
            "ls-files",
            "--error-unmatch",
            "--",
            crate::alert_state::LEGACY_ALERT_STATE_FILE,
        ])
        .current_dir(workspace)
        .output();
    match output {
        Ok(o) => o.status.success(),
        Err(_) => false,
    }
}

fn copy_then_remove_file(src: &Path, dst: &Path) -> Result<()> {
    std::fs::copy(src, dst)
        .with_context(|| format!("copy {} → {}", src.display(), dst.display()))?;
    std::fs::remove_file(src).with_context(|| format!("remove {}", src.display()))?;
    Ok(())
}

/// Untrack the workspace's `.alert-state.json` from git: `git rm --cached`,
/// commit with the pinned subject, push to the base branch. Uses the same
/// token + push path as normal autocoder pushes.
fn git_rm_cached_commit_and_push(
    repo: &RepositoryConfig,
    _github_cfg: &GithubConfig,
    workspace: &Path,
) -> Result<()> {
    // `git rm --cached -- .alert-state.json`
    let rm = Command::new("git")
        .args([
            "rm",
            "--cached",
            "--",
            crate::alert_state::LEGACY_ALERT_STATE_FILE,
        ])
        .current_dir(workspace)
        .output()
        .context("spawning `git rm --cached .alert-state.json`")?;
    if !rm.status.success() {
        let stderr = String::from_utf8_lossy(&rm.stderr);
        return Err(anyhow::anyhow!(
            "git rm --cached .alert-state.json failed: {}",
            stderr.trim()
        ));
    }

    // `git commit -m '<subject>'`
    let commit = Command::new("git")
        .args(["commit", "-m", REMOVAL_COMMIT_SUBJECT])
        .current_dir(workspace)
        .output()
        .context("spawning `git commit`")?;
    if !commit.status.success() {
        let stderr = String::from_utf8_lossy(&commit.stderr);
        let stdout = String::from_utf8_lossy(&commit.stdout);
        return Err(anyhow::anyhow!(
            "git commit for alert-state removal failed: stderr=`{}` stdout=`{}`",
            stderr.trim(),
            stdout.trim()
        ));
    }

    // `git push origin <base_branch>`. The migration pushes to origin
    // because the removal is a base-branch cleanup, not an agent-branch
    // commit. Branch protection rejecting this push surfaces as a
    // non-zero exit; the caller treats that as a per-repo failure and
    // logs the operator-action.
    let push = Command::new("git")
        .args(["push", "origin", &repo.base_branch])
        .current_dir(workspace)
        .output()
        .context("spawning `git push origin <base_branch>`")?;
    if !push.status.success() {
        let stderr = String::from_utf8_lossy(&push.stderr);
        return Err(anyhow::anyhow!(
            "git push origin {} failed: {}",
            repo.base_branch,
            stderr.trim()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RepositoryConfig;
    use crate::paths::DaemonPaths;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fixture_paths(tempdir: &TempDir) -> DaemonPaths {
        let paths = DaemonPaths::under_root(tempdir.path());
        for d in [&paths.state, &paths.cache, &paths.logs, &paths.runtime] {
            std::fs::create_dir_all(d).unwrap();
        }
        paths
    }

    fn fixture_repo(workspace: PathBuf, url: &str) -> RepositoryConfig {
        RepositoryConfig {
            url: url.into(),
            base_branch: "main".into(),
            agent_branch: "agent-q".into(),
            poll_interval_sec: 60,
            chatops_channel_id: None,
            local_path: Some(workspace),
            max_changes_per_pr: None,
            audits: None,
            spec_storage: None,
            upstream: None,
            auto_submit_pr: true,
        }
    }

    fn fixture_github_cfg() -> GithubConfig {
        GithubConfig {
            token_env: "GITHUB_TOKEN".into(),
            token: None,
            owner_tokens: None,
            fork_owner: None,
            recreate_fork_on_reinit: false,
        }
    }

    #[test]
    fn workspace_file_moves_to_state_dir_and_marker_written() {
        let tempdir = TempDir::new().unwrap();
        let paths = fixture_paths(&tempdir);
        let workspace = paths.cache.join("workspaces/github_com_owner_repo");
        std::fs::create_dir_all(&workspace).unwrap();
        let in_workspace =
            workspace.join(crate::alert_state::LEGACY_ALERT_STATE_FILE);
        std::fs::write(&in_workspace, br#"{"alerts":{}}"#).unwrap();
        let repo = fixture_repo(workspace.clone(), "git@github.com:owner/repo.git");

        migrate_alert_state_from_workspace(&paths, &[repo], &fixture_github_cfg())
            .unwrap();

        let target = paths.alert_state_path("github_com_owner_repo");
        assert!(target.exists(), "state-dir file must exist at {}", target.display());
        assert!(
            !in_workspace.exists(),
            "workspace file must be removed after migration"
        );
        assert!(
            paths.alert_state_dir().join(MIGRATION_MARKER).exists(),
            "marker must be written after a clean pass"
        );
    }

    #[test]
    fn both_files_exist_prefers_state_dir_and_removes_workspace_copy() {
        let tempdir = TempDir::new().unwrap();
        let paths = fixture_paths(&tempdir);
        let workspace = paths.cache.join("workspaces/github_com_owner_repo");
        std::fs::create_dir_all(&workspace).unwrap();
        let in_workspace =
            workspace.join(crate::alert_state::LEGACY_ALERT_STATE_FILE);
        std::fs::write(&in_workspace, br#"{"alerts":{"workspace_copy":1}}"#).unwrap();
        std::fs::create_dir_all(paths.alert_state_dir()).unwrap();
        let in_state_dir = paths.alert_state_path("github_com_owner_repo");
        std::fs::write(&in_state_dir, br#"{"alerts":{"state_dir_copy":1}}"#).unwrap();
        let repo = fixture_repo(workspace.clone(), "git@github.com:owner/repo.git");

        migrate_alert_state_from_workspace(&paths, &[repo], &fixture_github_cfg())
            .unwrap();

        // Workspace copy gone, state-dir copy untouched.
        assert!(!in_workspace.exists());
        let kept = std::fs::read_to_string(&in_state_dir).unwrap();
        assert!(
            kept.contains("state_dir_copy"),
            "state-dir version must be preserved unchanged; got: {kept}"
        );
        assert!(paths.alert_state_dir().join(MIGRATION_MARKER).exists());
    }

    #[test]
    fn no_workspace_files_is_noop_and_marker_written() {
        let tempdir = TempDir::new().unwrap();
        let paths = fixture_paths(&tempdir);
        let workspace = paths.cache.join("workspaces/github_com_owner_repo");
        std::fs::create_dir_all(&workspace).unwrap();
        let repo = fixture_repo(workspace, "git@github.com:owner/repo.git");

        migrate_alert_state_from_workspace(&paths, &[repo], &fixture_github_cfg())
            .unwrap();

        assert!(
            paths.alert_state_dir().join(MIGRATION_MARKER).exists(),
            "no-op scan must still write the marker"
        );
        assert!(
            !paths
                .alert_state_path("github_com_owner_repo")
                .exists(),
            "no state file is created when no migration was needed"
        );
    }

    #[test]
    fn marker_present_short_circuits_the_scan() {
        let tempdir = TempDir::new().unwrap();
        let paths = fixture_paths(&tempdir);
        let workspace = paths.cache.join("workspaces/github_com_owner_repo");
        std::fs::create_dir_all(&workspace).unwrap();
        let in_workspace =
            workspace.join(crate::alert_state::LEGACY_ALERT_STATE_FILE);
        std::fs::write(&in_workspace, br#"{"alerts":{}}"#).unwrap();
        std::fs::create_dir_all(paths.alert_state_dir()).unwrap();
        std::fs::write(
            paths.alert_state_dir().join(MIGRATION_MARKER),
            "ok\n",
        )
        .unwrap();
        let repo = fixture_repo(workspace.clone(), "git@github.com:owner/repo.git");

        migrate_alert_state_from_workspace(&paths, &[repo], &fixture_github_cfg())
            .unwrap();

        // Marker present + idempotent: the workspace file was NOT moved.
        assert!(in_workspace.exists(), "marker short-circuits the scan");
    }

    #[test]
    fn empty_repo_list_writes_marker() {
        let tempdir = TempDir::new().unwrap();
        let paths = fixture_paths(&tempdir);
        migrate_alert_state_from_workspace(&paths, &[], &fixture_github_cfg()).unwrap();
        assert!(paths.alert_state_dir().join(MIGRATION_MARKER).exists());
    }
}
