use super::*;

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

    let repo = RepositoryConfig { forge: None,
        url: "git@github.com:owner/empty.git".into(),
        local_path: Some(ws.clone()),
        base_branch: "main".into(),
        agent_branch: "agent-q".into(),
        poll_interval_sec: 60,
        chatops_channel_id: None,
        max_changes_per_pr: None,
        audits: None,
        spec_storage: None,
        upstream: None,
        auto_submit_pr: true,
        octopus_guide: None,
        sandbox: None,
    };
    let github = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let cancel = CancellationToken::new();
    let executor: Arc<dyn Executor> = Arc::new(AlwaysFails);

    let cancel_for_task = cancel.clone();
    let github_holder: GithubHolder = Arc::new(arc_swap::ArcSwap::from_pointee(github));
    let reviewer_holder: ReviewerHolder = Arc::new(arc_swap::ArcSwap::from_pointee(None));
    let chatops_holder: ChatOpsHolder = Arc::new(arc_swap::ArcSwap::from_pointee(None));
    let cache_holder: CacheHolder = Arc::new(arc_swap::ArcSwap::from_pointee(
        crate::config::CacheConfig::default(),
    ));
    let repo_holder: Arc<ArcSwap<RepositoryConfig>> = Arc::new(ArcSwap::from_pointee(repo));
    let iteration_sleep = Arc::new(tokio::sync::Notify::new());
    let hooks = RunHooks {
        on_iteration_sleep: Some(iteration_sleep.clone()),
        fork_ops: None,
    };
    let paths_for_run = std::sync::Arc::new(crate::testing::test_daemon_paths().1);
    // Serialize on the github-api-base test hook: the spawned `run_with_hooks`
    // loop reads the process-wide override via its open-PR pre-check. Hold the
    // guard in the test body (NOT moved into the task) so it serializes against
    // other tests that install the override. See `test_hooks::lock`.
    let _hook = test_hooks::lock();
    let handle = tokio::spawn(async move {
        run_with_hooks(
            paths_for_run,
            repo_holder,
            executor,
            github_holder,
            reviewer_holder,
            chatops_holder,
            cache_holder,
            2400,
            u32::MAX,
            Some(u32::MAX),
            0,  // revision_cap: disabled in tests
            Some(10), // human_revise_cap: irrelevant (dispatcher disabled)
            0,  // startup_jitter_max_secs: deterministic for tests
            0,  // inter_iteration_jitter_pct: deterministic for tests
            std::sync::Arc::new(crate::audits::AuditRegistry::default()),
            None,
            std::sync::Arc::new(std::collections::HashMap::new()),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            crate::control_socket::RevisionRequestQueues::new(),
            std::sync::Arc::new(std::sync::Mutex::new(None)),
            std::sync::Arc::new(tokio::sync::Notify::new()),
            cancel_for_task,
            false, // fork_pending: normal task
            hooks,
        )
        .await;
    });

    // Wait event-driven for the loop to reach its inter-iteration
    // sleep — the `on_iteration_sleep` hook fires immediately before
    // the select! enters the sleep, so a cancel after this notify is
    // guaranteed to race against the sleep branch (the case under
    // test). The 5s wall-clock cap is a guardrail, not a poll interval.
    tokio::time::timeout(Duration::from_secs(5), iteration_sleep.notified())
        .await
        .expect("polling loop did not reach inter-iteration sleep within 5s");
    cancel.cancel();

    // The loop must exit within 1s of cancellation. The 60s sleep would
    // otherwise dominate.
    let res = tokio::time::timeout(Duration::from_secs(1), handle).await;
    assert!(res.is_ok(), "polling loop did not exit within 1s of cancel");
}

