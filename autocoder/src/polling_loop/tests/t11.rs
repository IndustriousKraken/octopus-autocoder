use super::*;

/// recover-dirty-workspace-mid-iteration: when recovery itself
/// errors (e.g. `git reset --hard` against an origin that doesn't
/// have the configured base branch), the iteration falls back to
/// the old alert-and-return-Err path. The alert is the operator's
/// signal that a manually-fixable problem is present.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dirty_workspace_recovery_failure_still_alerts() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    // Dirty state same as the success-path test.
    std::fs::create_dir_all(ws.join("openspec/changes/leftover")).unwrap();
    std::fs::write(
        ws.join("openspec/changes/leftover/proposal.md"),
        "## Why\nleftover\n",
    )
    .unwrap();

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let mock = server
        .mock("POST", "/chat.postMessage")
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .expect(1)
        .create_async()
        .await;
    let chatops_ctx = ChatOpsContext {
        chatops,
        channel: "C_TEST".to_string(),
        start_work_enabled: true,
        failure_alerts_enabled: true,
        pr_opened_enabled: true,
    };
    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    // base_branch points at a branch that does NOT exist on origin
    // → `git reset --hard origin/nonexistent-branch` errors →
    // recovery returns Err → fall back to alert path.
    let mut repo = fixture_repo(&ws);
    repo.base_branch = "nonexistent-branch".into();

    struct UnreachableExecutor;
    #[async_trait::async_trait]
    impl Executor for UnreachableExecutor {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            unreachable!()
        }
        async fn resume(&self, _h: ResumeHandle, _a: &str) -> Result<ExecutorOutcome> {
            unreachable!()
        }
    }
    let result = run_pass_through_commits(
        &paths,
        &ws,
        &repo,
        &github_cfg,
        &UnreachableExecutor,
        Some(&chatops_ctx),
        u32::MAX,
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::sync::Mutex::new(Vec::new()),
    )
    .await;
    assert!(result.is_err(), "recovery failure must surface as Err");
    let err = format!("{:#}", result.unwrap_err());
    assert!(
        err.contains("recovery failed") || err.contains("dirty"),
        "error should name the recovery failure; got: {err}"
    );
    mock.assert_async().await;
    let state = crate::alert_state::AlertState::load_or_default(&paths, &ws);
    assert!(
        state
            .alerts
            .contains_key(&crate::alert_state::AlertCategory::WorkspaceDirtyMidIteration),
        "alert state must record the WorkspaceDirtyMidIteration timestamp"
    );
}

/// classify-recovery-failure-mid-iteration: when a recovery failure
/// classifies as `Permanent` (e.g. "remains dirty after recovery"),
/// the chatops alert text carries the operator-inspection suffix.
/// The 24h throttle is unchanged; only the message body differs from
/// the legacy (no-class) form. Exercises the composition path
/// directly so the test does not depend on reproducing the rarer
/// `recheck_filtered` non-empty branch of `run_pass_through_commits`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dirty_workspace_remains_dirty_after_recovery_alerts_with_permanent_suffix() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let mock = server
        .mock("POST", "/chat.postMessage")
        .match_body(mockito::Matcher::AllOf(vec![mockito::Matcher::Regex(
            "workspace dirty mid-iteration \\(permanent; skipped until daemon restart\\) \
                 — operator inspection required\\. Latest:"
                .to_string(),
        )]))
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .expect(1)
        .create_async()
        .await;
    let chatops_ctx = ChatOpsContext {
        chatops,
        channel: "C_TEST".to_string(),
        start_work_enabled: true,
        failure_alerts_enabled: true,
        pr_opened_enabled: true,
    };
    let err = anyhow!(
        "workspace {} still dirty after recovery; refusing to proceed:\n D foo.rs",
        ws.display()
    );
    crate::alerts::handle_classified_recovery_failure(
        &paths,
        &ws,
        "git@github.com:owner/repo.git",
        Some(&chatops_ctx),
        true,
        crate::alert_state::AlertCategory::WorkspaceDirtyMidIteration,
        &err,
        crate::recovery_classification::RecoveryFailureClass::Permanent,
    )
    .await;
    mock.assert_async().await;
}

