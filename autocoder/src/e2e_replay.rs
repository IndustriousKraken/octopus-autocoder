//! app-under-test-e2e: the red-green replay that catches vacuous end-to-end
//! tests.
//!
//! A test written in the same session as the feature can pass without
//! exercising it — asserting on a selector the agent invented, or never
//! mounting the component. The check is mechanical, so it needs no LLM:
//! replay the pass's NEW end-to-end tests against the PRE-CHANGE tree and
//! require them to fail. A test that passes without the change under test is
//! not evidence that the change works.
//!
//! Only the test files are overlaid onto the base tree — never the
//! implementation, which would defeat the entire check. Everything happens in
//! a scratch worktree, so the agent branch is never touched.

use crate::app_under_test::{self, E2eOutcome};
use crate::config::AppUnderTestConfig;
use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// What the replay established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayOutcome {
    /// The new tests FAILED against the base commit — the expected, healthy
    /// result. They detect something the change introduced.
    Red { tests: Vec<String> },
    /// The new tests PASSED against the base commit. Either they are vacuous
    /// or the behavior already existed; both need a human, so the PR drafts.
    GreenAgainstBase { tests: Vec<String> },
    /// No end-to-end test file changed, so there was nothing to replay.
    Skipped { reason: String },
    /// The replay itself could not be performed. Reported, never silently
    /// treated as a pass.
    CouldNotRun { reason: String },
}

impl ReplayOutcome {
    /// A green-against-base result drafts the PR: the work is kept, but a
    /// human decides whether the test is vacuous or the behavior pre-existed.
    pub fn drafts_pr(&self) -> bool {
        matches!(self, ReplayOutcome::GreenAgainstBase { .. })
    }

    /// The replay's contribution to the `## End-to-end verification` section.
    pub fn render_pr_section(&self) -> String {
        match self {
            ReplayOutcome::Red { tests } => format!(
                "\nVacuous-test replay: {} new end-to-end test file(s) were replayed against \
                 the base commit and failed there, as expected.\n",
                tests.len()
            ),
            ReplayOutcome::GreenAgainstBase { tests } => format!(
                "\n**Vacuous-test replay FAILED.** These end-to-end tests PASS against the \
                 base commit, so they do not demonstrate that this change works — they are \
                 either vacuous or the behavior already existed. This PR is a draft.\n\n{}\n",
                tests.iter().map(|t| format!("- `{t}`")).collect::<Vec<_>>().join("\n")
            ),
            ReplayOutcome::Skipped { reason } => {
                format!("\nVacuous-test replay: not run ({reason}).\n")
            }
            ReplayOutcome::CouldNotRun { reason } => format!(
                "\nVacuous-test replay: could NOT run ({reason}); the new tests were not \
                 checked against the base commit.\n"
            ),
        }
    }
}