/// startup-fork-setup-retries-transient-failures: the loop-level fork-pending
/// driver must EXIT the polling set when a re-attempt classifies Permanent.
/// This covers the `if fork_pending { … }` wiring in `run_with_hooks` (the
/// `ForkPendingStep::Permanent => break` arm) that the isolated
/// `run_fork_pending_iteration` tests can't reach. A scripted `ForkOps` is
/// injected via the `RunHooks::fork_ops` override; its create returns a
/// mismatched fork identity (→ permanent, no reachability poll), so the task
/// must return on its OWN with no cancellation — a wiring bug (Permanent not
/// breaking) would sleep past the timeout, and `NeverRuns` would panic if the
/// loop wrongly fell through to normal executor work.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fork_pending_loop_breaks_on_permanent_reattempt() {
    use crate::executor::ResumeHandle;
    use async_trait::async_trait;

    // Fork never reachable → the re-attempt issues a create POST; the create
    // names a DIFFERENT fork than the derived `mu/a` → identity mismatch →
    // PERMANENT. The mismatch records the failure before the 60s reachability
    // poll, so the test is fast.
    struct MismatchOnCreate;
    #[async_trait]
    impl crate::cli::run::ForkOps for MismatchOnCreate {
        fn fork_reachable(&self, _fork_url: &str) -> bool {
            false
        }
        async fn create_fork(
            &self,
            _upstream_owner: &str,
            _upstream_repo: &str,
            _token: &str,
        ) -> Result<Option<String>> {
            Ok(Some("mu/renamed".into()))
        }
    }

    // Fork-pending never runs the executor; a call here means the loop wrongly
    // left the fork-pending state.
    struct NeverRuns;
    #[async_trait]
    impl Executor for NeverRuns {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            unreachable!("fork-pending iterations never run the executor");
        }
        async fn resume(&self, _h: ResumeHandle, _a: &str) -> Result<ExecutorOutcome> {
            unreachable!()
        }
    }

    let repo = RepositoryConfig {
        forge: None,
        url: "git@github.com:orgA/a.git".into(),
        local_path: None,
        base_branch: "main".into(),
        agent_branch: "agent-q".into(),
        poll_interval_sec: 60,
        chatops_channel_id: None,
        max_changes_per_pr: None,
        audits: None,
        spec_storage: None,
        upstream: None,
        auto_submit_pr: true,
        octopus_guide: None,
        sandbox: None,
    };
    // fork_owner set + an inline PAT so fork-URL derivation AND PAT resolution
    // both succeed and the driver reaches the create POST.
    let github = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: Some(crate::config::SecretSource::Inline {
            value: "inline-fork-pat".into(),
        }),
        owner_tokens: None,
        fork_owner: Some("mu".into()),
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };

    let cancel = CancellationToken::new();
    let executor: Arc<dyn Executor> = Arc::new(NeverRuns);
    let cancel_for_task = cancel.clone();
    let github_holder: GithubHolder = Arc::new(arc_swap::ArcSwap::from_pointee(github));
    let reviewer_holder: ReviewerHolder = Arc::new(arc_swap::ArcSwap::from_pointee(None));
    let chatops_holder: ChatOpsHolder = Arc::new(arc_swap::ArcSwap::from_pointee(None));
    let cache_holder: CacheHolder = Arc::new(arc_swap::ArcSwap::from_pointee(
        crate::config::CacheConfig::default(),
    ));
    let repo_holder: Arc<ArcSwap<RepositoryConfig>> = Arc::new(ArcSwap::from_pointee(repo));
    let hooks = RunHooks {
        on_iteration_sleep: None,
        fork_ops: Some(Arc::new(MismatchOnCreate)),
    };
    let paths_for_run = std::sync::Arc::new(crate::testing::test_daemon_paths().1);

    let handle = tokio::spawn(async move {
        run_with_hooks(
            paths_for_run,
            repo_holder,
            executor,
            github_holder,
            reviewer_holder,
            chatops_holder,
            cache_holder,
            2400,
            u32::MAX,
            Some(u32::MAX),
            0,
            Some(10),
            0,
            0,
            std::sync::Arc::new(crate::audits::AuditRegistry::default()),
            None,
            std::sync::Arc::new(std::collections::HashMap::new()),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            crate::control_socket::RevisionRequestQueues::new(),
            std::sync::Arc::new(std::sync::Mutex::new(None)),
            std::sync::Arc::new(tokio::sync::Notify::new()),
            cancel_for_task,
            true, // fork_pending: start in the fork-pending state
            hooks,
        )
        .await;
    });

    // The Permanent arm breaks on the FIRST iteration — no cancellation. The
    // 5s cap is a guardrail; a correct loop returns almost immediately.
    let res = tokio::time::timeout(Duration::from_secs(5), handle).await;
    assert!(
        res.is_ok(),
        "fork-pending loop must break by itself on a permanent re-attempt"
    );
}

