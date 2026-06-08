//! a71: the queue walk yields to pending operator chatops requests.
//!
//! With a non-empty pending list AND an operator request (`send it` /
//! `propose` / `changelog`) queued, the walk processes at most ONE change
//! then ends the batch — so the next iteration's iteration-top drain
//! consumes the operator request rather than letting it wait behind the
//! whole backlog. The peek does NOT drain; the request stays queued.

use super::*;
use std::sync::{Arc, Mutex};

fn github_cfg() -> GithubConfig {
    GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    }
}

fn empty_queues() -> OperatorRequestQueues {
    OperatorRequestQueues {
        triages: Arc::new(Mutex::new(Vec::new())),
        proposal_requests: Arc::new(Mutex::new(Vec::new())),
        changelog_requests: Arc::new(Mutex::new(Vec::new())),
    }
}

fn fixture_changelog_request() -> crate::control_socket::ChangelogRequest {
    crate::control_socket::ChangelogRequest {
        request_id: "clr-1".into(),
        repo_url: "git@github.com:owner/fixture.git".into(),
        raw_args: "".into(),
        channel: "C_TEST".into(),
        lifecycle_thread_ts: "T-changelog".into(),
        submitted_at: chrono::Utc::now(),
    }
}

fn fixture_proposal_request() -> crate::control_socket::ProposalRequest {
    crate::control_socket::ProposalRequest {
        request_id: "pr-1".into(),
        channel: "C_TEST".into(),
        thread_ts: "T-propose".into(),
        operator_user: "U_OP".into(),
        request_text: "add a /healthz endpoint".into(),
        submitted_at: chrono::Utc::now(),
    }
}

/// Task 2.1: with ≥2 pending changes AND a queued `changelog` request, the
/// walk processes exactly ONE change then yields — the remaining change is
/// NOT processed AND the changelog request is still queued (drained next
/// iteration).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn walk_yields_after_one_change_when_changelog_pending() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "ch01", "first");
    add_committed_change(&ws, "ch02", "second");

    let queues = empty_queues();
    queues
        .changelog_requests
        .lock()
        .unwrap()
        .push(fixture_changelog_request());

    let executor = PerChangeArtifactExecutor;
    let repo = fixture_repo(&ws);
    let gh = github_cfg();
    let (processed, _) = operator_requests::scope(
        Some(queues.clone()),
        run_pass_through_commits(
            &paths,
            &ws,
            &repo,
            &gh,
            &executor,
            None,
            u32::MAX,
            u32::MAX, // generous cap; the yield must come from the request
            &crate::audits::AuditRegistry::default(),
            None,
            &std::collections::HashMap::new(),
            &std::collections::HashSet::new(),
        ),
    )
    .await
    .expect("pass succeeds");

    assert_eq!(
        processed,
        vec!["ch01".to_string()],
        "exactly one change processed before yielding to the changelog request"
    );
    let still_pending = queue::list_pending(&paths, &ws).unwrap();
    assert!(
        still_pending.contains(&"ch02".to_string()),
        "ch02 must still be pending (walk yielded before reaching it): {still_pending:?}"
    );
    assert_eq!(
        queues.changelog_requests.lock().unwrap().len(),
        1,
        "the changelog request must remain queued — the walk peeks, never drains"
    );
}

/// Task 2.2: same bound for a queued `propose` request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn walk_yields_after_one_change_when_proposal_pending() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "ch01", "first");
    add_committed_change(&ws, "ch02", "second");

    let queues = empty_queues();
    queues
        .proposal_requests
        .lock()
        .unwrap()
        .push(fixture_proposal_request());

    let executor = PerChangeArtifactExecutor;
    let repo = fixture_repo(&ws);
    let gh = github_cfg();
    let (processed, _) = operator_requests::scope(
        Some(queues.clone()),
        run_pass_through_commits(
            &paths,
            &ws,
            &repo,
            &gh,
            &executor,
            None,
            u32::MAX,
            u32::MAX,
            &crate::audits::AuditRegistry::default(),
            None,
            &std::collections::HashMap::new(),
            &std::collections::HashSet::new(),
        ),
    )
    .await
    .expect("pass succeeds");

    assert_eq!(
        processed,
        vec!["ch01".to_string()],
        "exactly one change processed before yielding to the propose request"
    );
    let still_pending = queue::list_pending(&paths, &ws).unwrap();
    assert!(
        still_pending.contains(&"ch02".to_string()),
        "ch02 must still be pending: {still_pending:?}"
    );
    assert_eq!(
        queues.proposal_requests.lock().unwrap().len(),
        1,
        "the propose request must remain queued"
    );
}

