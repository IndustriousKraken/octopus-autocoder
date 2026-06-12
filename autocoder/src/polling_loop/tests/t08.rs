use super::*;

/// a18: A perma-stuck change on the queue blocks subsequent pending
/// changes in the same repo. The pending sibling's executor is never
/// invoked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn perma_stuck_marker_blocks_subsequent_pending_change() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "01-broken", "fixture");
    add_committed_change(&ws, "02-sibling", "fixture");
    // Pre-place the perma-stuck marker on the first change.
    std::fs::write(
        ws.join("openspec/changes/01-broken/.perma-stuck.json"),
        r#"{"change":"01-broken","consecutive_failures":2,"last_reason":"x","marked_stuck_at":"2026-01-01T00:00:00Z","operator_action":"Delete this file to retry the change."}"#,
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
        "queue must halt on perma-stuck; pending sibling must not be processed"
    );
    // Sibling is still on disk waiting to run next time.
    assert!(
        ws.join("openspec/changes/02-sibling/proposal.md").exists(),
        "sibling change must remain in the queue"
    );
}

/// a18: When `.ignore-for-queue.json` accompanies the blocking
/// marker, the queue walk RESUMES — the pending sibling IS processed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ignore_for_queue_marker_unblocks_subsequent_pending_change() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "01-broken", "fixture");
    add_committed_change(&ws, "02-sibling", "fixture");
    // Perma-stuck marker on the first change.
    std::fs::write(
        ws.join("openspec/changes/01-broken/.perma-stuck.json"),
        r#"{"change":"01-broken","consecutive_failures":2,"last_reason":"x","marked_stuck_at":"2026-01-01T00:00:00Z","operator_action":"x"}"#,
    )
    .unwrap();
    // AND the ignore-for-queue downgrade marker.
    std::fs::write(
        ws.join("openspec/changes/01-broken/.ignore-for-queue.json"),
        r#"{"change":"01-broken","marked_at":"2026-01-01T00:00:00Z","marked_by":"U_OP","reason":"x","operator_action":"x"}"#,
    )
    .unwrap();

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let invoked_with = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    struct Counter {
        count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }
    #[async_trait::async_trait]
    impl Executor for Counter {
        async fn run(&self, _w: &Path, change: &str) -> Result<ExecutorOutcome> {
            self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.seen.lock().unwrap().push(change.to_string());
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
    let executor = Counter {
        count: invocations.clone(),
        seen: invoked_with.clone(),
    };
    let _ = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX).await;
    let seen = invoked_with.lock().unwrap().clone();
    assert!(
        !seen.contains(&"01-broken".to_string()),
        "perma-stuck change must still be excluded; got {seen:?}"
    );
    assert!(
        seen.contains(&"02-sibling".to_string()),
        "ignore-for-queue must let the sibling proceed; got {seen:?}"
    );
}

/// a18: `.needs-spec-revision.json` continues to block the queue
/// (unchanged behavior — confirms the new pre-walk gate matches the
/// existing per-iteration behavior).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn needs_spec_revision_marker_blocks_subsequent_pending_change() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "01-revision", "fixture");
    add_committed_change(&ws, "02-sibling", "fixture");
    std::fs::write(
        ws.join("openspec/changes/01-revision/.needs-spec-revision.json"),
        r#"{"change":"01-revision","marked_at":"2026-01-01T00:00:00Z","unimplementable_tasks":[],"revision_suggestion":"x","operator_action":"x"}"#,
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
        "queue must halt on needs-spec-revision; pending sibling must not be processed"
    );
}

/// a18: A workspace with no operator-action markers proceeds normally
/// — the new pre-walk gate is a no-op.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clean_workspace_processes_pending_changes_normally() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "only-pending", "fixture");

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
        1,
        "clean queue must process the pending change"
    );
}