/// classify-recovery-failure-mid-iteration: a transient classification
/// (network blip, e.g. "Could not resolve host") produces an alert
/// with the `(transient; retrying)` suffix and otherwise behaves
/// identically to the pre-classification path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_init_transient_alert_carries_retrying_suffix() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let mock = server
        .mock("POST", "/chat.postMessage")
        .match_body(mockito::Matcher::Regex(
            "workspace init keeps failing \\(transient; retrying\\)\\. Latest:".to_string(),
        ))
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .expect(1)
        .create_async()
        .await;
    let chatops_ctx = ChatOpsContext {
        chatops,
        channel: "C_TEST".to_string(),
        start_work_enabled: true,
        failure_alerts_enabled: true,
        pr_opened_enabled: true,
    };
    let err = anyhow!("clone failed: fatal: Could not resolve host: github.com");
    let class = crate::recovery_classification::classify_recovery_failure(&err);
    assert_eq!(
        class,
        crate::recovery_classification::RecoveryFailureClass::Transient,
        "fixture should classify as transient"
    );
    crate::alerts::handle_classified_recovery_failure(
        &paths,
        &ws,
        "git@github.com:owner/repo.git",
        Some(&chatops_ctx),
        true,
        crate::alert_state::AlertCategory::WorkspaceInitFailure,
        &err,
        class,
    )
    .await;
    mock.assert_async().await;
}

/// recover-dirty-workspace-mid-iteration: without chatops the
/// auto-recovery still runs. Workspace dirty → recovery cleans
/// → iteration succeeds. No state file is written (no alert posted).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dirty_workspace_recovers_without_chatops() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    std::fs::create_dir_all(ws.join("openspec/changes/leftover")).unwrap();
    std::fs::write(
        ws.join("openspec/changes/leftover/proposal.md"),
        "## Why\nleftover\n",
    )
    .unwrap();

    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    struct UnreachableExecutor;
    #[async_trait::async_trait]
    impl Executor for UnreachableExecutor {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            unreachable!()
        }
        async fn resume(&self, _h: ResumeHandle, _a: &str) -> Result<ExecutorOutcome> {
            unreachable!()
        }
    }
    let result = run_pass_through_commits(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &github_cfg,
        &UnreachableExecutor,
        None, // no chatops
        u32::MAX,
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::sync::Mutex::new(Vec::new()),
    )
    .await;
    assert!(result.is_ok(), "iteration should succeed: {result:?}");
    assert!(
        !ws.join(".alert-state.json").exists(),
        "no chatops, no state file write"
    );
}

/// attempt_dirty_workspace_recovery is a thin wrapper; unit-test it
/// in isolation so a regression in the helper itself is caught
/// independently of the run_pass_through_commits integration.
#[test]
fn attempt_dirty_workspace_recovery_clears_untracked_and_tracked_modifications() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, _paths) = crate::testing::test_daemon_paths();
    // Tracked modification: rewrite README.md.
    std::fs::write(ws.join("README.md"), "modified\n").unwrap();
    // Untracked file.
    std::fs::write(ws.join("untracked.txt"), "stranger\n").unwrap();
    // Sanity: status reports both.
    let dirty = git::status_porcelain(&ws).unwrap();
    assert!(
        dirty.contains("README.md") && dirty.contains("untracked.txt"),
        "fixture must seed both kinds of dirt: {dirty}"
    );
    attempt_dirty_workspace_recovery(&ws, "main").expect("recovery should succeed");
    let after = git::status_porcelain(&ws).unwrap();
    assert!(
        after.is_empty(),
        "workspace must be clean after recovery; got: {after}"
    );
    // README.md should be restored to origin's content.
    let restored = std::fs::read_to_string(ws.join("README.md")).unwrap();
    assert_eq!(restored, "hi\n", "tracked file restored from origin");
}