/// Task 2.2: same bound for a queued `send it` audit-triage request (the
/// `pending_triages` queue holds the audit thread's `thread_ts`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn walk_yields_after_one_change_when_triage_pending() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "ch01", "first");
    add_committed_change(&ws, "ch02", "second");

    let queues = empty_queues();
    queues.triages.lock().unwrap().push("T-sendit".to_string());

    let executor = PerChangeArtifactExecutor;
    let repo = fixture_repo(&ws);
    let gh = github_cfg();
    let (processed, _) = operator_requests::scope(
        Some(queues.clone()),
        run_pass_through_commits(
            &paths,
            &ws,
            &repo,
            &gh,
            &executor,
            None,
            u32::MAX,
            u32::MAX,
            &crate::audits::AuditRegistry::default(),
            None,
            &std::collections::HashMap::new(),
            &std::collections::HashSet::new(),
        ),
    )
    .await
    .expect("pass succeeds");

    assert_eq!(
        processed,
        vec!["ch01".to_string()],
        "exactly one change processed before yielding to the `send it` triage"
    );
    let still_pending = queue::list_pending(&paths, &ws).unwrap();
    assert!(
        still_pending.contains(&"ch02".to_string()),
        "ch02 must still be pending: {still_pending:?}"
    );
    assert_eq!(
        queues.triages.lock().unwrap().len(),
        1,
        "the `send it` triage must remain queued"
    );
}

/// Task 2.3: with ≥2 pending changes AND no operator request pending, the
/// walk processes the FULL batch — existing behavior is unchanged even when
/// the (empty) operator-request context is bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn walk_processes_full_batch_when_no_operator_request_pending() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "ch01", "first");
    add_committed_change(&ws, "ch02", "second");
    add_committed_change(&ws, "ch03", "third");

    // Context present but every queue empty → no yield.
    let queues = empty_queues();

    let executor = PerChangeArtifactExecutor;
    let repo = fixture_repo(&ws);
    let gh = github_cfg();
    let (processed, _) = operator_requests::scope(
        Some(queues.clone()),
        run_pass_through_commits(
            &paths,
            &ws,
            &repo,
            &gh,
            &executor,
            None,
            u32::MAX,
            u32::MAX,
            &crate::audits::AuditRegistry::default(),
            None,
            &std::collections::HashMap::new(),
            &std::collections::HashSet::new(),
        ),
    )
    .await
    .expect("pass succeeds");

    assert_eq!(
        processed,
        vec!["ch01".to_string(), "ch02".to_string(), "ch03".to_string()],
        "the whole batch processes when no operator request is pending"
    );
    let still_pending = queue::list_pending(&paths, &ws).unwrap();
    assert!(
        still_pending.is_empty(),
        "no changes should remain pending: {still_pending:?}"
    );
}

/// Task 2.4: the currently-executing change is always allowed to finish. A
/// request that becomes pending DURING change N does not abort change N; the
/// yield happens only after N's outcome is recorded. The executor pushes a
/// changelog request mid-run on the FIRST change to simulate an operator
/// request arriving while change N occupies the workspace.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn walk_finishes_current_change_before_yielding() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "ch01", "first");
    add_committed_change(&ws, "ch02", "second");

    let queues = empty_queues();

    // Executor that, while processing the FIRST change, enqueues a
    // changelog request (mid-execution arrival) AND then completes the
    // change normally with a real diff so it archives.
    struct QueueRequestMidRun {
        changelog_requests: Arc<Mutex<Vec<crate::control_socket::ChangelogRequest>>>,
        first: std::sync::atomic::AtomicBool,
    }
    #[async_trait::async_trait]
    impl Executor for QueueRequestMidRun {
        async fn run(&self, workspace: &Path, change: &str) -> Result<ExecutorOutcome> {
            if self
                .first
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                self.changelog_requests
                    .lock()
                    .unwrap()
                    .push(crate::control_socket::ChangelogRequest {
                        request_id: "clr-midrun".into(),
                        repo_url: "git@github.com:owner/fixture.git".into(),
                        raw_args: "".into(),
                        channel: "C_TEST".into(),
                        lifecycle_thread_ts: "T-midrun".into(),
                        submitted_at: chrono::Utc::now(),
                    });
            }
            std::fs::write(
                workspace.join(format!("artifact-{change}.txt")),
                format!("contents for {change}\n"),
            )?;
            Ok(ExecutorOutcome::Completed { final_answer: None })
        }
        async fn resume(&self, _h: ResumeHandle, _a: &str) -> Result<ExecutorOutcome> {
            unreachable!()
        }
    }

    let executor = QueueRequestMidRun {
        changelog_requests: queues.changelog_requests.clone(),
        first: std::sync::atomic::AtomicBool::new(true),
    };
    let repo = fixture_repo(&ws);
    let gh = github_cfg();
    let (processed, _) = operator_requests::scope(
        Some(queues.clone()),
        run_pass_through_commits(
            &paths,
            &ws,
            &repo,
            &gh,
            &executor,
            None,
            u32::MAX,
            u32::MAX,
            &crate::audits::AuditRegistry::default(),
            None,
            &std::collections::HashMap::new(),
            &std::collections::HashSet::new(),
        ),
    )
    .await
    .expect("pass succeeds");

    // ch01 ran to completion (was NOT aborted) and archived; the yield
    // happened only AFTER ch01's outcome was recorded, so ch02 is untouched.
    assert_eq!(
        processed,
        vec!["ch01".to_string()],
        "ch01 must finish (not abort) and the walk yields before ch02"
    );
    let still_pending = queue::list_pending(&paths, &ws).unwrap();
    assert!(
        still_pending.contains(&"ch02".to_string()),
        "ch02 must still be pending — the mid-run request did not abort ch01 but did stop the next change: {still_pending:?}"
    );
    assert_eq!(
        queues.changelog_requests.lock().unwrap().len(),
        1,
        "the mid-run changelog request remains queued for the next iteration's drain"
    );
}