#[test]
fn compose_branch_url_formats_github_tree_url() {
    // No `forge:` block + a github.com URL → GitHub branch tree URL.
    assert_eq!(
        compose_branch_url(
            None,
            "https://github.com/upstream-owner/upstream-repo.git",
            "upstream-owner",
            "upstream-repo",
            "agent-q"
        ),
        "https://github.com/upstream-owner/upstream-repo/tree/agent-q"
    );
}

#[test]
fn compose_branch_url_uses_gitlab_mr_hint_under_gitlab_block() {
    let forge = crate::config::ForgeConfig {
        kind: crate::config::ForgeKind::Gitlab,
        host: Some("gitlab.example.com".into()),
        api_base: None,
        token: None,
        token_env: None,
    };
    assert_eq!(
        compose_branch_url(
            Some(&forge),
            "https://gitlab.example.com/group/proj.git",
            "group",
            "proj",
            "agent-q"
        ),
        "https://gitlab.example.com/group/proj/-/merge_requests/new?merge_request%5Bsource_branch%5D=agent-q"
    );
}

#[test]
fn push_only_command_is_forge_specific() {
    use crate::polling_loop::alerts_notify::push_only_command;
    assert_eq!(
        push_only_command(None, "main", "agent-q"),
        "gh pr create --base main --head agent-q"
    );
    let gitlab = crate::config::ForgeConfig {
        kind: crate::config::ForgeKind::Gitlab,
        host: None,
        api_base: None,
        token: None,
        token_env: None,
    };
    assert_eq!(
        push_only_command(Some(&gitlab), "main", "agent-q"),
        "glab mr create --target-branch main --source-branch agent-q"
    );
}

#[test]
fn auto_submit_pr_defaults_to_true_on_fixture() {
    let repo = open_pr_test_repo();
    assert!(repo.auto_submit_pr);
}

#[test]
fn suggested_pr_command_picks_upstream_branch_when_configured() {
    // When upstream is set, the suggested gh pr create base is
    // upstream.branch.
    let mut repo = open_pr_test_repo();
    repo.upstream = Some(crate::config::UpstreamConfig {
        remote: "upstream".to_string(),
        branch: "trunk".to_string(),
        url: "https://github.com/up/repo.git".to_string(),
    });
    let pr_base = repo
        .upstream
        .as_ref()
        .map(|u| u.branch.as_str())
        .unwrap_or(&repo.base_branch);
    assert_eq!(pr_base, "trunk");
}

#[test]
fn suggested_pr_command_falls_back_to_base_branch_when_no_upstream() {
    let repo = open_pr_test_repo();
    let pr_base = repo
        .upstream
        .as_ref()
        .map(|u| u.branch.as_str())
        .unwrap_or(&repo.base_branch);
    assert_eq!(pr_base, "main");
}

#[tokio::test]
async fn open_pr_check_returns_true_when_pr_exists() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock(
            "GET",
            "/repos/upstream-owner/upstream-repo/pulls?state=open&head=upstream-owner%3Aagent-q&base=main",
        )
        .with_status(200)
        .with_body(
            r#"[{"number":7,"html_url":"https://github.com/upstream-owner/upstream-repo/pull/7"}]"#,
        )
        .expect(1)
        .create_async()
        .await;

    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let result = open_pr_exists_for_agent_branch_at(
        &paths,
        &server.url(),
        &open_pr_test_repo(),
        &open_pr_test_github(&server.url()),
    )
    .await;
    assert!(
        matches!(result, OpenPrGateOutcome::Open),
        "should report PR exists (Open), got {result:?}"
    );
    mock.assert_async().await;
}

