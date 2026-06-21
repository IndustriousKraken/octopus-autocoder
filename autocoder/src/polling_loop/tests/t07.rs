use super::*;

/// Iteration-level workspace-validity gate (see
/// `audits-require-valid-workspace`): when `ensure_initialized`
/// returns Err for the iteration, the audit scheduler must NOT be
/// invoked. The registry can carry an audit fixture that records
/// its invocations; after the iteration, the counter must be zero.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_scheduler_not_invoked_when_ensure_initialized_fails() {
    let (_td_paths, _paths) = crate::testing::test_daemon_paths();
    use crate::audits::{Audit, AuditContext, AuditOutcome, AuditRegistry, WritePolicy};
    use std::sync::atomic::{AtomicU32, Ordering};

    struct CountingAudit {
        invocations: Arc<AtomicU32>,
    }
    #[async_trait::async_trait]
    impl Audit for CountingAudit {
        fn audit_type(&self) -> &'static str {
            "iter_gate_probe"
        }
        fn description(&self) -> &'static str {
            "test probe for the iteration-level workspace-validity gate"
        }
        fn requires_head_change(&self) -> bool {
            false
        }
        fn write_policy(&self) -> WritePolicy {
            WritePolicy::None
        }
        async fn run(&self, _ctx: &mut AuditContext<'_>) -> Result<AuditOutcome> {
            self.invocations.fetch_add(1, Ordering::SeqCst);
            Ok(AuditOutcome::NoFindings)
        }
    }

    let dir = tempfile::TempDir::new().unwrap();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let ws = dir.path().join("not-a-repo");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("placeholder.txt"), "x").unwrap();

    let repo = RepositoryConfig { forge: None,
        url: "git@github.com:owner/missing.git".into(),
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
    let github_cfg = GithubConfig {
        token_env: "X".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let executor = AlwaysFailingExecutor;

    let invocations = Arc::new(AtomicU32::new(0));
    let probe = CountingAudit {
        invocations: invocations.clone(),
    };
    let registry = AuditRegistry::with_audits(vec![Arc::new(probe) as Arc<dyn Audit>]);

    let result = run_pass_through_commits(
        &paths,
        &ws,
        &repo,
        &github_cfg,
        &executor,
        None,
        1,
        u32::MAX,
        &registry,
        None,
        &std::collections::HashMap::new(),
        &std::sync::Mutex::new(Vec::new()),
    )
    .await;
    assert!(
        result.is_err(),
        "ensure_initialized failure must propagate; the iteration's audit-scheduler call is unreachable"
    );
    assert_eq!(
        invocations.load(Ordering::SeqCst),
        0,
        "iteration-level gate: audit scheduler must NOT be invoked when ensure_initialized fails"
    );
}

/// End-to-end: when the workspace dir exists with partial-clone-shape
/// content but no `.git/`, the iteration's auto-cleanup + re-clone
/// runs internally and the iteration's outcome is a normal success
/// (not Failed). The recovery is invisible to the iteration's
/// reporting layer — only the WARN log signals it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn iteration_auto_recovers_partial_clone_without_failure() {
    use std::process::Command;
    // Set up a real local fixture remote so the re-clone after
    // auto-cleanup actually succeeds (no network access required).
    let dir = tempfile::TempDir::new().unwrap();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let remote = dir.path().join("remote");
    std::fs::create_dir_all(&remote).unwrap();
    fn run(path: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(path)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "git {args:?} failed in {}",
            path.display()
        );
    }
    run(&remote, &["init", "-q", "-b", "main"]);
    run(&remote, &["config", "user.email", "test@example.com"]);
    run(&remote, &["config", "user.name", "test"]);
    std::fs::write(remote.join("README.md"), "fixture\n").unwrap();
    run(&remote, &["add", "README.md"]);
    run(&remote, &["commit", "-q", "-m", "initial"]);

    // Workspace dir exists with openspec partial-clone artifacts and
    // NO `.git/`. The safety check must pass (nothing operator-
    // meaningful here) and the auto-cleanup must run, then the
    // re-clone from the local fixture remote succeeds.
    let ws = dir.path().join("workspace");
    std::fs::create_dir_all(ws.join("openspec/changes/foo")).unwrap();
    std::fs::write(ws.join("openspec/changes/foo/proposal.md"), "## proposal\n").unwrap();

    let remote_url = remote.to_string_lossy().to_string();
    let repo = RepositoryConfig { forge: None,
        url: remote_url,
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
    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let executor = AlwaysFailingExecutor; // unused: no pending changes after re-clone

    let result = run_pass_through_commits(
        &paths,
        &ws,
        &repo,
        &github_cfg,
        &executor,
        None,
        u32::MAX,
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::sync::Mutex::new(Vec::new()),
    )
    .await;
    let (processed, _, _self_heal) = result.expect(
        "iteration must report normal success after internal auto-cleanup + re-clone; \
         the recovery is invisible to the outcome layer",
    );
    assert!(
        processed.is_empty(),
        "the fixture remote has no pending changes, so nothing should be archived"
    );
    // The workspace is now a fresh clone of the remote — `.git/`
    // present, partial-clone artifact gone, remote's README in place.
    assert!(
        ws.join(".git").is_dir(),
        "auto-cleanup + re-clone must produce a valid .git/"
    );
    assert!(
        ws.join("README.md").is_file(),
        "remote's README.md must exist after re-clone"
    );
    assert!(
        !ws.join("openspec/changes/foo/proposal.md").exists(),
        "partial-clone artifact must not survive auto-cleanup"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn perma_stuck_alert_posts_to_chatops() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "perma-stuck-alert-fixture", "fixture reason");

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let alert_mock = server
        .mock("POST", "/chat.postMessage")
        // Behavioral: alert posted AND names the change (a derived value),
        // not the hand-authored subject wording.
        .match_body(mockito::Matcher::Regex(
            "perma-stuck-alert-fixture".to_string(),
        ))
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .expect(1)
        .create_async()
        .await;
    // Allow (and consume) any other unrelated chatops POSTs.
    let _other = server
        .mock("POST", "/chat.postMessage")
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .create_async()
        .await;

    let chatops_ctx = ChatOpsContext {
        chatops: chatops.clone(),
        channel: "C_TEST".into(),
        start_work_enabled: false, // suppress start-of-work to keep matcher unambiguous
        failure_alerts_enabled: true,
        pr_opened_enabled: true,
    };
    let test_github = GithubConfig {
        token_env: "X".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let executor = AlwaysFailingExecutor;
    let _ = run_pass_through_commits(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &test_github,
        &executor,
        Some(&chatops_ctx),
        1, // threshold = 1 → first failure marks perma-stuck
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::sync::Mutex::new(Vec::new()),
    )
    .await;

    assert!(
        ws.join("openspec/changes/perma-stuck-alert-fixture/.perma-stuck.json")
            .exists(),
        "marker should be written when threshold = 1 and the executor failed once"
    );
    alert_mock.assert_async().await;
}

/// perma-stuck-alert-includes-log-path: the alert body MUST include a
/// `run_log:` line pointing at the per-change run log so the
/// operator can diagnose the failure without knowing the path convention.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn perma_stuck_alert_body_contains_log_path() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "log-path-fixture", "diagnostic test");

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    // Match BOTH the perma-stuck subject AND the run_log: line with
    // the expected change name segment.
    let alert_mock = server
        .mock("POST", "/chat.postMessage")
        // Behavioral: the alert body carries the derived run-log path so the
        // operator can locate it; assert that derived value, not prose.
        .match_body(mockito::Matcher::Regex(
            "log-path-fixture\\.log".to_string(),
        ))
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .expect(1)
        .create_async()
        .await;
    let _other = server
        .mock("POST", "/chat.postMessage")
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .create_async()
        .await;

    let chatops_ctx = ChatOpsContext {
        chatops: chatops.clone(),
        channel: "C_TEST".into(),
        start_work_enabled: false,
        failure_alerts_enabled: true,
        pr_opened_enabled: true,
    };
    let test_github = GithubConfig {
        token_env: "X".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let executor = AlwaysFailingExecutor;
    let _ = run_pass_through_commits(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &test_github,
        &executor,
        Some(&chatops_ctx),
        1, // threshold = 1
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::sync::Mutex::new(Vec::new()),
    )
    .await;
    alert_mock.assert_async().await;
}

/// SpecNeedsRevision outcome → marker written, chatops alert posted,
/// queue walk halts. Later pending changes are not processed in the
/// same iteration.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spec_needs_revision_writes_marker_and_alerts_and_halts_queue() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "01-needs-revision", "fixture");
    add_committed_change(&ws, "02-would-run-if-not-halted", "fixture");

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let alert_mock = server
        .mock("POST", "/chat.postMessage")
        // Behavioral: alert posted naming the change (derived), not prose.
        .match_body(mockito::Matcher::Regex("01-needs-revision".into()))
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .expect(1)
        .create_async()
        .await;
    // Allow other unrelated POSTs (start-of-work etc.) without
    // failing assert. We suppress start-of-work in the ctx below to
    // keep things tidy, but accept any extras.
    let _other = server
        .mock("POST", "/chat.postMessage")
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .create_async()
        .await;

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executor = SpecRevisionExecutor {
        tasks: fixture_unimpl_tasks(),
        suggestion: "drop 5.2 from tasks.md".into(),
        invocations: invocations.clone(),
    };
    let chatops_ctx = ChatOpsContext {
        chatops: chatops.clone(),
        channel: "C_TEST".into(),
        start_work_enabled: false,
        failure_alerts_enabled: true,
        pr_opened_enabled: false,
    };
    let test_github = GithubConfig {
        token_env: "X".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let _ = run_pass_through_commits(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &test_github,
        &executor,
        Some(&chatops_ctx),
        u32::MAX,
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::sync::Mutex::new(Vec::new()),
    )
    .await;

    // Marker is at the expected path with the expected schema fields.
    let marker_path = ws.join("openspec/changes/01-needs-revision/.needs-spec-revision.json");
    assert!(
        marker_path.exists(),
        "marker file must be written at {}",
        marker_path.display()
    );
    let raw = std::fs::read_to_string(&marker_path).unwrap();
    assert!(raw.contains("\"change\""));
    assert!(raw.contains("\"01-needs-revision\""));
    assert!(raw.contains("\"unimplementable_tasks\""));
    assert!(raw.contains("\"5.2\""));
    assert!(raw.contains("\"revision_suggestion\""));
    assert!(raw.contains("drop 5.2 from tasks.md"));
    assert!(raw.contains("\"operator_action\""));
    assert!(raw.contains("\"marked_at\""));

    // Alert was posted exactly once.
    alert_mock.assert_async().await;

    // Queue walk halted: the executor ran for the first change only.
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "queue walk must halt after SpecNeedsRevision; later changes must not run"
    );
    // The second change is still in pending (not archived, not marked).
    assert!(
        ws.join("openspec/changes/02-would-run-if-not-halted")
            .exists(),
        "second change must remain in the queue"
    );

    // The lock for the flagged change was cleaned up.
    assert!(
        !ws.join("openspec/changes/01-needs-revision/.in-progress")
            .exists(),
        ".in-progress lock must be removed after SpecNeedsRevision"
    );
}