/// pr-opened-chatops-notification: when `pr_opened_enabled = true`,
/// `maybe_post_pr_opened` posts exactly one message to the channel,
/// containing the repository URL, the PR URL, and the change count.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pr_opened_notification_fires_when_enabled() {
    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let mock = server
        .mock("POST", "/chat.postMessage")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::Regex("opened PR".to_string()),
            mockito::Matcher::Regex("https://github\\.com/owner/repo/pull/42".to_string()),
            mockito::Matcher::Regex("3 change".to_string()),
        ]))
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .expect(1)
        .create_async()
        .await;
    let ctx = ChatOpsContext {
        chatops,
        channel: "C_TEST".to_string(),
        start_work_enabled: true,
        failure_alerts_enabled: true,
        pr_opened_enabled: true,
    };
    let repo = RepositoryConfig { forge: None,
        url: "git@github.com:owner/repo.git".into(),
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
    maybe_post_pr_opened(
        &repo,
        Some(&ctx),
        "https://github.com/owner/repo/pull/42",
        3,
    )
    .await;
    mock.assert_async().await;
}

/// pr-opened-chatops-notification: when `pr_opened_enabled = false`,
/// no chatops post is made.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pr_opened_notification_suppressed_when_disabled() {
    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let mock = server
        .mock("POST", "/chat.postMessage")
        .expect(0)
        .create_async()
        .await;
    let ctx = ChatOpsContext {
        chatops,
        channel: "C_TEST".to_string(),
        start_work_enabled: true,
        failure_alerts_enabled: true,
        pr_opened_enabled: false,
    };
    let repo = RepositoryConfig { forge: None,
        url: "git@github.com:owner/repo.git".into(),
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
    maybe_post_pr_opened(
        &repo,
        Some(&ctx),
        "https://github.com/owner/repo/pull/42",
        1,
    )
    .await;
    mock.assert_async().await;
}

/// pr-opened-chatops-notification: when the chatops backend's post
/// returns Err, the helper does NOT panic and does NOT propagate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pr_opened_notification_failure_does_not_propagate() {
    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let _mock = server
        .mock("POST", "/chat.postMessage")
        .with_status(200)
        .with_body(r#"{"ok":false,"error":"channel_not_found"}"#)
        .create_async()
        .await;
    let ctx = ChatOpsContext {
        chatops,
        channel: "C_TEST".to_string(),
        start_work_enabled: true,
        failure_alerts_enabled: true,
        pr_opened_enabled: true,
    };
    let repo = RepositoryConfig { forge: None,
        url: "git@github.com:owner/repo.git".into(),
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
    // Should not panic; should return Ok-equivalent (it's an async fn
    // returning unit, so "doesn't panic" is the assertion).
    maybe_post_pr_opened(
        &repo,
        Some(&ctx),
        "https://github.com/owner/repo/pull/42",
        1,
    )
    .await;
}

/// re-fork-chatops-notification: a successful re-fork triggers
/// exactly one chat.postMessage whose body contains the destructive-
/// action notice and the repo URL.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refork_notification_fires_when_failure_alerts_enabled() {
    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let mock = server
        .mock("POST", "/chat.postMessage")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::Regex("re-forked".to_string()),
            mockito::Matcher::Regex("owner/repo".to_string()),
            mockito::Matcher::Regex("closed".to_string()),
        ]))
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .expect(1)
        .create_async()
        .await;
    let ctx = ChatOpsContext {
        chatops,
        channel: "C_TEST".to_string(),
        start_work_enabled: true,
        failure_alerts_enabled: true,
        pr_opened_enabled: true,
    };
    let repo = RepositoryConfig { forge: None,
        url: "git@github.com:owner/repo.git".into(),
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
    maybe_post_refork_notification(&repo, Some(&ctx)).await;
    mock.assert_async().await;
}

