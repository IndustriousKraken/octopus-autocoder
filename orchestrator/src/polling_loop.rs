//! Per-repository polling loop. Each iteration runs a single pass: branch
//! init → queue walk → push + PR if commits were produced. Failures inside
//! one iteration are logged and the loop continues to the next sleep.

use crate::config::{GithubConfig, RepositoryConfig};
use crate::executor::{Executor, ExecutorOutcome};
use crate::{git, github, queue, workspace};
use anyhow::{Context, Result, anyhow};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

/// Run the polling loop for a single repository. Each iteration is wrapped in
/// `execute_one_pass`; failures inside a pass are logged and do not break the
/// loop. Cancellation is checked between iterations and during the sleep.
pub async fn run(
    repo: RepositoryConfig,
    executor: Arc<dyn Executor>,
    github: GithubConfig,
    cancel: CancellationToken,
) {
    let workspace = workspace::resolve_path(&repo);
    tracing::info!(
        url = repo.url.as_str(),
        workspace = %workspace.display(),
        poll_interval_sec = repo.poll_interval_sec,
        "starting polling loop"
    );

    loop {
        if cancel.is_cancelled() {
            break;
        }

        if let Err(error) = execute_one_pass(&workspace, &repo, executor.as_ref(), &github).await {
            tracing::error!(
                url = repo.url.as_str(),
                "polling iteration failed for {}: {error:#}",
                repo.url
            );
        }

        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            () = sleep(Duration::from_secs(repo.poll_interval_sec)) => {}
        }
    }

    tracing::info!(url = repo.url.as_str(), "polling loop exiting");
}

/// Single-pass workflow: workspace init → stale-lock cleanup → dirty-workspace
/// check → branch recreation → queue walk → push + PR if commits were
/// produced.
pub async fn execute_one_pass(
    workspace: &Path,
    repo: &RepositoryConfig,
    executor: &dyn Executor,
    github_cfg: &GithubConfig,
) -> Result<()> {
    let processed = run_pass_through_commits(workspace, repo, executor).await?;
    if processed.is_empty() {
        return Ok(());
    }

    let range = format!("{}..{}", repo.base_branch, repo.agent_branch);
    let commit_count = git::rev_list_count(workspace, &range)?;
    if commit_count == 0 {
        tracing::info!(
            url = repo.url.as_str(),
            "polling pass produced no commits (all completed changes had empty diffs)"
        );
        return Ok(());
    }

    git::push_force_with_lease(workspace, &repo.agent_branch)?;
    open_pull_request(repo, github_cfg, &processed).await?;
    Ok(())
}

/// Run a polling pass up to and including any commits, but stop before push
/// and PR creation. Returns the names of changes archived during the pass.
/// The caller (production: `execute_one_pass`) is responsible for the
/// remote-side work; tests use this directly to verify commit-formation
/// behavior without needing a live GitHub endpoint or a writable remote.
pub async fn run_pass_through_commits(
    workspace: &Path,
    repo: &RepositoryConfig,
    executor: &dyn Executor,
) -> Result<Vec<String>> {
    workspace::ensure_initialized(workspace, &repo.url)?;
    let _cleared = queue::clear_stale_locks(workspace)?;

    let dirty = git::status_porcelain(workspace)?;
    if !dirty.is_empty() {
        return Err(anyhow!(
            "workspace {} is dirty before pass; refusing to proceed:\n{dirty}",
            workspace.display()
        ));
    }

    git::fetch(workspace)?;
    git::checkout(workspace, &repo.base_branch)?;
    git::pull_ff_only(workspace, &repo.base_branch)?;
    git::recreate_branch(workspace, &repo.agent_branch)?;

    let processed = walk_queue(workspace, repo, executor).await?;

    if processed.is_empty() {
        tracing::info!(url = repo.url.as_str(), "polling pass produced no changes");
    }
    Ok(processed)
}