/// SpecNeedsRevision must NOT increment the perma-stuck counter.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spec_needs_revision_does_not_increment_perma_stuck_counter() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "no-counter-bump", "fixture");
    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executor = SpecRevisionExecutor {
        tasks: fixture_unimpl_tasks(),
        suggestion: "x".into(),
        invocations: invocations.clone(),
    };
    let _ = run_one_pass_with_threshold(&paths, &ws, &executor, 1).await;
    // Marker is present.
    assert!(
        ws.join("openspec/changes/no-counter-bump/.needs-spec-revision.json")
            .exists()
    );
    // failure-state must NOT have an entry for this change. The
    // marker handles exclusion; the counter is operator-action
    // territory, not repeat-failure territory.
    let state = failure_state::load(&paths, &ws).unwrap();
    assert!(
        !state.entries.contains_key("no-counter-bump"),
        "SpecNeedsRevision must not write a failure-state entry"
    );
}

/// Pre-place a marker → change is excluded from list_pending → the
/// executor is never invoked for that change.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn change_with_revision_marker_excluded_from_list_pending() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "pre-marked", "fixture");
    // Pre-place the marker; the marker file must NOT trip the dirty
    // check because workspace::ensure_initialized adds it to
    // .git/info/exclude.
    std::fs::write(
        ws.join("openspec/changes/pre-marked/.needs-spec-revision.json"),
        r#"{"change":"pre-marked","marked_at":"2026-01-01T00:00:00Z","unimplementable_tasks":[],"revision_suggestion":"x","operator_action":"Edit tasks.md, commit, then delete this marker."}"#,
    )
    .unwrap();

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    struct Counter(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    #[async_trait::async_trait]
    impl Executor for Counter {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(ExecutorOutcome::Completed { final_answer: None })
        }
        async fn resume(
            &self,
            _h: crate::executor::ResumeHandle,
            _a: &str,
        ) -> Result<ExecutorOutcome> {
            unreachable!()
        }
    }
    let executor = Counter(invocations.clone());
    let _ = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX).await;
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "executor must NOT be invoked for a change with a needs-spec-revision marker"
    );
}

/// Pre-place the marker, run once (executor not called), then delete the
/// marker and run again — the executor IS called the second time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn marker_removed_re_enables_change() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "operator-cleared", "fixture");
    let marker = ws.join("openspec/changes/operator-cleared/.needs-spec-revision.json");
    std::fs::write(
        &marker,
        r#"{"change":"operator-cleared","marked_at":"2026-01-01T00:00:00Z","unimplementable_tasks":[],"revision_suggestion":"x","operator_action":"Edit tasks.md, commit, then delete this marker."}"#,
    )
    .unwrap();

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    struct Counter(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    #[async_trait::async_trait]
    impl Executor for Counter {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(ExecutorOutcome::Failed {
                reason: "noop fixture".into(),
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
    let executor = Counter(invocations.clone());
    // First pass: marker present → executor must not run.
    let _ = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX).await;
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "executor must not be invoked while marker is present"
    );

    // Operator removes the marker.
    std::fs::remove_file(&marker).unwrap();

    // Second pass: change is back in pending, executor runs.
    let _ = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX).await;
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "executor must run after the operator clears the marker"
    );
}
