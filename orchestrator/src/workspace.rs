//! Per-repository workspace management: deterministic path derivation,
//! idempotent clone-or-fetch, and startup-time collision detection.

use crate::{config::RepositoryConfig, git};
use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const WORKSPACE_ROOT: &str = "/tmp/workspaces";

/// Derive a per-repo workspace path under `/tmp/workspaces/`. Deterministic:
/// the same URL always produces the same path. SSH and HTTPS forms of the
/// same repository collapse to the same derived path.
pub fn derive_path(url: &str) -> PathBuf {
    PathBuf::from(WORKSPACE_ROOT).join(sanitize(url))
}

fn sanitize(url: &str) -> String {
    let stripped = url
        .strip_prefix("git@")
        .or_else(|| url.strip_prefix("ssh://"))
        .or_else(|| url.strip_prefix("https://"))
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let stripped = stripped.strip_suffix(".git").unwrap_or(stripped);
    stripped
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Resolve the workspace path for a repository: explicit `local_path` if set,
/// otherwise the derived path.
pub fn resolve_path(repo: &RepositoryConfig) -> PathBuf {
    repo.local_path
        .clone()
        .unwrap_or_else(|| derive_path(&repo.url))
}

/// Ensure the repository is locally cloned. If the path does not exist, run
/// `git clone`. If it exists and is a git repository, run `git fetch`. If it
/// exists but is not a git repo, return an error without modifying the path.
pub fn ensure_initialized(workspace: &Path, url: &str) -> Result<()> {
    if !workspace.exists() {
        git::clone(workspace, url)
            .with_context(|| format!("cloning {url} into {}", workspace.display()))?;
        return Ok(());
    }
    if !workspace.join(".git").is_dir() {
        return Err(anyhow!(
            "workspace path exists but is not a git repository (no .git directory): {}",
            workspace.display()
        ));
    }
    git::fetch(workspace)
        .with_context(|| format!("fetching origin in {}", workspace.display()))?;
    Ok(())
}

/// Detect any two configured repositories that resolve to the same workspace
/// path. Returns an error naming both URLs and the shared path when found.
pub fn detect_collisions(repos: &[RepositoryConfig]) -> Result<()> {
    let mut seen: HashMap<PathBuf, &str> = HashMap::new();
    for repo in repos {
        let path = resolve_path(repo);
        if let Some(prior_url) = seen.get(&path) {
            return Err(anyhow!(
                "workspace path collision: `{prior}` and `{current}` both resolve to {path}",
                prior = prior_url,
                current = repo.url,
                path = path.display(),
            ));
        }
        seen.insert(path, &repo.url);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn cfg(url: &str) -> RepositoryConfig {
        RepositoryConfig {
            url: url.to_string(),
            local_path: None,
            base_branch: "main".into(),
            agent_branch: "agent-q".into(),
            poll_interval_sec: 60,
            slack_channel_id: None,
        }
    }

    fn cfg_with_local(url: &str, local: &str) -> RepositoryConfig {
        RepositoryConfig {
            url: url.to_string(),
            local_path: Some(PathBuf::from(local)),
            base_branch: "main".into(),
            agent_branch: "agent-q".into(),
            poll_interval_sec: 60,
            slack_channel_id: None,
        }
    }

    #[test]
    fn derive_path_ssh_form() {
        let p = derive_path("git@github.com:owner/repo.git");
        assert_eq!(p, PathBuf::from("/tmp/workspaces/github_com_owner_repo"));
    }

    #[test]
    fn derive_path_https_form() {
        let p = derive_path("https://github.com/owner/repo.git");
        assert_eq!(p, PathBuf::from("/tmp/workspaces/github_com_owner_repo"));
    }

    #[test]
    fn derive_path_strips_git_suffix() {
        let with_git = derive_path("git@github.com:owner/repo.git");
        let without = derive_path("git@github.com:owner/repo");
        assert_eq!(with_git, without);
    }

    #[test]
    fn derive_path_distinct_for_different_repos() {
        let a = derive_path("git@github.com:owner/repo-a.git");
        let b = derive_path("git@github.com:owner/repo-b.git");
        assert_ne!(a, b);
    }

    #[test]
    fn derive_path_is_stable() {
        let url = "git@github.com:owner/repo.git";
        assert_eq!(derive_path(url), derive_path(url));
    }

    #[test]
    fn collision_detected() {
        let repos = vec![
            cfg("git@github.com:owner/repo.git"),
            cfg("https://github.com/owner/repo.git"),
        ];
        let err = detect_collisions(&repos).expect_err("should detect collision");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("git@github.com:owner/repo.git"),
            "first url should be in error: {msg}"
        );
        assert!(
            msg.contains("https://github.com/owner/repo.git"),
            "second url should be in error: {msg}"
        );
    }

    #[test]
    fn collision_detected_via_explicit_local_path() {
        let repos = vec![
            cfg_with_local("git@github.com:owner/a.git", "/tmp/workspaces/shared"),
            cfg_with_local("git@github.com:owner/b.git", "/tmp/workspaces/shared"),
        ];
        let err = detect_collisions(&repos).expect_err("should detect explicit collision");
        let msg = format!("{err:#}");
        assert!(msg.contains("/tmp/workspaces/shared"), "got: {msg}");
    }

    #[test]
    fn no_collisions_when_distinct() {
        let repos = vec![
            cfg("git@github.com:owner/a.git"),
            cfg("git@github.com:owner/b.git"),
        ];
        detect_collisions(&repos).expect("distinct repos should pass");
    }

    fn run_git(path: &Path, args: &[&str]) {
        let status = Command::new("git").args(args).current_dir(path).status().unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    /// Create a regular fixture repo with one commit at `path`. Suitable as
    /// the "remote" target for a clone.
    fn make_fixture_remote(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        run_git(path, &["init", "-q", "-b", "main"]);
        run_git(path, &["config", "user.email", "test@example.com"]);
        run_git(path, &["config", "user.name", "test"]);
        std::fs::write(path.join("README.md"), "hi\n").unwrap();
        run_git(path, &["add", "README.md"]);
        run_git(path, &["commit", "-q", "-m", "initial"]);
    }

    #[test]
    fn ensure_initialized_clones_when_absent() {
        let dir = TempDir::new().unwrap();
        let remote = dir.path().join("remote");
        let workspace = dir.path().join("local");
        make_fixture_remote(&remote);
        let url = remote.to_string_lossy().to_string();
        ensure_initialized(&workspace, &url).unwrap();
        assert!(workspace.join(".git").is_dir());
        assert!(workspace.join("README.md").is_file());
    }

    #[test]
    fn ensure_initialized_fetches_when_present() {
        let dir = TempDir::new().unwrap();
        let remote = dir.path().join("remote");
        let workspace = dir.path().join("local");
        make_fixture_remote(&remote);
        let url = remote.to_string_lossy().to_string();
        ensure_initialized(&workspace, &url).unwrap();
        // Make a local branch in the workspace; we'll verify it survives a fetch.
        run_git(&workspace, &["branch", "local-only-branch"]);
        // Second call should fetch (not re-clone) and preserve local branches.
        ensure_initialized(&workspace, &url).unwrap();
        let output = Command::new("git")
            .args(["branch", "--list", "local-only-branch"])
            .current_dir(&workspace)
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("local-only-branch"),
            "local branch should survive ensure_initialized re-entry"
        );
    }

    #[test]
    fn ensure_initialized_errors_on_non_git_directory() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("not-a-repo");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("hello.txt"), "x").unwrap();
        let err = ensure_initialized(&workspace, "irrelevant-url")
            .expect_err("should error when path is not a git repo");
        let msg = format!("{err:#}");
        assert!(msg.contains(".git"), "error should mention missing .git: {msg}");
    }
}