/// A change whose MODIFIED header doesn't match canonical (the a07
/// failure mode) is caught by the pre-flight: the executor is NOT
/// invoked, a `.needs-spec-revision.json` marker is written with
/// `unarchivable_deltas` populated, and the queue walk halts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preflight_catches_a07_style_modified_mismatch() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_canonical_spec(
        &ws,
        "code-reviewer",
        "## Requirements\n\n### Requirement: AI-driven code-quality review\nThe reviewer SHALL accept.\n",
    );
    add_committed_change_with_spec(
        &ws,
        "a07-style-broken",
        "code-reviewer",
        "## MODIFIED Requirements\n\n### Requirement: Reviewer prompt budget is operator-configurable\nThe reviewer SHALL read.\n",
    );
    // A clean change that would run if the pre-flight didn't halt
    // the queue. Its presence verifies the same-iteration halt
    // semantics.
    add_committed_change(&ws, "b-runs-if-not-halted", "fixture");

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

    // Executor must NOT have been invoked for the broken change.
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "pre-flight must halt before any executor invocation"
    );

    // Marker is at the expected path with `unarchivable_deltas`
    // populated and an auto-generated suggestion.
    let marker_path = ws.join("openspec/changes/a07-style-broken/.needs-spec-revision.json");
    assert!(marker_path.exists(), "marker must be written");
    let raw = std::fs::read_to_string(&marker_path).unwrap();
    assert!(raw.contains("\"unarchivable_deltas\""));
    assert!(raw.contains("\"code-reviewer\""));
    assert!(raw.contains("\"Modified\""));
    assert!(raw.contains("Reviewer prompt budget is operator-configurable"));
    assert!(
        raw.contains("a07-style"),
        "auto-generated reason should mention a07-style class"
    );
    assert!(raw.contains("\"revision_suggestion\""));
    assert!(
        raw.contains("Pre-flight check found"),
        "revision_suggestion should lead with pre-flight prefix"
    );

    // Marker excludes the change from list_pending. The clean
    // second change is in the same iteration; the queue walk halts
    // on the first marker write, so it must NOT have been processed.
    assert!(
        ws.join("openspec/changes/b-runs-if-not-halted").exists(),
        "the clean trailing change must remain in pending"
    );
}

/// A change whose spec deltas are clean against canonical passes
/// pre-flight and reaches the executor — the existing behavior is
/// preserved.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preflight_passes_clean_change_through_to_executor() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_canonical_spec(
        &ws,
        "code-reviewer",
        "## Requirements\n\n### Requirement: AI-driven code-quality review\nThe reviewer SHALL accept.\n",
    );
    add_committed_change_with_spec(
        &ws,
        "clean-modify",
        "code-reviewer",
        "## MODIFIED Requirements\n\n### Requirement: AI-driven code-quality review\nReplacement body SHALL.\n",
    );

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    struct Counter(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    #[async_trait::async_trait]
    impl Executor for Counter {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
    let executor = Counter(invocations.clone());

    let _ = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX).await;

    // Executor IS invoked: pre-flight was a no-op for the clean change.
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "clean change must reach the executor"
    );
    // No marker written.
    assert!(
        !ws.join("openspec/changes/clean-modify/.needs-spec-revision.json")
            .exists(),
        "no marker for clean change"
    );
}

/// The pre-flight chatops alert fires with body framing the failure
/// as "unarchivable spec deltas" and enumerating each violation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preflight_failure_posts_chatops_alert_with_deltas_body() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_canonical_spec(
        &ws,
        "code-reviewer",
        "## Requirements\n\n### Requirement: AI-driven code-quality review\nThe reviewer SHALL accept.\n",
    );
    add_committed_change_with_spec(
        &ws,
        "alerted-broken",
        "code-reviewer",
        "## MODIFIED Requirements\n\n### Requirement: Invented Title\nBody SHALL.\n",
    );

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let alert_mock = server
        .mock("POST", "/chat.postMessage")
        .match_body(mockito::Matcher::Regex("unarchivable spec deltas".into()))
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

    struct Noop;
    #[async_trait::async_trait]
    impl Executor for Noop {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            unreachable!("pre-flight must short-circuit before executor.run")
        }
        async fn resume(
            &self,
            _h: crate::executor::ResumeHandle,
            _a: &str,
        ) -> Result<ExecutorOutcome> {
            unreachable!()
        }
    }
    let executor = Noop;
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

    alert_mock.assert_async().await;
}