#[tokio::test]
async fn open_pr_check_returns_false_when_no_pr() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock(
            "GET",
            "/repos/upstream-owner/upstream-repo/pulls?state=open&head=upstream-owner%3Aagent-q&base=main",
        )
        .with_status(200)
        .with_body("[]")
        .expect(1)
        .create_async()
        .await;

    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let result = open_pr_exists_for_agent_branch_at(
        &paths,
        &server.url(),
        &open_pr_test_repo(),
        &open_pr_test_github(&server.url()),
    )
    .await;
    assert!(
        matches!(result, OpenPrGateOutcome::NoPr),
        "empty list should report no PR (NoPr → proceed), got {result:?}"
    );
    mock.assert_async().await;
}

#[tokio::test]
async fn open_pr_check_returns_unknown_on_query_error() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", mockito::Matcher::Any)
        .with_status(500)
        .with_body(r#"{"message":"server error"}"#)
        .create_async()
        .await;

    // Fail closed (open-pr-gate-fails-closed): a 500 from GitHub is an
    // UNCONFIRMED answer, so the gate returns `Unknown` and the caller skips
    // the iteration rather than risking a duplicate agentic run.
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let result = open_pr_exists_for_agent_branch_at(
        &paths,
        &server.url(),
        &open_pr_test_repo(),
        &open_pr_test_github(&server.url()),
    )
    .await;
    assert!(
        matches!(result, OpenPrGateOutcome::Unknown(_)),
        "transport/HTTP errors must fail closed (Unknown → skip), got {result:?}"
    );
}

#[tokio::test]
async fn open_pr_check_uses_fork_owner_in_head_qualifier() {
    // With fork_owner = "bot-machine-user", the head query parameter
    // must be `bot-machine-user:agent-q` (not the upstream owner).
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock(
            "GET",
            "/repos/upstream-owner/upstream-repo/pulls?state=open&head=bot-machine-user%3Aagent-q&base=main",
        )
        .with_status(200)
        .with_body("[]")
        .expect(1)
        .create_async()
        .await;

    let mut gh = open_pr_test_github(&server.url());
    gh.fork_owner = Some("bot-machine-user".into());
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let result =
        open_pr_exists_for_agent_branch_at(&paths, &server.url(), &open_pr_test_repo(), &gh).await;
    assert!(
        matches!(result, OpenPrGateOutcome::NoPr),
        "empty list should report no PR (NoPr), got {result:?}"
    );
    mock.assert_async().await;
}

/// Start-of-work notification fires once when a pending change is
/// dequeued. The mockito server is matched on a body fragment so the
/// test doesn't care about JSON-key ordering or how `text` is encoded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_of_work_notification_posted_on_dequeue() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "feature-start-of-work", "make work observable");

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let start_mock = server
        .mock("POST", "/chat.postMessage")
        .match_body(mockito::Matcher::PartialJsonString(
            serde_json::json!({
                "channel": "C_TEST",
                "text": "🚀 `git@github.com:owner/fixture.git`: starting work on `feature-start-of-work` — make work observable"
            })
            .to_string(),
        ))
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .expect(1)
        .create_async()
        .await;

    let executor = CompletingExecutorWithDiff {
        artifact_name: "SOWA.txt".into(),
        artifact_text: "x".into(),
    };
    let chatops_ctx = ChatOpsContext {
        chatops: chatops.clone(),
        channel: "C_TEST".into(),
        start_work_enabled: true,
        failure_alerts_enabled: true,
        pr_opened_enabled: true,
    };
    let github = GithubConfig {
        token_env: "X".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let (processed, _, _) = run_pass_through_commits(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &github,
        &executor,
        Some(&chatops_ctx),
        u32::MAX,
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::sync::Mutex::new(Vec::new()),
    )
    .await
    .expect("pass succeeds");
    assert_eq!(processed, vec!["feature-start-of-work".to_string()]);
    start_mock.assert_async().await;
}

