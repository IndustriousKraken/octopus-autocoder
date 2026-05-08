//! `orchestrator run` — daemon entry point. Spawns one polling task per
//! configured repository and waits for shutdown signal (SIGINT/SIGTERM) or
//! all tasks to finish.

use crate::config::{Config, ExecutorKind, RepositoryConfig};
use crate::executor::{Executor, claude_cli::ClaudeCliExecutor};
use crate::{git, polling_loop, workspace};
use anyhow::Result;
use std::sync::Arc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub async fn execute(cfg: Config) -> Result<()> {
    workspace::detect_collisions(&cfg.repositories)?;

    let executor: Arc<dyn Executor> = match cfg.executor.kind {
        ExecutorKind::ClaudeCli => Arc::new(ClaudeCliExecutor::new(
            cfg.executor.command.clone(),
            cfg.executor.timeout_secs,
        )),
    };

    for repo in &cfg.repositories {
        let derived = workspace::resolve_path(repo);
        tracing::info!(
            url = repo.url.as_str(),
            workspace = %derived.display(),
            poll_interval_sec = repo.poll_interval_sec,
            "configured repository"
        );
    }

    let cancel = CancellationToken::new();

    let mut tasks: JoinSet<()> = JoinSet::new();
    for repo in cfg.repositories.iter().cloned() {
        if !repo_passes_startup_check(&repo) {
            // Per orchestrator-cli baseline: a repo dirty at startup is
            // skipped for the remainder of the process lifetime. Other
            // configured repositories continue to be serviced.
            continue;
        }
        let executor = executor.clone();
        let github = cfg.github.clone();
        let cancel = cancel.clone();
        tasks.spawn(async move { polling_loop::run(repo, executor, github, cancel).await });
    }

    spawn_signal_handler(cancel.clone());

    while let Some(joined) = tasks.join_next().await {
        if let Err(e) = joined {
            tracing::error!("polling task panicked: {e}");
        }
    }

    tracing::info!("shutdown complete");
    Ok(())
}

/// Initialize the workspace and check for a dirty working tree. Returns
/// `true` if the repository is healthy and a polling task should be spawned;
/// `false` (with a logged error) if the workspace is dirty or cannot be
/// initialized.
pub fn repo_passes_startup_check(repo: &RepositoryConfig) -> bool {
    let workspace_path = workspace::resolve_path(repo);
    if let Err(e) = workspace::ensure_initialized(&workspace_path, &repo.url) {
        tracing::error!(
            url = repo.url.as_str(),
            workspace = %workspace_path.display(),
            "workspace initialization failed; this repository is skipped for the process lifetime: {e:#}"
        );
        return false;
    }
    match git::status_porcelain(&workspace_path) {
        Ok(s) if s.is_empty() => true,
        Ok(dirty) => {
            let dirty_count = dirty.lines().count();
            tracing::error!(
                url = repo.url.as_str(),
                workspace = %workspace_path.display(),
                "workspace is dirty at startup ({dirty_count} entries from `git status --porcelain`); skipping this repository for the process lifetime"
            );
            false
        }
        Err(e) => {
            tracing::error!(
                url = repo.url.as_str(),
                "could not run git status on workspace: {e:#}; skipping this repository for the process lifetime"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn run_git(path: &Path, args: &[&str]) {
        let st = Command::new("git").args(args).current_dir(path).status().unwrap();
        assert!(st.success(), "git {args:?} failed");
    }

    /// Build a remote + workspace clone pair. The workspace has `origin`
    /// pointing at the remote, so `git fetch` succeeds during the startup
    /// check.
    fn workspace_pair() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let remote = dir.path().join("remote");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&remote).unwrap();
        run_git(&remote, &["init", "-q", "-b", "main"]);
        run_git(&remote, &["config", "user.email", "test@example.com"]);
        run_git(&remote, &["config", "user.name", "test"]);
        std::fs::write(remote.join("README.md"), "x").unwrap();
        run_git(&remote, &["add", "README.md"]);
        run_git(&remote, &["commit", "-q", "-m", "initial"]);

        let parent = workspace.parent().unwrap();
        let st = Command::new("git")
            .args(["clone", "-q", remote.to_string_lossy().as_ref(),
                   workspace.to_string_lossy().as_ref()])
            .current_dir(parent)
            .status()
            .unwrap();
        assert!(st.success(), "clone failed");
        run_git(&workspace, &["config", "user.email", "test@example.com"]);
        run_git(&workspace, &["config", "user.name", "test"]);
        (dir, workspace)
    }

    fn dirty_workspace_fixture() -> (TempDir, PathBuf) {
        let (dir, path) = workspace_pair();
        // Untracked file → status --porcelain non-empty → dirty.
        std::fs::write(path.join("LEFTOVER.txt"), "stale\n").unwrap();
        (dir, path)
    }

    fn clean_workspace_fixture() -> (TempDir, PathBuf) {
        workspace_pair()
    }

    fn cfg_with(local: PathBuf) -> RepositoryConfig {
        RepositoryConfig {
            url: format!("git@github.com:fixture/{}.git", local.file_name().unwrap().to_string_lossy()),
            local_path: Some(local),
            base_branch: "main".into(),
            agent_branch: "agent-q".into(),
            poll_interval_sec: 60,
        }
    }

    /// 13.1.3 / orchestrator-cli baseline: a workspace dirty at startup
    /// causes that repository to be skipped for the process lifetime.
    /// Other configured repositories continue to be serviced.
    #[test]
    fn dirty_workspace_skipped_at_startup() {
        let (_dirty, dirty_path) = dirty_workspace_fixture();
        let (_clean, clean_path) = clean_workspace_fixture();

        let dirty_repo = cfg_with(dirty_path);
        let clean_repo = cfg_with(clean_path);

        // Dirty repo fails the startup check; clean repo passes.
        assert!(!repo_passes_startup_check(&dirty_repo),
            "dirty workspace must fail startup check");
        assert!(repo_passes_startup_check(&clean_repo),
            "clean workspace must pass startup check");
    }
}

fn spawn_signal_handler(cancel: CancellationToken) {
    tokio::spawn(async move {
        let ctrl_c = async {
            let _ = tokio::signal::ctrl_c().await;
        };

        #[cfg(unix)]
        let terminate = async {
            use tokio::signal::unix::{SignalKind, signal};
            match signal(SignalKind::terminate()) {
                Ok(mut sig) => {
                    sig.recv().await;
                }
                Err(e) => {
                    tracing::warn!("could not install SIGTERM handler: {e}");
                    std::future::pending::<()>().await;
                }
            }
        };
        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            () = ctrl_c => tracing::info!("received SIGINT; shutting down"),
            () = terminate => tracing::info!("received SIGTERM; shutting down"),
        }
        cancel.cancel();
    });
}