/// re-fork-chatops-notification: when failure alerts are disabled
/// the helper is a no-op (re-fork is a recovery event, gated by the
/// same toggle as the other operator-visible failure alerts).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refork_notification_suppressed_when_failure_alerts_disabled() {
    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let mock = server
        .mock("POST", "/chat.postMessage")
        .expect(0)
        .create_async()
        .await;
    let ctx = ChatOpsContext {
        chatops,
        channel: "C_TEST".to_string(),
        start_work_enabled: true,
        failure_alerts_enabled: false,
        pr_opened_enabled: true,
    };
    let repo = RepositoryConfig { forge: None,
        url: "git@github.com:owner/repo.git".into(),
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
    maybe_post_refork_notification(&repo, Some(&ctx)).await;
    mock.assert_async().await;
}

/// success-with-drift: report has zero failures + a PR URL → the
/// notification names the PR, the modified-file count, and the
/// archived-change count.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn end_of_rebuild_success_with_drift_posts_pr_url_message() {
    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let mock = server
        .mock("POST", "/chat.postMessage")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::Regex("PR".to_string()),
            mockito::Matcher::Regex("3 capability".to_string()),
            mockito::Matcher::Regex("5 archived change".to_string()),
        ]))
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .expect(1)
        .create_async()
        .await;
    let ctx = ChatOpsContext {
        chatops,
        channel: "C_TEST".to_string(),
        start_work_enabled: false,
        failure_alerts_enabled: false,
        pr_opened_enabled: false, // notification fires regardless
    };
    let report = crate::cli::sync_specs::RebuildReport {
        processed: 5,
        successful: 5,
        failed: 0,
        spec_files: vec![
            crate::cli::sync_specs::SpecFileOutcome {
                path: "openspec/specs/a/spec.md".into(),
                modified: true,
            },
            crate::cli::sync_specs::SpecFileOutcome {
                path: "openspec/specs/b/spec.md".into(),
                modified: true,
            },
            crate::cli::sync_specs::SpecFileOutcome {
                path: "openspec/specs/c/spec.md".into(),
                modified: true,
            },
            crate::cli::sync_specs::SpecFileOutcome {
                path: "openspec/specs/d/spec.md".into(),
                modified: false,
            },
        ],
        ..Default::default()
    };
    maybe_post_end_of_rebuild_notification(
        &fixture_repo_for_rebuild_test(),
        &report,
        Some("https://github.com/owner/repo/pull/77"),
        Some(&ctx),
    )
    .await;
    mock.assert_async().await;
}

// ---- a03: contradiction alerts are tracked revision threads (task 5.1) ----