/// When `start_work_enabled` is false the mock receives zero calls even
/// though chatops is otherwise wired.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_of_work_suppressed_when_disabled() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "feature-suppressed", "should not be announced");

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let no_post_mock = server
        .mock("POST", "/chat.postMessage")
        .expect(0)
        .create_async()
        .await;

    let executor = CompletingExecutorWithDiff {
        artifact_name: "SUPPRESSED.txt".into(),
        artifact_text: "x".into(),
    };
    let chatops_ctx = ChatOpsContext {
        chatops: chatops.clone(),
        channel: "C_TEST".into(),
        start_work_enabled: false, // disabled
        failure_alerts_enabled: true,
        pr_opened_enabled: true,
    };
    let github = GithubConfig {
        token_env: "X".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let (processed, _, _) = run_pass_through_commits(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &github,
        &executor,
        Some(&chatops_ctx),
        u32::MAX,
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::sync::Mutex::new(Vec::new()),
    )
    .await
    .expect("pass succeeds");
    assert_eq!(processed, vec!["feature-suppressed".to_string()]);
    no_post_mock.assert_async().await;
}

/// 24h throttle: the first push failure posts; a second pass within
/// the throttle window does NOT post.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failure_alert_posted_then_suppressed_within_24h() {
    let (_dir, ws) = fixture_workspace_with_broken_remote("alert-throttle");
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "needs-push", "push-failure fixture");

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    // Exactly one alert post across two iterations.
    let alert_mock = server
        .mock("POST", "/chat.postMessage")
        .match_body(mockito::Matcher::Regex(
            "branch push keeps failing".to_string(),
        ))
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .expect(1)
        .create_async()
        .await;
    // Start-of-work posts are unrelated and may fire any number of
    // times; allow them.
    let _start_work_mock = server
        .mock("POST", "/chat.postMessage")
        .match_body(mockito::Matcher::Regex("starting work on".to_string()))
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .create_async()
        .await;

    // Fail-closed open-PR gate: answer the pre-check with an empty list so the
    // iteration proceeds to the push step under test. Served by the same
    // mockito server as chatops.
    let _gate_mock = mock_open_pr_gate_empty(&mut server).await;

    let executor = CompletingExecutorWithDiff {
        artifact_name: "PUSH_ART.txt".into(),
        artifact_text: "x".into(),
    };
    let chatops_ctx = ChatOpsContext {
        chatops: chatops.clone(),
        channel: "C_TEST".into(),
        start_work_enabled: true,
        failure_alerts_enabled: true,
        pr_opened_enabled: true,
    };
    let github = open_pr_gate_ok_github();

    // Serialize on the github-api-base test hook: `execute_one_pass` reads the
    // process-wide override via its open-PR pre-check, so this test must not run
    // while another test has the override installed (else the pre-check request
    // would land on that test's mockito server). See `test_hooks::lock`.
    let _hook = test_hooks::lock();
    test_hooks::set_github_api_base(Some(server.url()));

    // Iteration 1: pass through commits succeeds, push fails → alert
    // is posted and `.alert-state.json` is written.
    let stuck_secs = 2400u64;
    let _ = execute_one_pass(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &executor,
        &github,
        None,
        Some(&chatops_ctx),
        stuck_secs,
        u32::MAX,
        u32::MAX,
        0,  // revision_cap: disabled in tests
        Some(10), // human_revise_cap: irrelevant (dispatcher disabled)
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::sync::Mutex::new(Vec::new()),
        &mut 0u32,
    )
    .await;
    let basename = ws.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        paths.alert_state_path(&basename).exists(),
        "iter 1's push failure must persist alert state"
    );

    // Iteration 2: invoke `handle_predictable_failure` directly with a
    // synthesized push error. State is loaded from disk; the entry is
    // recent (< 24h), so should_alert is false → no post, mock counter
    // stays at 1. This is the throttle assertion: a repeat failure
    // within the window is silent.
    crate::alerts::handle_predictable_failure(
        &paths,
        &ws,
        &fixture_repo(&ws).url,
        Some(&chatops_ctx),
        true,
        crate::alert_state::AlertCategory::BranchPushFailure,
        &anyhow!("simulated repeat push failure"),
    )
    .await;

    test_hooks::set_github_api_base(None);
    alert_mock.assert_async().await;
}

