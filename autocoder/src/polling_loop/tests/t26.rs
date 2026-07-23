//! open-pr-gate-fails-closed: the open-PR skip gate fails CLOSED. An
//! unconfirmed query (transport / non-2xx / parse / token) skips the pass
//! instead of proceeding, and a sustained streak of failures raises a
//! throttled operator alert.

use super::*;

/// Executor that PANICS if its `run` is ever invoked. Proves a skipped pass
/// never reaches the lane walk.
struct PanicOnRunExecutor;

#[async_trait::async_trait]
impl Executor for PanicOnRunExecutor {
    async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
        panic!("the executor must NOT run when the open-PR gate skips the pass");
    }
    async fn resume(&self, _h: crate::executor::ResumeHandle, _a: &str) -> Result<ExecutorOutcome> {
        unreachable!()
    }
}

/// Executor that records whether it ran (into a shared flag) and writes a
/// file so the change produces a real diff. Proves the lane walk DID run on a
/// proceeding pass.
struct RecordingExecutor {
    ran: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait::async_trait]
impl Executor for RecordingExecutor {
    async fn run(&self, workspace: &Path, _c: &str) -> Result<ExecutorOutcome> {
        self.ran.store(true, std::sync::atomic::Ordering::SeqCst);
        std::fs::write(workspace.join("GATE_ART.txt"), "x")?;
        Ok(ExecutorOutcome::Completed { final_answer: None })
    }
    async fn resume(&self, _h: crate::executor::ResumeHandle, _a: &str) -> Result<ExecutorOutcome> {
        unreachable!()
    }
}

/// Task 3.1: a 5xx yields `Unknown` and the pass is skipped — no branch init
/// (agent-q is never created), no lane walk (the executor never runs). A
/// subsequent 200-empty response proceeds normally (branch init + lane walk)
/// AND resets the consecutive-failure counter.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn query_failure_skips_pass_then_empty_list_proceeds_and_resets() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "gate-change", "the work behind the gate");

    let github = open_pr_gate_ok_github();
    let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let executor = RecordingExecutor { ran: ran.clone() };

    let _hook = test_hooks::lock();

    // --- Phase 1: the query fails (500) → Unknown → skip. ---
    let mut fail_server = mockito::Server::new_async().await;
    let _fail_mock = fail_server
        .mock("GET", mockito::Matcher::Any)
        .with_status(500)
        .with_body(r#"{"message":"transient outage"}"#)
        .create_async()
        .await;
    test_hooks::set_github_api_base(Some(fail_server.url()));

    let mut counter = 0u32;
    let r1 = execute_one_pass(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &executor,
        &github,
        None,
        None,
        2400u64,
        u32::MAX,
        u32::MAX,
        0,
        Some(10),
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::sync::Mutex::new(Vec::new()),
        &mut counter,
    )
    .await;
    r1.expect("a query-failure skip parks the pass and returns Ok (not an error)");
    assert_eq!(counter, 1, "one Unknown increments the consecutive-failure counter");
    assert!(
        !ran.load(std::sync::atomic::Ordering::SeqCst),
        "no lane walk on a skipped pass: the executor must not run"
    );
    assert!(
        crate::git::rev_parse(&ws, "agent-q").is_err(),
        "no branch init on a skipped pass: agent-q must not be created"
    );

    // --- Phase 2: the query confirms an empty list (200 []) → None → proceed. ---
    let mut ok_server = mockito::Server::new_async().await;
    let _ok_get = mock_open_pr_gate_empty(&mut ok_server).await;
    let _ok_post = ok_server
        .mock("POST", mockito::Matcher::Regex("/pulls".to_string()))
        .with_status(201)
        .with_body(r#"{"html_url":"https://github.com/owner/fixture/pull/1","number":1}"#)
        .create_async()
        .await;
    test_hooks::set_github_api_base(Some(ok_server.url()));

    let r2 = execute_one_pass(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &executor,
        &github,
        None,
        None,
        2400u64,
        u32::MAX,
        u32::MAX,
        0,
        Some(10),
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::sync::Mutex::new(Vec::new()),
        &mut counter,
    )
    .await;
    test_hooks::set_github_api_base(None);

    // The core proof of "proceed": the lane walk ran AND the counter reset.
    // (branch init + the executor both happen before push/PR, so these hold
    // regardless of the remote-side outcome carried by `r2`.)
    assert!(
        ran.load(std::sync::atomic::Ordering::SeqCst),
        "a confirmed-empty gate proceeds: the lane walk runs the executor"
    );
    assert_eq!(counter, 0, "a successful query resets the consecutive-failure counter");
    assert!(
        crate::git::rev_parse(&ws, "agent-q").is_ok(),
        "branch init ran on the proceeding pass: agent-q now exists"
    );
    r2.expect("the proceeding pass completes end-to-end (push + PR mocked)");
}

/// Task 3.2: three consecutive failures post exactly ONE throttled alert; a
/// fourth failure within the throttle window does not re-alert; a success
/// resets the count, and a new failure streak starts fresh (still throttled).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_consecutive_failures_alert_once_then_reset_starts_fresh() {
    let dir = tempfile::TempDir::new().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();

    // ChatOps backend + a mock expecting EXACTLY ONE open-PR-gate alert across
    // the whole test (the third consecutive failure fires it; every later
    // failure is suppressed by the 24h throttle window).
    let mut chatops_server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut chatops_server).await;
    let alert_mock = chatops_server
        .mock("POST", "/chat.postMessage")
        .match_body(mockito::Matcher::Regex(
            "open-PR gate query keeps failing".to_string(),
        ))
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .expect(1)
        .create_async()
        .await;
    let ctx = ChatOpsContext {
        chatops: chatops.clone(),
        channel: "C_TEST".into(),
        start_work_enabled: true,
        failure_alerts_enabled: true,
        pr_opened_enabled: true,
    };

    let repo = open_pr_test_repo();
    let github = open_pr_test_github("");

    let _hook = test_hooks::lock();

    // One gate server that fails (500) and one that confirms an empty list.
    let mut fail_server = mockito::Server::new_async().await;
    let _fail = fail_server
        .mock("GET", mockito::Matcher::Any)
        .with_status(500)
        .with_body(r#"{"message":"github outage"}"#)
        .create_async()
        .await;
    let mut ok_server = mockito::Server::new_async().await;
    let _ok = mock_open_pr_gate_empty(&mut ok_server).await;

    let mut counter = 0u32;

    // Failures 1 & 2: increment, below threshold → no alert yet.
    test_hooks::set_github_api_base(Some(fail_server.url()));
    for expected in [1u32, 2] {
        let proceed =
            open_pr_gate_decision(&paths, &ws, &repo, &github, Some(&ctx), &mut counter).await;
        assert!(!proceed, "a query failure must fail closed (skip)");
        assert_eq!(counter, expected);
    }

    // Failure 3: threshold reached → the single alert fires.
    assert!(!open_pr_gate_decision(&paths, &ws, &repo, &github, Some(&ctx), &mut counter).await);
    assert_eq!(counter, 3);

    // Failure 4: still failing, within the throttle window → NO re-alert.
    assert!(!open_pr_gate_decision(&paths, &ws, &repo, &github, Some(&ctx), &mut counter).await);
    assert_eq!(counter, 4);

    // A successful query resets the consecutive-failure count.
    test_hooks::set_github_api_base(Some(ok_server.url()));
    let proceed =
        open_pr_gate_decision(&paths, &ws, &repo, &github, Some(&ctx), &mut counter).await;
    assert!(proceed, "a confirmed empty list proceeds");
    assert_eq!(counter, 0, "a successful query resets the consecutive-failure count");

    // A new failure streak starts the count FRESH (1, 2, 3 — not 5, 6, 7).
    // The 24h throttle from the first alert still holds, so no re-alert.
    test_hooks::set_github_api_base(Some(fail_server.url()));
    for expected in [1u32, 2, 3] {
        open_pr_gate_decision(&paths, &ws, &repo, &github, Some(&ctx), &mut counter).await;
        assert_eq!(
            counter, expected,
            "the streak restarts from zero after a success"
        );
    }

    test_hooks::set_github_api_base(None);
    // Exactly one alert across both streaks.
    alert_mock.assert_async().await;
}

