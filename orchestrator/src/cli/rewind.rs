//! `orchestrator rewind` — recover from a failed PR or bad implementation by
//! unarchiving named changes and resetting the agent branch to base.

use crate::config::{Config, RepositoryConfig};
use crate::{git, queue, workspace};
use anyhow::{Result, anyhow};
use std::io::{BufRead, Write};
use std::path::Path;

/// Single-repo rewind. The multi-repo `--repo` selector is added in the
/// `rewind-and-recovery` change. If the config has more than one repo, log
/// a warning and operate on the first.
pub async fn execute(cfg: Config, changes: Vec<String>, hard: bool) -> Result<()> {
    if cfg.repositories.is_empty() {
        return Err(anyhow!("no repositories configured"));
    }
    if cfg.repositories.len() > 1 {
        tracing::warn!(
            "multi-repo config ({} entries); rewind operates on the first repo only until `rewind-and-recovery` lands",
            cfg.repositories.len()
        );
    }

    let repo = &cfg.repositories[0];
    let workspace_path = workspace::resolve_path(repo);
    workspace::ensure_initialized(&workspace_path, &repo.url)?;

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    rewind_with_io(
        repo,
        &workspace_path,
        &changes,
        hard,
        &mut stdin.lock(),
        &mut stdout.lock(),
    )
    .await
}

/// IO-injected core of `rewind::execute`. The interactive `execute` wraps
/// real stdin/stdout; tests pass in-memory cursors to verify the
/// confirmation behavior.
pub async fn rewind_with_io<R: BufRead, W: Write>(
    repo: &RepositoryConfig,
    workspace_path: &Path,
    changes: &[String],
    hard: bool,
    reader: &mut R,
    writer: &mut W,
) -> Result<()> {
    if !hard && !confirm(repo, reader, writer)? {
        tracing::info!("rewind cancelled by user");
        return Ok(());
    }

    if hard {
        // Branch deletion (local + remote) is the responsibility of the
        // forthcoming `rewind-and-recovery` change. Until those utilities
        // land, we log the intent so users running `--hard` know that they
        // still have to clean up the agent branch manually.
        tracing::warn!(
            "--hard requested but agent-branch deletion is implemented by the `rewind-and-recovery` change; you must currently delete `{}` (local + remote) by hand",
            repo.agent_branch
        );
    }

    for change in changes {
        queue::unarchive(workspace_path, change)?;
        tracing::info!("unarchived change `{change}`");
    }

    // Reset the agent branch to base per the spec: branch is recreated at
    // base, no merge of unfinished work.
    git::fetch(workspace_path)?;
    git::checkout(workspace_path, &repo.base_branch)?;
    git::pull_ff_only(workspace_path, &repo.base_branch)?;
    git::recreate_branch(workspace_path, &repo.agent_branch)?;
    tracing::info!(
        "agent branch `{}` reset to `{}`",
        repo.agent_branch,
        repo.base_branch
    );

    Ok(())
}

