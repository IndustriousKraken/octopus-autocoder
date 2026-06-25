//! executor-outcome-legibility-and-retry §5.2: the bounded
//! no-committable-result retry. Drives [`run_executor_with_retry`] with a
//! counting fake executor + a real temp git workspace (no real subprocess);
//! asserts attempt COUNTS / outcomes, never message wording.

use super::*;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// Counting fake implementer for the retry helper. Records how many times
/// `run` was invoked, returns a configurable outcome shape, and optionally
/// writes a file (a committable working-tree diff) on each run.
/// `session_retries` / `is_retryable` are overridable to drive the bound AND
/// the strategy hint.
struct RetryFake {
    runs: AtomicU32,
    session_retries: u32,
    /// Each `run` writes a fresh file into the workspace → a committable diff.
    writes_result: bool,
    /// Returned verbatim by `is_retryable`.
    retry_hint: Option<bool>,
    /// `run` returns `Completed` when true, else `Failed`.
    completed: bool,
}

impl RetryFake {
    fn failing(session_retries: u32) -> Self {
        Self {
            runs: AtomicU32::new(0),
            session_retries,
            writes_result: false,
            retry_hint: None,
            completed: false,
        }
    }
    fn runs(&self) -> u32 {
        self.runs.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl Executor for RetryFake {
    fn session_retries(&self) -> u32 {
        self.session_retries
    }
    fn is_retryable(&self, _outcome: &ExecutorOutcome) -> Option<bool> {
        self.retry_hint
    }
    async fn run(&self, workspace: &Path, _change: &str) -> Result<ExecutorOutcome> {
        let n = self.runs.fetch_add(1, Ordering::SeqCst);
        if self.writes_result {
            std::fs::write(workspace.join(format!("artifact-{n}.txt")), "work\n")?;
        }
        Ok(if self.completed {
            ExecutorOutcome::Completed { final_answer: None }
        } else {
            ExecutorOutcome::Failed {
                reason: "transient upstream overload".into(),
            }
        })
    }
    async fn resume(
        &self,
        _h: crate::executor::ResumeHandle,
        _a: &str,
    ) -> Result<ExecutorOutcome> {
        unreachable!("retry tests never resume")
    }
}

/// A clean-working-tree workspace carrying one pending change.
fn clean_workspace() -> (tempfile::TempDir, std::path::PathBuf) {
    let (dir, ws) = fixture_workspace_with_remote();
    add_committed_change(&ws, "chg", "why line");
    (dir, ws)
}

/// A no-result failure (clean tree) is re-invoked up to `session_retries`
/// ADDITIONAL times (3 total for N=2), then the failure is surfaced.
#[tokio::test]
async fn no_result_failure_retried_up_to_the_bound() {
    let (_dir, ws) = clean_workspace();
    let repo = fixture_repo(&ws);
    let exec = RetryFake::failing(2);
    let outcome = run_executor_with_retry(&exec, &repo, &ws, "chg", Duration::ZERO).await;
    assert_eq!(exec.runs(), 3, "1 initial + 2 retries");
    assert!(
        matches!(outcome, Ok(ExecutorOutcome::Failed { .. })),
        "exhausted retries surface the failure: {outcome:?}"
    );
}

/// A failure that produced a committable result (dirty tree) is NOT retried —
/// it is surfaced with whatever it produced.
#[tokio::test]
async fn committable_result_failure_is_not_retried() {
    let (_dir, ws) = clean_workspace();
    let repo = fixture_repo(&ws);
    let mut exec = RetryFake::failing(2);
    exec.writes_result = true;
    let _ = run_executor_with_retry(&exec, &repo, &ws, "chg", Duration::ZERO).await;
    assert_eq!(exec.runs(), 1, "a committable result is never blindly re-run");
}

/// `session_retries: 0` disables retry entirely — exactly one attempt.
#[tokio::test]
async fn session_retries_zero_disables_retry() {
    let (_dir, ws) = clean_workspace();
    let repo = fixture_repo(&ws);
    let exec = RetryFake::failing(0);
    let _ = run_executor_with_retry(&exec, &repo, &ws, "chg", Duration::ZERO).await;
    assert_eq!(exec.runs(), 1, "zero bound → single attempt");
}

/// A strategy `is_retryable` of `Some(false)` short-circuits the retry even on
/// a no-result failure with a positive bound.
#[tokio::test]
async fn is_retryable_some_false_short_circuits() {
    let (_dir, ws) = clean_workspace();
    let repo = fixture_repo(&ws);
    let mut exec = RetryFake::failing(2);
    exec.retry_hint = Some(false);
    let _ = run_executor_with_retry(&exec, &repo, &ws, "chg", Duration::ZERO).await;
    assert_eq!(exec.runs(), 1, "Some(false) declares the failure non-retryable");
}

/// A strategy `is_retryable` of `Some(true)` retries even when a committable
/// result exists (overriding the no-result guard), still bounded by
/// `session_retries`.
#[tokio::test]
async fn is_retryable_some_true_retries_past_committable_result() {
    let (_dir, ws) = clean_workspace();
    let repo = fixture_repo(&ws);
    let mut exec = RetryFake::failing(2);
    exec.writes_result = true; // committable result present each run
    exec.retry_hint = Some(true);
    let _ = run_executor_with_retry(&exec, &repo, &ws, "chg", Duration::ZERO).await;
    assert_eq!(exec.runs(), 3, "Some(true) retries past the committable-result guard");
}

/// `session_retries: 0` suppresses even a `Some(true)` hint — the bound is the
/// absolute cap on additional attempts.
#[tokio::test]
async fn zero_bound_suppresses_some_true_hint() {
    let (_dir, ws) = clean_workspace();
    let repo = fixture_repo(&ws);
    let mut exec = RetryFake::failing(0);
    exec.retry_hint = Some(true);
    let _ = run_executor_with_retry(&exec, &repo, &ws, "chg", Duration::ZERO).await;
    assert_eq!(exec.runs(), 1, "zero bound caps additional attempts even with Some(true)");
}

/// A `Completed` outcome that produced a committable result is NOT retried by
/// default (no `is_retryable` opinion) — a `Completed` with a diff is a real
/// success, never blindly re-run.
#[tokio::test]
async fn completed_with_committable_result_not_retried_by_default() {
    let (_dir, ws) = clean_workspace();
    let repo = fixture_repo(&ws);
    let mut exec = RetryFake::failing(2);
    exec.completed = true; // returns `Completed`
    exec.writes_result = true; // committable result present
    let _ = run_executor_with_retry(&exec, &repo, &ws, "chg", Duration::ZERO).await;
    assert_eq!(
        exec.runs(),
        1,
        "a Completed with a committable result is a real success, not retried"
    );
}

/// A strategy `is_retryable` of `Some(true)` retries a `Completed` outcome even
/// when it produced a committable result, consistent with the `Failed` arm AND
/// the spec's "`Some(true)` retries even when a committable result exists" —
/// closing the prior asymmetry where the `Completed` arm ignored a `Some(true)`
/// hint. Still bounded by `session_retries`.
#[tokio::test]
async fn completed_some_true_retries_past_committable_result() {
    let (_dir, ws) = clean_workspace();
    let repo = fixture_repo(&ws);
    let mut exec = RetryFake::failing(2);
    exec.completed = true; // returns `Completed`
    exec.writes_result = true; // committable result present each run
    exec.retry_hint = Some(true);
    let _ = run_executor_with_retry(&exec, &repo, &ws, "chg", Duration::ZERO).await;
    assert_eq!(
        exec.runs(),
        3,
        "Some(true) retries a Completed past the committable-result guard"
    );
}

/// A strategy `is_retryable` of `Some(false)` vetoes retry on a `Completed`
/// no-result outcome, consistent with the `Failed` arm.
#[tokio::test]
async fn completed_some_false_vetoes_retry() {
    let (_dir, ws) = clean_workspace();
    let repo = fixture_repo(&ws);
    let mut exec = RetryFake::failing(2);
    exec.completed = true; // returns `Completed`, clean tree (no committable result)
    exec.retry_hint = Some(false);
    let _ = run_executor_with_retry(&exec, &repo, &ws, "chg", Duration::ZERO).await;
    assert_eq!(exec.runs(), 1, "Some(false) declares the Completed non-retryable");
}
