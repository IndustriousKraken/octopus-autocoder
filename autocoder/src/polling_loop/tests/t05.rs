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
            0,  // revision_cap: disabled in tests
            10, // human_revise_cap: irrelevant (dispatcher disabled)
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
    assert!(result, "should report PR exists");
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
    assert!(!result, "should report no PR");
    mock.assert_async().await;
}

#[tokio::test]
async fn open_pr_check_returns_false_on_query_error() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", mockito::Matcher::Any)
        .with_status(500)
        .with_body(r#"{"message":"server error"}"#)
        .create_async()
        .await;

    // Best-effort fallback: a 500 from GitHub should not block the
    // iteration — log WARN and proceed as if no PR exists.
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let result = open_pr_exists_for_agent_branch_at(
        &paths,
        &server.url(),
        &open_pr_test_repo(),
        &open_pr_test_github(&server.url()),
    )
    .await;
    assert!(!result, "transport/HTTP errors must degrade to 'no PR'");
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
    assert!(!result);
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
    let (processed, _) = run_pass_through_commits(
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
    let (processed, _) = run_pass_through_commits(
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
    let github = GithubConfig {
        token_env: "X".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };

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
        10, // human_revise_cap: irrelevant (dispatcher disabled)
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::sync::Mutex::new(Vec::new()),
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

    alert_mock.assert_async().await;
}