/// push-failure-preserves-completed-work: a push failure after a change is
/// committed writes a workspace-keyed push-block marker (slug + tip) and leaves
/// the completed work intact on the agent branch (never reset).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_failure_writes_marker_and_preserves_work() {
    let (_dir, ws) = fixture_workspace_with_broken_remote("pushblock-write");
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "needs-push", "push-block fixture");

    let executor = CompletingExecutorWithDiff {
        artifact_name: "PB_ART.txt".into(),
        artifact_text: "x".into(),
    };
    let github = open_pr_gate_ok_github();
    let _hook = test_hooks::lock();
    // Fail-closed open-PR gate: proceed past the pre-check to the push step.
    let mut server = mockito::Server::new_async().await;
    let _gate_mock = mock_open_pr_gate_empty(&mut server).await;
    test_hooks::set_github_api_base(Some(server.url()));

    // Push fails (broken remote) AFTER the change is committed + archived.
    let res = execute_one_pass(
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
        &mut 0u32,
    )
    .await;
    test_hooks::set_github_api_base(None);
    assert!(res.is_err(), "broken-remote push must fail the iteration");

    let marker = crate::push_block::read(&paths, &ws)
        .expect("push failure must write a push-block marker");
    assert!(
        marker.change_slugs.iter().any(|s| s == "needs-push"),
        "marker records the carried change slug: {:?}",
        marker.change_slugs
    );
    assert!(!marker.tip_commit.is_empty(), "marker records the branch tip");

    // The completed work is preserved on the agent branch: its tip still equals
    // the recorded marker tip (the push failure did NOT reset the branch).
    let agent = fixture_repo(&ws).agent_branch;
    let live_tip = crate::git::rev_parse(&ws, &agent).unwrap();
    assert_eq!(
        live_tip, marker.tip_commit,
        "agent branch must be preserved (not reset) after a push failure"
    );
}