/// Iterate the pending queue, invoking the executor for each ready change.
/// Returns the names of changes that were archived (i.e. those for which the
/// executor returned `Completed`, regardless of diff). On `AskUser` we log
/// and exit early per the architecture spec.
async fn walk_queue(
    workspace: &Path,
    repo: &RepositoryConfig,
    executor: &dyn Executor,
) -> Result<Vec<String>> {
    let pending = queue::list_pending(workspace)?;
    let mut archived: Vec<String> = Vec::new();

    for change in pending {
        queue::lock(workspace, &change)
            .with_context(|| format!("locking change `{change}`"))?;

        let outcome = executor.run(workspace, &change).await;
        let result = handle_outcome(workspace, &change, outcome).await;
        // Always unlock, even after a Completed → archive (archive moved the
        // dir, so the lock is gone, but `queue::unlock` is idempotent).
        let _ = queue::unlock(workspace, &change);

        match result {
            Ok(QueueStep::Archived) => archived.push(change),
            Ok(QueueStep::Failed) => {} // logged inside; continue to next
            Ok(QueueStep::AskUserExitEarly) => {
                tracing::error!(
                    url = repo.url.as_str(),
                    "executor returned AskUser for `{change}`; ChatOps escalation is not yet implemented; exiting pass"
                );
                break;
            }
            Err(e) => {
                tracing::error!(
                    url = repo.url.as_str(),
                    "fatal error processing change `{change}`: {e:#}"
                );
                break;
            }
        }
    }

    Ok(archived)
}

enum QueueStep {
    Archived,
    Failed,
    AskUserExitEarly,
}

async fn handle_outcome(
    workspace: &Path,
    change: &str,
    outcome: Result<ExecutorOutcome>,
) -> Result<QueueStep> {
    match outcome {
        Err(e) => {
            tracing::error!("executor errored on `{change}`: {e:#}");
            Ok(QueueStep::Failed)
        }
        Ok(ExecutorOutcome::Failed { reason }) => {
            tracing::error!("executor reported Failed for `{change}`: {reason}");
            Ok(QueueStep::Failed)
        }
        Ok(ExecutorOutcome::AskUser { question, .. }) => {
            tracing::warn!("executor asked a question on `{change}`: {question}");
            Ok(QueueStep::AskUserExitEarly)
        }
        Ok(ExecutorOutcome::Completed) => {
            // Remove the `.in-progress` lock BEFORE inspecting the working
            // tree: the lock file is untracked and would otherwise show up
            // in `git status --porcelain`, contaminating the dirty check
            // and getting swept into the commit by `git add -A`.
            queue::unlock(workspace, change)?;
            let dirty = git::status_porcelain(workspace)?;
            if dirty.is_empty() {
                tracing::warn!(
                    "executor reported Completed for `{change}` but workspace is clean; archiving anyway per spec"
                );
            } else {
                let subject = build_commit_subject(workspace, change)?;
                git::add_all(workspace)?;
                git::commit(workspace, &subject)?;
            }
            queue::archive(workspace, change)?;
            Ok(QueueStep::Archived)
        }
    }
}

/// Build a commit subject from the change name and the first non-empty line of
/// the `## Why` section of `proposal.md`. Truncated to 72 characters total.
fn build_commit_subject(workspace: &Path, change: &str) -> Result<String> {
    let proposal = workspace
        .join("openspec/changes")
        .join(change)
        .join("proposal.md");
    let raw = std::fs::read_to_string(&proposal)
        .with_context(|| format!("reading proposal for commit subject: {}", proposal.display()))?;
    let why_summary = first_line_of_section(&raw, "## Why").unwrap_or_else(|| change.to_string());
    let mut subject = format!("{change}: {why_summary}");
    if subject.chars().count() > 72 {
        subject = subject.chars().take(72).collect();
    }
    Ok(subject)
}

/// Return the first non-empty line under the named markdown header. Returns
/// `None` if the header is absent or has no non-empty body line.
fn first_line_of_section(text: &str, header: &str) -> Option<String> {
    let mut in_section = false;
    for raw_line in text.lines() {
        let line = raw_line.trim_end();
        if line.trim_start().starts_with("## ") {
            in_section = line.trim_start() == header;
            continue;
        }
        if in_section {
            let stripped = line.trim();
            if !stripped.is_empty() {
                return Some(stripped.to_string());
            }
        }
    }
    None
}