/// Disabled mode: no scoped context (or explicit `None`) → no LLM
/// call, executor reached normally.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn contradiction_preflight_disabled_proceeds_to_executor() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "plain", "fixture");

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    struct Counter(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    #[async_trait::async_trait]
    impl Executor for Counter {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
    let executor = Counter(invocations.clone());
    let fut = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX);
    let _ = crate::preflight::change_contradiction::scope(None, fut).await;
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "executor must be invoked when contradiction check is disabled"
    );
}

/// Enabled mode + LLM returns empty contradictions → executor still
/// reached (the check is a no-op outcome-wise).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn contradiction_preflight_empty_findings_proceeds_to_executor() {
    let ctx = cc_test_ctx(Some(serde_json::json!({ "contradictions": [] })), None);
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    // Change has a spec delta so the session has something to read,
    // but archivability check passes (no canonical to fight with).
    add_committed_change_with_spec(
        &ws,
        "clean",
        "newcap",
        "## ADDED Requirements\n\n### Requirement: A\nThe system SHALL a.\n",
    );

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    struct Counter(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    #[async_trait::async_trait]
    impl Executor for Counter {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
    let executor = Counter(invocations.clone());
    let fut = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX);
    let _ =
        crate::preflight::change_contradiction::scope(Some(std::sync::Arc::new(ctx)), fut).await;
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "executor must be invoked when contradiction check returns empty findings"
    );
}

/// Enabled mode + LLM returns contradictions → marker is written,
/// `unimplementable_tasks` AND `unarchivable_deltas` are empty, AND
/// the executor is NOT invoked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn contradiction_preflight_findings_write_marker_and_skip_executor() {
    let submission = serde_json::json!({
        "contradictions": [
            { "requirement_a": "All secrets in env vars",
              "requirement_b": "API key in config.yaml",
              "summary": "A forbids what B requires" },
            { "requirement_a": "Cap operations at 60s",
              "requirement_b": "Run the 5-minute workflow",
              "summary": "B exceeds A's cap" }
        ]
    });
    let ctx = cc_test_ctx(Some(submission), Some("anthropic/claude-opus-4-8".into()));
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change_with_spec(
        &ws,
        "conflicting",
        "newcap",
        "## ADDED Requirements\n\n### Requirement: All secrets in env vars\nThe system SHALL store secrets only in env vars.\n\n### Requirement: API key in config.yaml\nThe API key SHALL live in config.yaml.\n",
    );

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
    let fut = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX);
    let _ =
        crate::preflight::change_contradiction::scope(Some(std::sync::Arc::new(ctx)), fut).await;
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "executor must NOT be invoked when contradictions are found"
    );

    let marker_path = ws.join("openspec/changes/conflicting/.needs-spec-revision.json");
    assert!(marker_path.exists(), "marker must be written");
    let raw = std::fs::read_to_string(&marker_path).unwrap();
    assert!(
        raw.contains("Pre-flight contradiction check found 2 issue(s)"),
        "revision_suggestion should announce 2 findings; got: {raw}"
    );
    assert!(raw.contains("Requirement A: All secrets in env vars"));
    assert!(raw.contains("Requirement B: API key in config.yaml"));
    assert!(raw.contains("A forbids what B requires"));
    assert!(raw.contains("Requirement A: Cap operations at 60s"));
    assert!(raw.contains("Requirement B: Run the 5-minute workflow"));
    assert!(raw.contains("B exceeds A's cap"));
    assert!(
        raw.contains("clear-revision"),
        "revision_suggestion should name the clear-revision verb; got: {raw}"
    );

    let parsed: crate::spec_revision::SpecNeedsRevisionMarker = serde_json::from_str(&raw).unwrap();
    assert!(
        parsed.unimplementable_tasks.is_empty(),
        "unimplementable_tasks must be empty (semantic-not-mechanical case)"
    );
    assert!(
        parsed.unarchivable_deltas.is_empty(),
        "unarchivable_deltas must be empty (semantic-not-mechanical case)"
    );
    assert!(
        !parsed.revision_suggestion.is_empty(),
        "revision_suggestion must carry the narrative"
    );
}