/// Task 3.3: the incident shape (Abyssum 2026-07-16). An open PR is live on the
/// agent branch, but the pre-check query fails. Fail-closed means we cannot
/// confirm "no open PR", so the pass is skipped: NO executor run starts AND no
/// duplicate PR-open is attempted — the exact double-run this change prevents.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_pr_present_but_failing_query_starts_no_work_and_no_duplicate_pr() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "would-be-duplicated", "the in-flight work");

    let _hook = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    // The open-PR query fails (500). In the incident an open PR was live, but a
    // failing query cannot see it — fail-closed skips regardless.
    let _get = server
        .mock("GET", mockito::Matcher::Regex("/pulls".to_string()))
        .with_status(500)
        .with_body(r#"{"message":"transient"}"#)
        .create_async()
        .await;
    // No duplicate PR-open may be attempted: a POST to /pulls is a failure.
    let no_pr_mock = server
        .mock("POST", mockito::Matcher::Regex("/pulls".to_string()))
        .with_status(201)
        .with_body(r#"{"html_url":"x","number":99}"#)
        .expect(0)
        .create_async()
        .await;
    test_hooks::set_github_api_base(Some(server.url()));

    let github = open_pr_gate_ok_github();
    let mut counter = 0u32;
    let res = execute_one_pass(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &PanicOnRunExecutor,
        &github,
        None,
        None,
        2400u64,
        u32::MAX,
        u32::MAX,
        0,
        Some(10),
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::sync::Mutex::new(Vec::new()),
        &mut counter,
    )
    .await;
    test_hooks::set_github_api_base(None);

    res.expect("a query-failure skip parks the pass and returns Ok");
    assert_eq!(counter, 1, "the failure increments the consecutive-failure counter");
    assert!(
        crate::git::rev_parse(&ws, "agent-q").is_err(),
        "no branch init: agent-q must not be created on a skipped pass"
    );
    // The regression assertion: no executor run (PanicOnRunExecutor never
    // panicked) AND no duplicate PR-open.
    no_pr_mock.assert_async().await;
}