fn confirm<R: BufRead, W: Write>(
    repo: &RepositoryConfig,
    reader: &mut R,
    writer: &mut W,
) -> Result<bool> {
    write!(
        writer,
        "Reset agent branch `{}` and unarchive changes for {}? [y/N] ",
        repo.agent_branch, repo.url
    )?;
    writer.flush()?;
    let mut buf = String::new();
    reader.read_line(&mut buf)?;
    let response = buf.trim();
    Ok(response == "y" || response == "Y")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;
    use tempfile::TempDir;

    fn run_git(path: &Path, args: &[&str]) {
        let st = Command::new("git").args(args).current_dir(path).status().unwrap();
        assert!(st.success(), "git {args:?} failed");
    }

    /// Build a workspace with `main` and `agent-q` branches, and one
    /// archived change directory at `openspec/changes/archive/<date>-<name>/`.
    fn rewind_fixture(change_name: &str) -> (TempDir, PathBuf) {
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
            .args([
                "clone", "-q",
                remote.to_string_lossy().as_ref(),
                workspace.to_string_lossy().as_ref(),
            ])
            .current_dir(parent)
            .status()
            .unwrap();
        assert!(st.success());
        run_git(&workspace, &["config", "user.email", "test@example.com"]);
        run_git(&workspace, &["config", "user.name", "test"]);

        // Place an archived change with a date prefix that the unarchive
        // regex will match.
        let archived_dir = workspace
            .join("openspec/changes/archive")
            .join(format!("2026-01-01-{change_name}"));
        std::fs::create_dir_all(&archived_dir).unwrap();
        std::fs::write(archived_dir.join("proposal.md"), "## Why\nfixture\n").unwrap();
        std::fs::write(archived_dir.join("tasks.md"), "- [ ] x\n").unwrap();

        // Make a divergent agent-q so the test can confirm the branch reset.
        run_git(&workspace, &["checkout", "-q", "-B", "agent-q"]);
        std::fs::write(workspace.join("DRIFT.md"), "drift\n").unwrap();
        run_git(&workspace, &["add", "DRIFT.md"]);
        run_git(&workspace, &["commit", "-q", "-m", "agent-q drift"]);
        // Return to main so the workspace is in a clean state on entry.
        run_git(&workspace, &["checkout", "-q", "main"]);

        (dir, workspace)
    }

    fn cfg_for(workspace: &Path) -> RepositoryConfig {
        RepositoryConfig {
            url: "git@github.com:fixture/repo.git".into(),
            local_path: Some(workspace.to_path_buf()),
            base_branch: "main".into(),
            agent_branch: "agent-q".into(),
            poll_interval_sec: 60,
        }
    }

    /// 13.1.4 / orchestrator-cli baseline: rewind locates the most recent
    /// matching archived directory, moves it back, AND resets the agent
    /// branch to base.
    #[tokio::test]
    async fn hard_rewind_unarchives_and_resets_agent_branch() {
        let (_dir, ws) = rewind_fixture("feature-a");
        let repo = cfg_for(&ws);

        let main_sha = git::rev_parse(&ws, "main").unwrap();
        let agent_pre_sha = git::rev_parse(&ws, "agent-q").unwrap();
        assert_ne!(main_sha, agent_pre_sha, "agent-q should be ahead before rewind");

        let mut input = std::io::Cursor::new(Vec::<u8>::new());
        let mut output = Vec::<u8>::new();
        rewind_with_io(
            &repo,
            &ws,
            &["feature-a".to_string()],
            true, // --hard, skips the confirmation prompt
            &mut input,
            &mut output,
        )
        .await
        .expect("rewind succeeds");

        // Active queue contains feature-a; archive entry is gone.
        assert!(ws.join("openspec/changes/feature-a/proposal.md").is_file());
        assert!(!ws
            .join("openspec/changes/archive/2026-01-01-feature-a")
            .exists());

        // agent-q was reset to main's HEAD.
        let agent_post_sha = git::rev_parse(&ws, "agent-q").unwrap();
        assert_eq!(agent_post_sha, main_sha,
            "agent-q must equal main after rewind");
    }

    /// 13.1.4 second half: missing archived change errors.
    #[tokio::test]
    async fn rewind_missing_change_errors() {
        let (_dir, ws) = rewind_fixture("present-change");
        let repo = cfg_for(&ws);
        let mut input = std::io::Cursor::new(Vec::<u8>::new());
        let mut output = Vec::<u8>::new();
        let err = rewind_with_io(
            &repo,
            &ws,
            &["never-existed".to_string()],
            true,
            &mut input,
            &mut output,
        )
        .await
        .expect_err("missing change should error");
        let msg = format!("{err:#}");
        assert!(msg.contains("never-existed"), "error should name change: {msg}");
    }

    /// 13.1.5 / orchestrator-cli baseline: soft rewind WITHOUT `--hard`
    /// prompts on stdin and exits without state change on a non-y answer.
    #[tokio::test]
    async fn soft_rewind_declines_on_n() {
        let (_dir, ws) = rewind_fixture("feature-b");
        let repo = cfg_for(&ws);

        let agent_pre_sha = git::rev_parse(&ws, "agent-q").unwrap();

        let mut input = std::io::Cursor::new(b"n\n".to_vec());
        let mut output = Vec::<u8>::new();
        rewind_with_io(
            &repo,
            &ws,
            &["feature-b".to_string()],
            false, // soft rewind
            &mut input,
            &mut output,
        )
        .await
        .expect("declined rewind returns Ok(())");

        // Prompt was emitted to the writer.
        let prompt = String::from_utf8(output).unwrap();
        assert!(prompt.contains("Reset agent branch"), "prompt should be shown: {prompt}");

        // No state change: archived entry still in archive, agent-q unchanged.
        assert!(ws
            .join("openspec/changes/archive/2026-01-01-feature-b")
            .exists());
        assert!(!ws.join("openspec/changes/feature-b").exists());
        let agent_post_sha = git::rev_parse(&ws, "agent-q").unwrap();
        assert_eq!(agent_post_sha, agent_pre_sha,
            "agent-q must be unchanged when user declines");
    }

    /// 13.1.5: confirming with `y` proceeds with the rewind.
    #[tokio::test]
    async fn soft_rewind_proceeds_on_y() {
        let (_dir, ws) = rewind_fixture("feature-c");
        let repo = cfg_for(&ws);

        let main_sha = git::rev_parse(&ws, "main").unwrap();

        let mut input = std::io::Cursor::new(b"y\n".to_vec());
        let mut output = Vec::<u8>::new();
        rewind_with_io(
            &repo,
            &ws,
            &["feature-c".to_string()],
            false,
            &mut input,
            &mut output,
        )
        .await
        .expect("rewind succeeds");

        assert!(ws.join("openspec/changes/feature-c").exists());
        assert_eq!(git::rev_parse(&ws, "agent-q").unwrap(), main_sha);
    }
}