/// A threading-capable fake backend: `post_notification_with_thread` returns a
/// canned `thread_ts` so the contradiction poster can stamp a
/// `RevisionThreadState`, AND records the `thread_body` it was handed so a test
/// can assert what the operator-facing alert renders. All other trait methods
/// are minimal/unreachable.
struct TrackingThreadBackend {
    thread_ts: Option<String>,
    bodies: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl crate::chatops::ChatOpsBackend for TrackingThreadBackend {
    fn provider_name(&self) -> &'static str {
        "tracking-fake"
    }
    fn is_experimental(&self) -> bool {
        true
    }
    async fn post_question(&self, _: &str, _: &str, _: &str) -> Result<String> {
        unreachable!()
    }
    async fn poll_thread_for_human_reply(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Option<crate::chatops::HumanReply>> {
        Ok(None)
    }
    async fn post_notification(&self, _channel: &str, text: &str) -> Result<()> {
        // Record the untracked-path body too (the manual-fix posters go through
        // `post_notification`, not the threaded path) so a test can assert on it.
        self.bodies.lock().unwrap().push(text.to_string());
        Ok(())
    }
    async fn post_notification_with_thread(
        &self,
        _channel: &str,
        _top_line: &str,
        thread_body: &str,
    ) -> Result<Option<String>> {
        self.bodies.lock().unwrap().push(thread_body.to_string());
        Ok(self.thread_ts.clone())
    }
}

fn tracking_ctx(thread_ts: Option<&str>) -> (Arc<TrackingThreadBackend>, ChatOpsContext) {
    let backend = Arc::new(TrackingThreadBackend {
        thread_ts: thread_ts.map(|s| s.to_string()),
        bodies: std::sync::Mutex::new(Vec::new()),
    });
    let ctx = ChatOpsContext {
        chatops: backend.clone(),
        channel: "C_REV".to_string(),
        start_work_enabled: true,
        failure_alerts_enabled: true,
        pr_opened_enabled: true,
    };
    (backend, ctx)
}

#[tokio::test]
async fn contradiction_alert_records_revision_thread_state() {
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let tmp_ws = tempfile::TempDir::new().unwrap();
    let repo = fixture_repo(tmp_ws.path());
    let (_backend, ctx) = tracking_ctx(Some("1748.revthread"));

    let findings = vec![crate::preflight::change_contradiction::ContradictionFinding {
        requirement_a: "A".into(),
        requirement_b: "B".into(),
        summary: "A and B cannot both hold".into(),
        suggested_fix: "MODIFY B to drop the config.yaml clause".into(),
    }];
    maybe_post_contradiction_findings_alert(
        &paths,
        Some(&ctx),
        &repo,
        "a03-demo-change",
        &findings,
        "align the change to canon",
        None,
    )
    .await;

    // A RevisionThreadState is recorded under the daemon state_dir, keyed by
    // the alert's thread_ts, carrying channel/repo/slug.
    let state = crate::revision_thread::read_state(&paths.state, "1748.revthread")
        .unwrap()
        .expect("a contradiction alert must record a RevisionThreadState");
    assert_eq!(state.channel, "C_REV");
    assert_eq!(state.repo_url, repo.url);
    assert_eq!(state.change_slug, "a03-demo-change");
    assert_eq!(
        state.status,
        crate::revision_thread::RevisionThreadStatus::Open
    );
}

#[tokio::test]
async fn degraded_contradiction_alert_records_no_revision_thread_state() {
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let tmp_ws = tempfile::TempDir::new().unwrap();
    let repo = fixture_repo(tmp_ws.path());
    // Backend returns no thread_ts (no native threading) → not reply-matchable.
    let (_backend, ctx) = tracking_ctx(None);

    let findings = vec![crate::preflight::change_contradiction::ContradictionFinding {
        requirement_a: "A".into(),
        requirement_b: "B".into(),
        summary: "x".into(),
        suggested_fix: String::new(),
    }];
    maybe_post_contradiction_findings_alert(
        &paths,
        Some(&ctx),
        &repo,
        "a03-degraded",
        &findings,
        "suggestion",
        None,
    )
    .await;

    let dir = crate::revision_thread::state_dir(&paths.state);
    let count = std::fs::read_dir(&dir)
        .map(|rd| rd.count())
        .unwrap_or(0);
    assert_eq!(count, 0, "a degraded post records no RevisionThreadState");
}

/// contradiction-gate-findings-complete-and-actionable (task 5.1 + 5.2, `[in]`
/// alert): a submission of N (>2) findings — a mix of present + absent
/// `suggested_fix` — reaches the chatops alert in full. Every finding's
/// identity is present, none dropped; each non-empty suggested fix is rendered
/// labeled distinctly from the summary; the one with no suggested fix still
/// renders its identity + summary with no error.
#[tokio::test]
async fn in_contradiction_alert_carries_every_finding_and_suggested_fix() {
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let tmp_ws = tempfile::TempDir::new().unwrap();
    let repo = fixture_repo(tmp_ws.path());
    let (backend, ctx) = tracking_ctx(Some("1748.in-complete"));

    let findings = vec![
        crate::preflight::change_contradiction::ContradictionFinding {
            requirement_a: "ReqA1".into(),
            requirement_b: "ReqB1".into(),
            summary: "WHY-ONE".into(),
            suggested_fix: "EDIT-PLAN-ONE".into(),
        },
        crate::preflight::change_contradiction::ContradictionFinding {
            requirement_a: "ReqA2".into(),
            requirement_b: "ReqB2".into(),
            summary: "WHY-TWO".into(),
            suggested_fix: "EDIT-PLAN-TWO".into(),
        },
        crate::preflight::change_contradiction::ContradictionFinding {
            requirement_a: "ReqA3".into(),
            requirement_b: "ReqB3".into(),
            summary: "WHY-THREE".into(),
            // No suggested fix — must still render identity + summary.
            suggested_fix: String::new(),
        },
    ];
    let suggestion = build_contradiction_revision_suggestion(&findings);
    maybe_post_contradiction_findings_alert(
        &paths,
        Some(&ctx),
        &repo,
        "in-complete",
        &findings,
        &suggestion,
        None,
    )
    .await;

    let bodies = backend.bodies.lock().unwrap();
    let body = bodies
        .last()
        .expect("the alert must post a thread body");
    // Every finding's identity + summary reaches the alert — none dropped.
    for f in &findings {
        assert!(body.contains(&f.requirement_a), "missing {}: {body}", f.requirement_a);
        assert!(body.contains(&f.requirement_b), "missing {}: {body}", f.requirement_b);
        assert!(body.contains(&f.summary), "missing summary {}: {body}", f.summary);
    }
    // Each non-empty suggested fix is surfaced, labeled distinctly from the summary.
    assert!(body.contains("Suggested fix: EDIT-PLAN-ONE"), "body: {body}");
    assert!(body.contains("Suggested fix: EDIT-PLAN-TWO"), "body: {body}");
    // The empty-fix finding renders no bare "Suggested fix:" label (graceful
    // degradation): every labeled fix line carries non-empty content.
    for line in body.lines().filter(|l| l.contains("Suggested fix:")) {
        let after = line.split("Suggested fix:").nth(1).unwrap_or("").trim();
        assert!(
            !after.is_empty(),
            "a Suggested fix line must carry content (no bare label): {line:?}"
        );
    }
}

/// contradiction-gate-findings-complete-and-actionable (task 5.1 + 5.2,
/// `[canon]` alert): the same completeness + suggested-fix-rendering guarantees
/// for the `[canon]` gate's alert, including a finding whose conflict is in a
/// SECOND capability.
#[tokio::test]
async fn canon_contradiction_alert_carries_every_finding_and_suggested_fix() {
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let tmp_ws = tempfile::TempDir::new().unwrap();
    let repo = fixture_repo(tmp_ws.path());
    let (backend, ctx) = tracking_ctx(Some("1748.canon-complete"));

    let findings = vec![
        crate::preflight::canon_contradiction::CanonContradictionFinding {
            change_requirement: "ChangeReq1".into(),
            canonical_capability: "executor".into(),
            canonical_requirement: "CanonReq1".into(),
            summary: "WHY-ONE".into(),
            suggested_fix: "EDIT-PLAN-ONE".into(),
        },
        // Same change requirement conflicting with a canonical requirement in a
        // SECOND, different capability — must be reported as its own finding.
        crate::preflight::canon_contradiction::CanonContradictionFinding {
            change_requirement: "ChangeReq1".into(),
            canonical_capability: "sandbox".into(),
            canonical_requirement: "CanonReq2".into(),
            summary: "WHY-TWO".into(),
            suggested_fix: "EDIT-PLAN-TWO".into(),
        },
        crate::preflight::canon_contradiction::CanonContradictionFinding {
            change_requirement: "ChangeReq3".into(),
            canonical_capability: "security".into(),
            canonical_requirement: "CanonReq3".into(),
            summary: "WHY-THREE".into(),
            // No suggested fix — must still render identity + summary.
            suggested_fix: String::new(),
        },
    ];
    let suggestion = build_canon_contradiction_revision_suggestion(&findings);
    maybe_post_canon_contradiction_findings_alert(
        &paths,
        Some(&ctx),
        &repo,
        "canon-complete",
        &findings,
        &suggestion,
        None,
    )
    .await;

    let bodies = backend.bodies.lock().unwrap();
    let body = bodies
        .last()
        .expect("the alert must post a thread body");
    // Every finding's identity + summary reaches the alert — none dropped,
    // including the second-capability conflict.
    for f in &findings {
        assert!(body.contains(&f.canonical_requirement), "missing {}: {body}", f.canonical_requirement);
        assert!(body.contains(&f.canonical_capability), "missing {}: {body}", f.canonical_capability);
        assert!(body.contains(&f.summary), "missing summary {}: {body}", f.summary);
    }
    // The two-capability conflict on the same change requirement yields two
    // distinct findings, each naming its canonical requirement.
    assert!(body.contains("CanonReq1"), "body: {body}");
    assert!(body.contains("CanonReq2"), "body: {body}");
    // Each non-empty suggested fix is surfaced, labeled distinctly from the summary.
    assert!(body.contains("Suggested fix: EDIT-PLAN-ONE"), "body: {body}");
    assert!(body.contains("Suggested fix: EDIT-PLAN-TWO"), "body: {body}");
    // The empty-fix finding renders no bare "Suggested fix:" label (graceful
    // degradation): every labeled fix line carries non-empty content.
    for line in body.lines().filter(|l| l.contains("Suggested fix:")) {
        let after = line.split("Suggested fix:").nth(1).unwrap_or("").trim();
        assert!(
            !after.is_empty(),
            "a Suggested fix line must carry content (no bare label): {line:?}"
        );
    }
}

#[tokio::test]
async fn unimplementable_tasks_alert_records_no_revision_thread_state() {
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let tmp_ws = tempfile::TempDir::new().unwrap();
    let repo = fixture_repo(tmp_ws.path());
    let (_backend, ctx) = tracking_ctx(Some("1748.unimpl"));

    let tasks = vec![crate::executor::UnimplementableTask {
        task_id: "6.4".into(),
        task_text: "ssh into prod".into(),
        reason: "no prod access".into(),
    }];
    // The executor's unimplementable-tasks marker keeps its operator-authored
    // flow: its alert is NOT tracked as a revision thread.
    maybe_post_spec_revision_alert(
        &paths,
        Some(&ctx),
        &repo,
        "a03-unimpl",
        &tasks,
        "drop task 6.4",
    )
    .await;

    let dir = crate::revision_thread::state_dir(&paths.state);
    let count = std::fs::read_dir(&dir)
        .map(|rd| rd.count())
        .unwrap_or(0);
    assert_eq!(
        count, 0,
        "an unimplementable-tasks alert must not record a RevisionThreadState"
    );
}

// ---- send-it-explains-manual-fix-markers: manual-fix alerts are untracked
//      AND explain the manual remediation in their body ----

/// Count `RevisionThreadState` files under the daemon state dir (0 == none).
fn revision_state_count(paths: &DaemonPaths) -> usize {
    let dir = crate::revision_thread::state_dir(&paths.state);
    std::fs::read_dir(&dir).map(|rd| rd.count()).unwrap_or(0)
}

/// Task 4.1: an unarchivable-deltas alert records NO `RevisionThreadState`, AND
/// the body names the unarchivable-deltas cause, says `@<bot> send it` cannot
/// revise it, AND points the operator at `clear-revision`.
#[tokio::test]
async fn unarchivable_deltas_alert_explains_manual_fix_and_is_untracked() {
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let tmp_ws = tempfile::TempDir::new().unwrap();
    let repo = fixture_repo(tmp_ws.path());
    // A thread_ts is supplied, but the manual-fix poster uses the untracked
    // (`post_notification`) path, so no thread is ever recorded regardless.
    let (backend, ctx) = tracking_ctx(Some("1748.unarch"));

    let violations = vec![crate::preflight::spec_archivability::UnarchivableDelta {
        capability: "code-reviewer".into(),
        kind: crate::preflight::spec_archivability::DeltaKind::Modified,
        header: "Reviewer prompt budget is operator-configurable".into(),
        reason: "header not found in canonical openspec/specs/code-reviewer/spec.md".into(),
    }];
    maybe_post_unarchivable_deltas_alert(
        &paths,
        Some(&ctx),
        &repo,
        "held-unarch",
        &violations,
        "fix the delta header to match canonical",
    )
    .await;

    assert_eq!(
        revision_state_count(&paths),
        0,
        "a manual-fix unarchivable-deltas alert must record no RevisionThreadState"
    );
    let bodies = backend.bodies.lock().unwrap();
    let body = bodies.last().expect("the alert must post a body");
    assert!(
        body.contains("unarchivable spec deltas"),
        "body must name the unarchivable-deltas cause: {body}"
    );
    assert!(
        body.contains("send it") && body.contains("cannot revise"),
        "body must say `@<bot> send it` cannot revise it: {body}"
    );
    assert!(
        body.contains("clear-revision"),
        "body must point at the clear-revision remediation: {body}"
    );
}

/// Task 4.2: a gate-error alert records NO `RevisionThreadState`, AND the body
/// names the gate-error cause, says `@<bot> send it` cannot revise it, AND
/// points the operator at `clear-revision`.
#[tokio::test]
async fn gate_error_alert_explains_manual_fix_and_is_untracked() {
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let tmp_ws = tempfile::TempDir::new().unwrap();
    let repo = fixture_repo(tmp_ws.path());
    let (backend, ctx) = tracking_ctx(Some("1748.gateerr"));

    maybe_post_gate_error_alert(
        &paths,
        Some(&ctx),
        &repo,
        "held-gate",
        crate::verifier_gate::VerifierGate::In,
        "CLI strategy unavailable",
        None,
    )
    .await;

    assert_eq!(
        revision_state_count(&paths),
        0,
        "a gate-error alert must record no RevisionThreadState"
    );
    let bodies = backend.bodies.lock().unwrap();
    let body = bodies.last().expect("the alert must post a body");
    assert!(
        body.contains("FAILED TO RUN") && body.contains("CLI strategy unavailable"),
        "body must name the gate-error cause: {body}"
    );
    assert!(
        body.contains("send it") && body.contains("cannot revise"),
        "body must say `@<bot> send it` cannot revise it: {body}"
    );
    assert!(
        body.contains("clear-revision"),
        "body must point at the clear-revision remediation: {body}"
    );
}

/// Task 4.3 (regression): a CONTRADICTION marker still records a
/// `RevisionThreadState` AND its alert body still advertises `@<bot> send it`
/// — the manual-fix change must not have disturbed the tracked path.
#[tokio::test]
async fn contradiction_alert_still_tracks_and_advertises_send_it() {
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let tmp_ws = tempfile::TempDir::new().unwrap();
    let repo = fixture_repo(tmp_ws.path());
    let (backend, ctx) = tracking_ctx(Some("1748.contra"));

    let findings = vec![crate::preflight::change_contradiction::ContradictionFinding {
        requirement_a: "A".into(),
        requirement_b: "B".into(),
        summary: "A and B cannot both hold".into(),
        suggested_fix: String::new(),
    }];
    maybe_post_contradiction_findings_alert(
        &paths,
        Some(&ctx),
        &repo,
        "contra-demo",
        &findings,
        "align the change",
        None,
    )
    .await;

    // The tracked path still stamps a RevisionThreadState.
    crate::revision_thread::read_state(&paths.state, "1748.contra")
        .unwrap()
        .expect("a contradiction alert must still record a RevisionThreadState");
    // ...and its body still advertises `@<bot> send it` as an actionable path.
    let bodies = backend.bodies.lock().unwrap();
    let body = bodies.last().expect("the alert must post a body");
    assert!(
        body.contains("@<bot> send it"),
        "a contradiction alert must still advertise `@<bot> send it`: {body}"
    );
}