async fn open_pull_request(
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    changes: &[String],
) -> Result<()> {
    let token = std::env::var(&github_cfg.token_env).map_err(|_| {
        anyhow!(
            "github token environment variable `{}` is not set",
            github_cfg.token_env
        )
    })?;
    let (owner, repo_name) = github::parse_repo_url(&repo.url)?;
    let title = format!("agent: {} change(s) in pass", changes.len());
    let body = build_pr_body(changes);

    let url = github::create_pull_request(
        &owner,
        &repo_name,
        &repo.agent_branch,
        &repo.base_branch,
        &title,
        &body,
        &token,
    )
    .await?;
    tracing::info!(url = repo.url.as_str(), pr = url.as_str(), "opened PR");
    Ok(())
}

fn build_pr_body(changes: &[String]) -> String {
    let mut s = String::from("Changes implemented in this pass:\n\n");
    for change in changes {
        s.push_str(&format!("- {change}\n"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_line_of_why_section() {
        let text = "## Why\nSwitch from sync to async\n\n## What Changes\n- thing\n";
        let line = first_line_of_section(text, "## Why").unwrap();
        assert_eq!(line, "Switch from sync to async");
    }

    #[test]
    fn first_line_of_why_skips_blank_lines() {
        let text = "## Why\n\n   \n  Real content here  \n## What Changes\n";
        let line = first_line_of_section(text, "## Why").unwrap();
        assert_eq!(line, "Real content here");
    }

    #[test]
    fn first_line_of_section_returns_none_when_missing() {
        let text = "## What Changes\n- x\n";
        assert!(first_line_of_section(text, "## Why").is_none());
    }

    #[test]
    fn build_commit_subject_truncates_to_72_chars() {
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path();
        let change = "make-the-thing-better";
        let proposal = ws.join("openspec/changes").join(change).join("proposal.md");
        std::fs::create_dir_all(proposal.parent().unwrap()).unwrap();
        let long = "A".repeat(200);
        std::fs::write(&proposal, format!("## Why\n{long}\n")).unwrap();
        let subject = build_commit_subject(ws, change).unwrap();
        assert_eq!(subject.chars().count(), 72);
        assert!(subject.starts_with("make-the-thing-better: "));
    }

    #[test]
    fn build_commit_subject_falls_back_to_change_name_when_no_why() {
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path();
        let proposal = ws.join("openspec/changes/c/proposal.md");
        std::fs::create_dir_all(proposal.parent().unwrap()).unwrap();
        std::fs::write(&proposal, "## What Changes\n- thing\n").unwrap();
        let subject = build_commit_subject(ws, "c").unwrap();
        assert_eq!(subject, "c: c");
    }

    /// Build a fixture remote repo with one commit on `main` AND a cloned
    /// workspace whose `origin` points to the remote. Returns the temp dir
    /// guard (drop = cleanup) plus the workspace path.
    fn fixture_workspace_with_remote() -> (tempfile::TempDir, std::path::PathBuf) {
        use std::process::Command;
        fn run(path: &Path, args: &[&str]) {
            let status = Command::new("git")
                .args(args)
                .current_dir(path)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed in {}", path.display());
        }

        let dir = tempfile::TempDir::new().unwrap();
        let remote = dir.path().join("remote");
        let workspace = dir.path().join("workspace");

        std::fs::create_dir_all(&remote).unwrap();
        run(&remote, &["init", "-q", "-b", "main"]);
        run(&remote, &["config", "user.email", "test@example.com"]);
        run(&remote, &["config", "user.name", "test"]);
        std::fs::write(remote.join("README.md"), "hi\n").unwrap();
        run(&remote, &["add", "README.md"]);
        run(&remote, &["commit", "-q", "-m", "initial"]);

        let remote_url = remote.to_string_lossy().to_string();
        let parent = workspace.parent().unwrap();
        let status = Command::new("git")
            .args([
                "clone",
                "-q",
                &remote_url,
                workspace.to_string_lossy().as_ref(),
            ])
            .current_dir(parent)
            .status()
            .unwrap();
        assert!(status.success(), "clone failed");
        run(&workspace, &["config", "user.email", "test@example.com"]);
        run(&workspace, &["config", "user.name", "test"]);
        (dir, workspace)
    }

    /// Add an OpenSpec change with a known `## Why` line to a fixture
    /// workspace and commit it locally so the working tree stays clean.
    fn add_committed_change(workspace: &Path, name: &str, why_line: &str) {
        let dir = workspace.join("openspec/changes").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("proposal.md"), format!("## Why\n{why_line}\n")).unwrap();
        std::fs::write(dir.join("tasks.md"), "- [ ] do thing\n").unwrap();
        let st = std::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(workspace)
            .status()
            .unwrap();
        assert!(st.success());
        let st = std::process::Command::new("git")
            .args(["commit", "-q", "-m", &format!("scaffold {name}")])
            .current_dir(workspace)
            .status()
            .unwrap();
        assert!(st.success());
    }

    /// Build a `RepositoryConfig` pointing at a fixture workspace. Uses a
    /// non-existent token env var so any attempt to open a PR errors fast
    /// rather than reaching a live API.
    fn fixture_repo(workspace: &Path) -> RepositoryConfig {
        RepositoryConfig {
            url: "git@github.com:owner/fixture.git".into(),
            local_path: Some(workspace.to_path_buf()),
            base_branch: "main".into(),
            agent_branch: "agent-q".into(),
            poll_interval_sec: 60,
        }
    }

    /// Executor that returns `Completed` and writes a file so
    /// `git status --porcelain` is non-empty and a real commit gets made.
    struct CompletingExecutorWithDiff {
        artifact_name: String,
        artifact_text: String,
    }
    #[async_trait::async_trait]
    impl Executor for CompletingExecutorWithDiff {
        async fn run(&self, workspace: &Path, _c: &str) -> Result<ExecutorOutcome> {
            std::fs::write(workspace.join(&self.artifact_name), &self.artifact_text)?;
            Ok(ExecutorOutcome::Completed)
        }
        async fn resume(
            &self,
            _h: crate::executor::ResumeHandle,
            _a: &str,
        ) -> Result<ExecutorOutcome> {
            unreachable!()
        }
    }

    /// Executor that returns `Completed` but writes nothing. Exercises the
    /// "Completed but no diff" architecture scenario.
    struct CompletingExecutorNoDiff;
    #[async_trait::async_trait]
    impl Executor for CompletingExecutorNoDiff {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            Ok(ExecutorOutcome::Completed)
        }
        async fn resume(
            &self,
            _h: crate::executor::ResumeHandle,
            _a: &str,
        ) -> Result<ExecutorOutcome> {
            unreachable!()
        }
    }

    /// Executor that always returns `Failed`. Exercises the "backend failure"
    /// architecture scenario.
    struct AlwaysFailingExecutor;
    #[async_trait::async_trait]
    impl Executor for AlwaysFailingExecutor {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            Ok(ExecutorOutcome::Failed {
                reason: "fixture failure".into(),
            })
        }
        async fn resume(
            &self,
            _h: crate::executor::ResumeHandle,
            _a: &str,
        ) -> Result<ExecutorOutcome> {
            unreachable!()
        }
    }

    /// Run a single pass through the commit step but skip push + PR. Tests
    /// only need this when they want to verify commit/archive behavior
    /// without an HTTP fixture for the GitHub API.
    async fn run_one_pass_no_push(
        workspace: &Path,
        executor: &dyn Executor,
    ) -> Result<Vec<String>> {
        let repo = fixture_repo(workspace);
        run_pass_through_commits(workspace, &repo, executor).await
    }

    /// 13.3.2 / executor baseline: when the executor returns `Failed`,
    /// the orchestrator unlocks the change AND does NOT archive it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_change_unlocks_and_does_not_archive() {
        let (_dir, ws) = fixture_workspace_with_remote();
        add_committed_change(&ws, "feature-a", "fixture reason");

        let executor = AlwaysFailingExecutor;
        let _ = run_one_pass_no_push(&ws, &executor).await; // Failed is a normal outcome

        // The change is still in the active queue (not archived).
        let pending = queue::list_pending(&ws).unwrap();
        assert_eq!(pending, vec!["feature-a".to_string()]);
        // No archive directory was created for it.
        let archive_root = ws.join("openspec/changes/archive");
        if archive_root.exists() {
            for entry in std::fs::read_dir(&archive_root).unwrap() {
                let name = entry.unwrap().file_name().into_string().unwrap();
                assert!(
                    !name.contains("feature-a"),
                    "Failed change must not be archived; found {name}"
                );
            }
        }
        // No `.in-progress` lock left behind.
        let lock = ws.join("openspec/changes/feature-a/.in-progress");
        assert!(!lock.exists(), "lock file should be cleared after Failed");
    }

    /// 13.4.1 / git-workflow-manager baseline: at start of each pass, the
    /// agent branch is recreated to match the post-pull HEAD of the base
    /// branch.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn branch_init_resets_agent_to_base() {
        let (_dir, ws) = fixture_workspace_with_remote();
        // Empty queue is fine — we only care about the branch init step.

        let executor = CompletingExecutorNoDiff;
        run_one_pass_no_push(&ws, &executor).await.expect("pass succeeds");

        // After init, agent-q must point at the same SHA as main.
        let main_sha = crate::git::rev_parse(&ws, "main").unwrap();
        let agent_sha = crate::git::rev_parse(&ws, "agent-q").unwrap();
        assert_eq!(
            main_sha, agent_sha,
            "agent-q must equal main after branch init step"
        );
    }

    /// 13.4.3 / git-workflow-manager baseline: commit subject is
    /// `<change>: <first non-empty line of ## Why>`, truncated to 72 chars.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn commit_subject_matches_spec_format() {
        let (_dir, ws) = fixture_workspace_with_remote();
        add_committed_change(&ws, "add-greetings", "Make the project greet users on startup");

        let executor = CompletingExecutorWithDiff {
            artifact_name: "GREETINGS".into(),
            artifact_text: "hello world".into(),
        };
        run_one_pass_no_push(&ws, &executor).await.expect("pass succeeds");

        // Inspect HEAD on agent-q. The orchestrator left us on agent-q after
        // recreate_branch + commit; verify subject directly.
        let out = std::process::Command::new("git")
            .args(["log", "-1", "--pretty=%s", "agent-q"])
            .current_dir(&ws)
            .output()
            .unwrap();
        assert!(out.status.success(), "git log failed");
        let subject = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert_eq!(
            subject,
            "add-greetings: Make the project greet users on startup",
            "subject should be `<change>: <first ## Why line>`"
        );
        assert!(
            subject.chars().count() <= 72,
            "subject should be ≤72 chars, got {} chars: {subject:?}",
            subject.chars().count()
        );
    }

    /// 13.4.4 / git-workflow-manager baseline: `Completed` with empty diff
    /// archives the change but does NOT create an empty commit.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn completed_no_diff_archives_without_commit() {
        let (_dir, ws) = fixture_workspace_with_remote();
        add_committed_change(&ws, "no-op-change", "intentionally a no-op");

        let pre_main = crate::git::rev_parse(&ws, "main").unwrap();

        let executor = CompletingExecutorNoDiff;
        run_one_pass_no_push(&ws, &executor).await.expect("pass succeeds");

        // Change is archived (active dir gone, dated archive entry exists).
        assert!(!ws.join("openspec/changes/no-op-change").exists());
        let archive_root = ws.join("openspec/changes/archive");
        let mut found = false;
        if archive_root.exists() {
            for entry in std::fs::read_dir(&archive_root).unwrap() {
                let name = entry.unwrap().file_name().into_string().unwrap();
                if name.ends_with("-no-op-change") {
                    found = true;
                    break;
                }
            }
        }
        assert!(found, "change must be archived under archive/ with date prefix");

        // No commit was made: agent-q must still equal main's pre-pass SHA.
        let agent_sha = crate::git::rev_parse(&ws, "agent-q").unwrap();
        assert_eq!(agent_sha, pre_main, "no-diff Completed must not create a commit");
    }

    /// 13.4.2 / git-workflow-manager baseline: when `git pull --ff-only`
    /// fails (base branch has diverged from origin), the iteration aborts
    /// and the agent branch is NOT created or modified.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pull_conflict_aborts_iteration_without_touching_agent_branch() {
        let (_dir, ws) = fixture_workspace_with_remote();

        // Reach into the remote (the fixture's `remote/` sibling) to advance
        // origin/main with a commit our local doesn't have.
        let remote = ws.parent().unwrap().join("remote");
        std::fs::write(remote.join("REMOTE_ONLY.md"), "remote-side\n").unwrap();
        let st = std::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(&remote)
            .status()
            .unwrap();
        assert!(st.success());
        let st = std::process::Command::new("git")
            .args(["commit", "-q", "-m", "remote-side commit"])
            .current_dir(&remote)
            .status()
            .unwrap();
        assert!(st.success());

        // Now create a divergent local commit on main so pull --ff-only fails
        // (our local main is not an ancestor of origin/main and vice versa).
        std::fs::write(ws.join("LOCAL_ONLY.md"), "local-side\n").unwrap();
        let st = std::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(&ws)
            .status()
            .unwrap();
        assert!(st.success());
        let st = std::process::Command::new("git")
            .args(["commit", "-q", "-m", "local-side commit"])
            .current_dir(&ws)
            .status()
            .unwrap();
        assert!(st.success());

        // Sanity: agent-q must not exist yet.
        assert!(crate::git::rev_parse(&ws, "agent-q").is_err(),
            "agent-q must not exist before the pass");

        let executor = CompletingExecutorNoDiff;
        let result = run_one_pass_no_push(&ws, &executor).await;
        assert!(result.is_err(), "pass must error when pull --ff-only fails");
        let msg = format!("{:#}", result.unwrap_err());
        assert!(
            msg.contains("git pull --ff-only failed") || msg.contains("non-fast-forward"),
            "error must surface the git failure verbatim, got: {msg}"
        );

        // Agent branch must remain absent after the aborted iteration.
        assert!(
            crate::git::rev_parse(&ws, "agent-q").is_err(),
            "agent-q must not be created when the iteration aborts at pull"
        );
    }

    /// 13.4.7 / git-workflow-manager baseline: empty pass produces no
    /// commits and does not call the GitHub API.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_pass_produces_no_commits_and_no_pr() {
        let (_dir, ws) = fixture_workspace_with_remote();
        // No changes added — queue is empty.

        let pre_main = crate::git::rev_parse(&ws, "main").unwrap();

        let executor = CompletingExecutorNoDiff;
        // run_one_pass_no_push only runs through commit formation; if any
        // commits were produced inappropriately, the test would still need
        // to assert agent-q equals main below. The empty queue means the
        // function returns early without invoking the executor.
        let processed = run_one_pass_no_push(&ws, &executor)
            .await
            .expect("empty pass succeeds");
        assert!(processed.is_empty(), "expected no processed changes, got {processed:?}");

        let agent_sha = crate::git::rev_parse(&ws, "agent-q").unwrap();
        assert_eq!(agent_sha, pre_main, "empty pass must not advance agent branch");
    }

    /// Counting failing executor: increments a shared counter on every call,
    /// always returns `Failed`.
    struct CountingFailingExecutor(std::sync::atomic::AtomicUsize);
    #[async_trait::async_trait]
    impl Executor for CountingFailingExecutor {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            self.0
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(ExecutorOutcome::Failed {
                reason: "fixture".into(),
            })
        }
        async fn resume(
            &self,
            _h: crate::executor::ResumeHandle,
            _a: &str,
        ) -> Result<ExecutorOutcome> {
            unreachable!()
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn iteration_error_continues() {
        // Verify the polling loop runs ≥2 iterations even when the executor
        // returns `Failed` on every change. Failed changes stay in the queue
        // (no archive), so each iteration re-locks, re-invokes, and re-fails.
        let (_dir, ws) = fixture_workspace_with_remote();
        // One pending change so each pass invokes the executor. The change
        // material must be committed in the fixture so the workspace is not
        // dirty when the polling pass starts (production repos commit their
        // openspec/changes/ tree alongside source code).
        let change_dir = ws.join("openspec/changes/feature-x");
        std::fs::create_dir_all(&change_dir).unwrap();
        std::fs::write(change_dir.join("proposal.md"), "## Why\nbecause\n").unwrap();
        let status = std::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(&ws)
            .status()
            .unwrap();
        assert!(status.success());
        let status = std::process::Command::new("git")
            .args(["commit", "-q", "-m", "add fixture change"])
            .current_dir(&ws)
            .status()
            .unwrap();
        assert!(status.success());
        // Also push so origin/main matches local main; otherwise the
        // `git pull --ff-only origin main` in the pass becomes a no-op of
        // the original commit, which is fine. We don't actually need to push
        // in this test.

        let executor = Arc::new(CountingFailingExecutor(
            std::sync::atomic::AtomicUsize::new(0),
        ));
        let executor_dyn: Arc<dyn Executor> = executor.clone();

        let repo = RepositoryConfig {
            url: "git@github.com:owner/fixture.git".into(),
            local_path: Some(ws.clone()),
            base_branch: "main".into(),
            agent_branch: "agent-q".into(),
            poll_interval_sec: 0, // tight loop so we get many iterations fast
        };
        let github = GithubConfig {
            token_env: "DOES_NOT_EXIST".into(),
        };
        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let handle = tokio::spawn(async move {
            run(repo, executor_dyn, github, cancel_for_task).await;
        });

        // Let several iterations run, then cancel. The git operations are
        // moderately slow (clone/fetch take ~tens of ms each), so we give a
        // generous window.
        tokio::time::sleep(Duration::from_millis(500)).await;
        cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("loop should exit within 2s of cancel");

        let count = executor.0.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            count >= 2,
            "expected ≥2 executor invocations across iterations, got {count}"
        );
    }

    /// Cancellation must break the loop within the sleep window. We use a
    /// 60-second poll interval so the only way the test passes within the
    /// timeout is if `cancel.cancelled()` wins the `select!`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_during_sleep_exits() {
        use crate::executor::ResumeHandle;
        use async_trait::async_trait;

        struct AlwaysFails;
        #[async_trait]
        impl Executor for AlwaysFails {
            async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
                Ok(ExecutorOutcome::Failed {
                    reason: "fixture".into(),
                })
            }
            async fn resume(&self, _h: ResumeHandle, _a: &str) -> Result<ExecutorOutcome> {
                unreachable!()
            }
        }

        // Fixture workspace: an empty directory + a `local_path` that points
        // to it AND has no `.git` directory so `ensure_initialized` errors.
        // That error is logged and the loop sleeps; cancellation breaks out.
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();

        let repo = RepositoryConfig {
            url: "git@github.com:owner/empty.git".into(),
            local_path: Some(ws.clone()),
            base_branch: "main".into(),
            agent_branch: "agent-q".into(),
            poll_interval_sec: 60,
        };
        let github = GithubConfig {
            token_env: "DOES_NOT_EXIST".into(),
        };
        let cancel = CancellationToken::new();
        let executor: Arc<dyn Executor> = Arc::new(AlwaysFails);

        let cancel_for_task = cancel.clone();
        let handle = tokio::spawn(async move {
            run(repo, executor, github, cancel_for_task).await;
        });

        // Give the loop time to enter its sleep, then cancel.
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();

        // The loop must exit within 1s of cancellation. The 60s sleep would
        // otherwise dominate.
        let res = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(res.is_ok(), "polling loop did not exit within 1s of cancel");
    }
}