/// Run a git command, returning stdout on success.
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .with_context(|| format!("spawning `git {}`", args.join(" ")))?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Paths changed between `base` and `HEAD`, workspace-relative.
pub fn changed_paths(workspace: &Path, base: &str) -> Result<Vec<String>> {
    let out = git(workspace, &["diff", "--name-only", &format!("{base}..HEAD")])?;
    Ok(out.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
}

/// Removes the scratch worktree on every exit path, including panics.
///
/// A leaked worktree is not merely clutter: it stays registered in the
/// repository's metadata and the next `git worktree add` at the same path
/// fails, so the replay would break permanently after one bad run.
struct WorktreeGuard {
    repo: PathBuf,
    path: PathBuf,
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        let path = self.path.display().to_string();
        if let Err(e) = git(&self.repo, &["worktree", "remove", "--force", &path]) {
            tracing::warn!("could not remove replay worktree {path}: {e}");
            // Fall back to pruning the registration so a stale entry cannot
            // block the next replay.
            let _ = git(&self.repo, &["worktree", "prune"]);
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Replay the pass's new end-to-end tests against the base commit.
///
/// `base` is the commit the pass started from. The scratch worktree is created
/// under `scratch_root` (the daemon's cache), never inside the workspace,
/// so it can never be picked up as a repository change.
pub async fn run_replay(
    cfg: &AppUnderTestConfig,
    workspace: &Path,
    base: &str,
    scratch_root: &Path,
    ready_timeout: Duration,
    e2e_timeout: Duration,
) -> ReplayOutcome {
    // 1. Which changed files are end-to-end tests?
    let changed = match changed_paths(workspace, base) {
        Ok(c) => c,
        Err(e) => {
            return ReplayOutcome::CouldNotRun {
                reason: format!("could not list changed paths: {e}"),
            };
        }
    };
    let patterns = cfg.resolved_e2e_test_paths();
    let tests: Vec<String> = app_under_test::e2e_test_files(&changed, &patterns)
        .into_iter()
        .cloned()
        .collect();
    if tests.is_empty() {
        return ReplayOutcome::Skipped {
            reason: "the pass changed no end-to-end test file".to_string(),
        };
    }

    // 2. Scratch worktree at the base commit.
    if let Err(e) = std::fs::create_dir_all(scratch_root) {
        return ReplayOutcome::CouldNotRun {
            reason: format!("could not create the replay scratch root: {e}"),
        };
    }
    let scratch = scratch_root.join(format!("replay-{base}"));
    // A leftover from an interrupted run would make `worktree add` fail.
    let _ = git(workspace, &["worktree", "remove", "--force", &scratch.display().to_string()]);
    let _ = std::fs::remove_dir_all(&scratch);
    if let Err(e) = git(
        workspace,
        &["worktree", "add", "--detach", &scratch.display().to_string(), base],
    ) {
        return ReplayOutcome::CouldNotRun {
            reason: format!("could not create the replay worktree: {e}"),
        };
    }
    let _guard = WorktreeGuard { repo: workspace.to_path_buf(), path: scratch.clone() };

    // 3. Overlay ONLY the test files. Copying the implementation too would
    //    make every test trivially pass and the check meaningless.
    for rel in &tests {
        let src = workspace.join(rel);
        let dst = scratch.join(rel);
        if !src.exists() {
            // The pass deleted this test; nothing to overlay.
            continue;
        }
        if let Some(parent) = dst.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return ReplayOutcome::CouldNotRun {
                reason: format!("could not create {} in the replay worktree: {e}", parent.display()),
            };
        }
        if let Err(e) = std::fs::copy(&src, &dst) {
            return ReplayOutcome::CouldNotRun {
                reason: format!("could not overlay {rel}: {e}"),
            };
        }
    }

    // 4. Start the application FROM THE SCRATCH TREE, on its own port.
    let app = match app_under_test::start_and_wait_ready(cfg, &scratch, ready_timeout).await {
        Ok(a) => a,
        Err(e) => {
            return ReplayOutcome::CouldNotRun {
                reason: format!("the application did not start from the base tree: {e}"),
            };
        }
    };

    // 5. Run the suite. It MUST fail: the new tests are expected to detect
    //    the absence of the change.
    let outcome = app_under_test::run_e2e(cfg, &scratch, app.port, e2e_timeout).await;
    drop(app);

    match outcome {
        E2eOutcome::Passed { .. } => ReplayOutcome::GreenAgainstBase { tests },
        E2eOutcome::Failed { .. } | E2eOutcome::TimedOut { .. } => ReplayOutcome::Red { tests },
        E2eOutcome::NotRun { reason } => ReplayOutcome::CouldNotRun {
            reason: format!("the suite did not run against the base tree: {reason}"),
        },
    }
}

/// The pass-level result of end-to-end verification: the PR-body section and
/// whether the PR must open as a draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2eVerification {
    pub section: String,
    pub drafts_pr: bool,
}

impl E2eVerification {
    /// The outcome when no application was running for the pass. Rendered
    /// explicitly — an absent section would read as "no end-to-end concern",
    /// which is the inference this feature exists to prevent — but it does not
    /// draft: the feature is opt-in and degrades by design.
    pub fn not_run(reason: impl Into<String>) -> Self {
        let outcome = E2eOutcome::NotRun { reason: reason.into() };
        Self { section: outcome.render_pr_section("(not run)"), drafts_pr: false }
    }
}

/// Run the suite against the pass's application, then replay the pass's new
/// end-to-end tests against the base commit, and combine both into one PR
/// section and one draft decision.
///
/// The replay is deliberately skipped when the suite itself did not pass:
/// its question ("do these tests detect the change?") is only meaningful once
/// they are known to pass WITH the change. Replaying tests that fail in both
/// trees would report a confusing red-and-also-red.
pub async fn run_e2e_verification(
    cfg: &AppUnderTestConfig,
    workspace: &Path,
    port: u16,
    base: &str,
    scratch_root: &Path,
    ready_timeout: Duration,
    e2e_timeout: Duration,
) -> E2eVerification {
    let suite = app_under_test::run_e2e(cfg, workspace, port, e2e_timeout).await;
    let mut section = suite.render_pr_section(&cfg.e2e_command);
    let mut drafts = suite.drafts_pr();

    if suite.is_pass() {
        let replay =
            run_replay(cfg, workspace, base, scratch_root, ready_timeout, e2e_timeout).await;
        section.push_str(&replay.render_pr_section());
        drafts = drafts || replay.drafts_pr();
    } else {
        section.push_str(
            "\nVacuous-test replay: not run (the suite did not pass against this change, so \
             replaying it against the base commit would not be informative).\n",
        );
    }

    E2eVerification { section, drafts_pr: drafts }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ReadyCheck;

    fn cfg(e2e_command: &str) -> AppUnderTestConfig {
        AppUnderTestConfig {
            start_command: "sleep 30".to_string(),
            ready_check: ReadyCheck { http_path: None, command: Some("true".to_string()) },
            e2e_command: e2e_command.to_string(),
            working_dir: None,
            ready_timeout_secs: 5,
            e2e_timeout_secs: 30,
            e2e_test_paths: None,
        }
    }

    /// A repo with one base commit, then a second commit adding an e2e test
    /// AND the implementation file that test checks for.
    fn repo_with_change() -> (tempfile::TempDir, PathBuf, String) {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        git(&ws, &["init", "-q", "-b", "main"]).unwrap();
        git(&ws, &["config", "user.email", "t@example.com"]).unwrap();
        git(&ws, &["config", "user.name", "t"]).unwrap();

        std::fs::write(ws.join("README.md"), "base\n").unwrap();
        git(&ws, &["add", "-A"]).unwrap();
        git(&ws, &["commit", "-qm", "base"]).unwrap();
        let base = git(&ws, &["rev-parse", "HEAD"]).unwrap().trim().to_string();

        // The "change": an implementation file plus an e2e test for it.
        std::fs::create_dir_all(ws.join("tests/e2e")).unwrap();
        std::fs::write(ws.join("tests/e2e/feature.spec.ts"), "// asserts impl.txt exists\n")
            .unwrap();
        std::fs::write(ws.join("impl.txt"), "the feature\n").unwrap();
        git(&ws, &["add", "-A"]).unwrap();
        git(&ws, &["commit", "-qm", "feature + test"]).unwrap();

        (tmp, ws, base)
    }

    #[tokio::test]
    async fn a_genuine_test_replays_red_against_base() {
        let (_tmp, ws, base) = repo_with_change();
        let scratch = _tmp.path().join("scratch");
        // The suite checks for the implementation file, which does NOT exist
        // at the base commit → fails there → red → healthy.
        let outcome = run_replay(
            &cfg("test -f impl.txt"),
            &ws,
            &base,
            &scratch,
            Duration::from_secs(5),
            Duration::from_secs(30),
        )
        .await;
        assert!(matches!(outcome, ReplayOutcome::Red { .. }), "got {outcome:?}");
        assert!(!outcome.drafts_pr(), "a red replay is the healthy result");
    }

    #[tokio::test]
    async fn a_vacuous_test_replays_green_and_drafts_the_pr() {
        let (_tmp, ws, base) = repo_with_change();
        let scratch = _tmp.path().join("scratch");
        // A suite that passes regardless — the vacuous case.
        let outcome = run_replay(
            &cfg("true"),
            &ws,
            &base,
            &scratch,
            Duration::from_secs(5),
            Duration::from_secs(30),
        )
        .await;
        match &outcome {
            ReplayOutcome::GreenAgainstBase { tests } => {
                assert_eq!(tests, &vec!["tests/e2e/feature.spec.ts".to_string()]);
            }
            other => panic!("expected green-against-base, got {other:?}"),
        }
        assert!(outcome.drafts_pr());
        let section = outcome.render_pr_section();
        assert!(section.contains("feature.spec.ts"), "names the offending test");
        assert!(section.contains("draft"));
    }

    #[tokio::test]
    async fn no_e2e_test_change_skips_the_replay() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        git(&ws, &["init", "-q", "-b", "main"]).unwrap();
        git(&ws, &["config", "user.email", "t@example.com"]).unwrap();
        git(&ws, &["config", "user.name", "t"]).unwrap();
        std::fs::write(ws.join("README.md"), "base\n").unwrap();
        git(&ws, &["add", "-A"]).unwrap();
        git(&ws, &["commit", "-qm", "base"]).unwrap();
        let base = git(&ws, &["rev-parse", "HEAD"]).unwrap().trim().to_string();
        // Implementation only — no e2e test touched.
        std::fs::write(ws.join("src.txt"), "impl\n").unwrap();
        git(&ws, &["add", "-A"]).unwrap();
        git(&ws, &["commit", "-qm", "impl only"]).unwrap();

        let outcome = run_replay(
            &cfg("true"),
            &ws,
            &base,
            &tmp.path().join("scratch"),
            Duration::from_secs(5),
            Duration::from_secs(30),
        )
        .await;
        assert!(matches!(outcome, ReplayOutcome::Skipped { .. }), "got {outcome:?}");
        assert!(!outcome.drafts_pr());
        assert!(outcome.render_pr_section().contains("not run"));
    }

    #[tokio::test]
    async fn replay_leaves_the_workspace_untouched_and_removes_the_worktree() {
        let (_tmp, ws, base) = repo_with_change();
        let scratch = _tmp.path().join("scratch");
        let head_before = git(&ws, &["rev-parse", "HEAD"]).unwrap();

        let _ = run_replay(
            &cfg("test -f impl.txt"),
            &ws,
            &base,
            &scratch,
            Duration::from_secs(5),
            Duration::from_secs(30),
        )
        .await;

        // The agent branch is exactly where it was.
        assert_eq!(git(&ws, &["rev-parse", "HEAD"]).unwrap(), head_before);
        assert!(
            git(&ws, &["status", "--porcelain"]).unwrap().trim().is_empty(),
            "the workspace working tree is unmodified"
        );
        // The scratch worktree is deregistered, so the next replay can run.
        let list = git(&ws, &["worktree", "list"]).unwrap();
        assert!(!list.contains("replay-"), "worktree removed: {list}");
    }

    #[tokio::test]
    async fn verification_combines_a_passing_suite_with_a_red_replay() {
        let (_tmp, ws, base) = repo_with_change();
        let v = run_e2e_verification(
            // Passes with the change present (impl.txt exists in the
            // workspace) and fails at base, where it does not.
            &cfg("test -f impl.txt"),
            &ws,
            5555,
            &base,
            &_tmp.path().join("scratch"),
            Duration::from_secs(5),
            Duration::from_secs(30),
        )
        .await;
        assert!(!v.drafts_pr, "passing suite + red replay is the healthy path");
        assert!(v.section.contains("## End-to-end verification"));
        assert!(v.section.contains("passed"));
        assert!(v.section.contains("failed there, as expected"));
    }

    #[tokio::test]
    async fn verification_drafts_when_the_replay_is_green_against_base() {
        let (_tmp, ws, base) = repo_with_change();
        let v = run_e2e_verification(
            &cfg("true"), // passes everywhere → vacuous
            &ws,
            5555,
            &base,
            &_tmp.path().join("scratch"),
            Duration::from_secs(5),
            Duration::from_secs(30),
        )
        .await;
        assert!(v.drafts_pr, "a vacuous test drafts the PR even though the suite passed");
        assert!(v.section.contains("Vacuous-test replay FAILED"));
    }

    #[tokio::test]
    async fn a_failing_suite_drafts_and_skips_the_replay_as_uninformative() {
        let (_tmp, ws, base) = repo_with_change();
        let v = run_e2e_verification(
            &cfg("exit 1"),
            &ws,
            5555,
            &base,
            &_tmp.path().join("scratch"),
            Duration::from_secs(5),
            Duration::from_secs(30),
        )
        .await;
        assert!(v.drafts_pr, "a failing suite drafts");
        assert!(v.section.contains("FAILED (exit 1)"));
        assert!(
            v.section.contains("would not be informative"),
            "the replay is skipped with a stated reason: {}",
            v.section
        );
    }

    #[test]
    fn not_run_renders_a_section_without_drafting() {
        let v = E2eVerification::not_run("no application was started for this pass");
        assert!(!v.drafts_pr);
        assert!(v.section.contains("## End-to-end verification"));
        assert!(v.section.contains("did NOT run"));
    }

    #[tokio::test]
    async fn a_bad_base_reports_rather_than_claiming_a_result() {
        let (_tmp, ws, _base) = repo_with_change();
        let outcome = run_replay(
            &cfg("true"),
            &ws,
            "0000000000000000000000000000000000000000",
            &_tmp.path().join("scratch"),
            Duration::from_secs(5),
            Duration::from_secs(30),
        )
        .await;
        assert!(matches!(outcome, ReplayOutcome::CouldNotRun { .. }), "got {outcome:?}");
        // Critically, it does not draft — an unrunnable replay is not evidence
        // of a vacuous test.
        assert!(!outcome.drafts_pr());
    }
}