/// push-failure-preserves-completed-work: with a matching push-block marker
/// present, the next pass resumes at the push step and NEVER re-runs the
/// executor (the work is already committed).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_block_resume_skips_executor() {
    use crate::executor::ResumeHandle;
    use async_trait::async_trait;

    // Iteration-2 executor: its `run` must never be called on a resume.
    struct PanicOnRun;
    #[async_trait]
    impl Executor for PanicOnRun {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            panic!("executor must not run while a push-block hold is active");
        }
        async fn resume(&self, _h: ResumeHandle, _a: &str) -> Result<ExecutorOutcome> {
            unreachable!()
        }
    }

    let (_dir, ws) = fixture_workspace_with_broken_remote("pushblock-resume");
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "needs-push", "push-block fixture");
    let github = open_pr_gate_ok_github();
    let _hook = test_hooks::lock();
    // Fail-closed open-PR gate: iter 1 proceeds past the pre-check to push.
    // (Iter 2 resumes the push-block hold BEFORE the gate, so it never queries.)
    let mut server = mockito::Server::new_async().await;
    let _gate_mock = mock_open_pr_gate_empty(&mut server).await;
    test_hooks::set_github_api_base(Some(server.url()));

    // Iteration 1: complete the change, push fails → marker written.
    let exec1 = CompletingExecutorWithDiff {
        artifact_name: "PB_ART.txt".into(),
        artifact_text: "x".into(),
    };
    let r1 = execute_one_pass(
        &paths, &ws, &fixture_repo(&ws), &exec1, &github, None, None,
        2400u64, u32::MAX, u32::MAX, 0, Some(10),
        &crate::audits::AuditRegistry::default(), None,
        &std::collections::HashMap::new(), &std::sync::Mutex::new(Vec::new()),
        &mut 0u32,
    )
    .await;
    assert!(r1.is_err());
    assert!(crate::push_block::exists(&paths, &ws), "iter 1 writes the marker");

    // Iteration 2: marker present + tip matches → resume path. PanicOnRun proves
    // the executor is not invoked. Push fails again (broken remote) → Err, marker
    // refreshed and retained.
    let r2 = execute_one_pass(
        &paths, &ws, &fixture_repo(&ws), &PanicOnRun, &github, None, None,
        2400u64, u32::MAX, u32::MAX, 0, Some(10),
        &crate::audits::AuditRegistry::default(), None,
        &std::collections::HashMap::new(), &std::sync::Mutex::new(Vec::new()),
        &mut 0u32,
    )
    .await;
    test_hooks::set_github_api_base(None);
    assert!(r2.is_err(), "push still fails on the resume retry");
    assert!(
        crate::push_block::exists(&paths, &ws),
        "marker retained while the push keeps failing"
    );
}

/// push-failure-preserves-completed-work: a STALE marker (its tip no longer
/// matches the agent branch — e.g. a stale post-merge branch) is removed and the
/// pass proceeds normally (recreate + run executor), rather than wrongly
/// preserving and force-pushing stale work.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_push_block_marker_is_cleared_and_pass_proceeds() {
    let (_dir, ws) = fixture_workspace_with_broken_remote("pushblock-stale");
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "needs-push", "push-block fixture");

    // Plant a marker whose tip will NOT match the live agent branch.
    crate::push_block::write(
        &paths,
        &ws,
        &crate::push_block::PushBlock {
            tip_commit: "0000000000000000000000000000000000000000".into(),
            change_slugs: vec!["already-merged".into()],
            reason: "stale".into(),
            blocked_at: chrono::Utc::now(),
            review_report: None,
            spec_verification_section: None,
            gate_verdicts_section: None,
        },
    )
    .unwrap();

    let executor = CompletingExecutorWithDiff {
        artifact_name: "PB_ART.txt".into(),
        artifact_text: "x".into(),
    };
    let github = open_pr_gate_ok_github();
    let _hook = test_hooks::lock();
    // Fail-closed open-PR gate: the stale marker is cleared, then the normal
    // flow proceeds past the pre-check to push (which fails on the broken remote).
    let mut server = mockito::Server::new_async().await;
    let _gate_mock = mock_open_pr_gate_empty(&mut server).await;
    test_hooks::set_github_api_base(Some(server.url()));

    let res = execute_one_pass(
        &paths, &ws, &fixture_repo(&ws), &executor, &github, None, None,
        2400u64, u32::MAX, u32::MAX, 0, Some(10),
        &crate::audits::AuditRegistry::default(), None,
        &std::collections::HashMap::new(), &std::sync::Mutex::new(Vec::new()),
        &mut 0u32,
    )
    .await;
    test_hooks::set_github_api_base(None);
    assert!(res.is_err(), "broken-remote push still fails the iteration");

    // The stale marker was cleared and the normal flow ran (recreate + executor),
    // so the marker now reflects the freshly-processed change, not the stale one.
    let marker = crate::push_block::read(&paths, &ws).expect("normal flow re-failed the push");
    assert_ne!(
        marker.tip_commit, "0000000000000000000000000000000000000000",
        "stale marker must have been replaced by a fresh one"
    );
    assert!(
        marker.change_slugs.iter().any(|s| s == "needs-push"),
        "fresh marker carries the reprocessed change: {:?}",
        marker.change_slugs
    );
}
