use super::*;

/// Task 5.3: mixed case — workspace with one iteration-pending
/// marker present AND the iteration produces audit-shaped commits
/// AND iteration_request-WIP-shaped commits on agent-q. The
/// suppression rule fires on ANY marker presence regardless of
/// commit-message content; no PR opens AND the agent-q commits
/// remain on disk for the next iteration to ship.
///
/// Note on fixture mechanics: the iteration's `recreate_branch`
/// step (`git checkout -B agent-q` from base) wipes any
/// pre-iteration agent-q commits, so the audit fixture below
/// creates BOTH an audit-shaped AND an iteration-WIP-shaped
/// commit during its `run()` to put the mixed-content state on
/// agent-q AFTER recreate. The suppression rule fires after the
/// audit phase, so this is the same shape the production flow
/// presents.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_only_pr_suppressed_mixed_audit_and_iteration_wip() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let ws = {
        let renamed = ws.parent().unwrap().join("workspace-a38-mixed-test");
        std::fs::rename(&ws, &renamed).unwrap();
        renamed
    };
    let basename = ws.file_name().and_then(|s| s.to_str()).unwrap().to_string();

    // Plant the iteration-pending marker (the prior iteration's
    // `IterationRequested` arm would have written it).
    crate::iteration_pending::write_marker(
        &paths,
        &basename,
        "a35-foo",
        &crate::iteration_pending::IterationPendingMarker {
            completed_tasks: vec!["1".into()],
            remaining_tasks: vec!["2".into()],
            reason: "scope-overflow".into(),
            iteration_number: 2,
        },
    )
    .unwrap();

    // Fixture audit: produces an audit-shaped commit AND an
    // extra iteration-WIP-shaped commit on agent-q so commit_count
    // > 0 AND the agent branch carries mixed content at the time
    // the suppression rule runs.
    struct MixedContentAudit {
        log: Arc<std::sync::Mutex<Vec<String>>>,
    }
    #[async_trait::async_trait]
    impl crate::audits::Audit for MixedContentAudit {
        fn audit_type(&self) -> &'static str {
            "security_bug"
        }
        fn description(&self) -> &'static str {
            "mixed-content fixture"
        }
        fn requires_head_change(&self) -> bool {
            false
        }
        fn write_policy(&self) -> crate::audits::WritePolicy {
            crate::audits::WritePolicy::OpenSpecOnly
        }
        async fn run(
            &self,
            ctx: &mut crate::audits::AuditContext<'_>,
        ) -> Result<crate::audits::AuditOutcome> {
            self.log.lock().unwrap().push("audit:security_bug".into());
            // Audit-shaped commit: new proposal directory.
            let dir = ctx.workspace.join("openspec/changes/secure-test-3");
            std::fs::create_dir_all(&dir)?;
            std::fs::write(
                dir.join("proposal.md"),
                "## Why\nfixture proposal secure-test-3\n",
            )?;
            std::fs::write(dir.join("tasks.md"), "- [ ] do thing\n")?;
            let st = std::process::Command::new("git")
                .args(["add", "-A"])
                .current_dir(ctx.workspace)
                .status()?;
            anyhow::ensure!(st.success(), "git add failed");
            let st = std::process::Command::new("git")
                .args([
                    "commit",
                    "-q",
                    "-m",
                    "audit: security_bug proposals (1 change(s))",
                ])
                .current_dir(ctx.workspace)
                .status()?;
            anyhow::ensure!(st.success(), "audit commit failed");
            // Iteration-WIP-shaped commit on top so the suppression
            // rule sees mixed commit content at gate time.
            std::fs::write(ctx.workspace.join("wip.txt"), "iteration 2 work\n")?;
            let st = std::process::Command::new("git")
                .args(["add", "-A"])
                .current_dir(ctx.workspace)
                .status()?;
            anyhow::ensure!(st.success(), "git add wip failed");
            let st = std::process::Command::new("git")
                .args([
                    "commit",
                    "-q",
                    "-m",
                    "iteration 2 of a35-foo: refactor scope-overflow",
                ])
                .current_dir(ctx.workspace)
                .status()?;
            anyhow::ensure!(st.success(), "wip commit failed");
            Ok(crate::audits::AuditOutcome::specs_written(vec![
                "secure-test-3".to_string(),
            ]))
        }
    }

    let log = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let probe = MixedContentAudit { log: log.clone() };
    let registry = crate::audits::AuditRegistry::with_audits(vec![
        Arc::new(probe) as Arc<dyn crate::audits::Audit>
    ]);
    let queued = std::sync::Mutex::new(vec![crate::polling_loop::QueuedAudit { audit_type: "security_bug".to_string(), origin: None }]);

    let _hook_guard = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    let _list_mock = server
        .mock(
            "GET",
            mockito::Matcher::Regex("^/repos/owner/fixture/pulls".to_string()),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create_async()
        .await;
    let pr_mock = server
        .mock("POST", "/repos/owner/fixture/pulls")
        .with_status(201)
        .with_body(r#"{"html_url":"x","number":1}"#)
        .expect(0)
        .create_async()
        .await;
    test_hooks::set_github_api_base(Some(server.url()));

    let github_cfg = GithubConfig {
        token_env: "X".into(),
        token: Some(crate::config::SecretSource::Inline {
            value: "inline-test-token".into(),
        }),
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };

    let executor = AlwaysFailingExecutor;
    let result = execute_one_pass(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &executor,
        &github_cfg,
        None,
        None,
        2400u64,
        u32::MAX,
        u32::MAX,
        0,
        10, // human_revise_cap: irrelevant (dispatcher disabled)
        &registry,
        None,
        &std::collections::HashMap::new(),
        &queued,
    )
    .await;
    test_hooks::set_github_api_base(None);
    result.expect("mixed-content suppressed iteration must return Ok(())");

    // PR-creation HTTP call NOT invoked (suppression rule fires
    // on ANY marker, mixed commit content doesn't change it).
    pr_mock.assert_async().await;

    // Both commits remain on local agent-q awaiting the next
    // iteration's PR after the iteration-pending change concludes.
    let local_log = std::process::Command::new("git")
        .args(["log", "agent-q", "--format=%s"])
        .current_dir(&ws)
        .output()
        .unwrap();
    let local_subjects = String::from_utf8_lossy(&local_log.stdout).to_string();
    assert!(
        local_subjects.contains("audit: security_bug proposals (1 change(s))"),
        "audit's commit must remain on LOCAL agent-q; got: {local_subjects}"
    );
    assert!(
        local_subjects.contains("iteration 2 of a35-foo: refactor scope-overflow"),
        "iteration-WIP commit must remain on LOCAL agent-q; got: {local_subjects}"
    );
}

/// build_iteration_commit_subject keeps the subject under 80 chars
/// AND uses the canonical `iteration N of <change>: <reason>` shape.
#[test]
fn build_iteration_commit_subject_truncates_long_reason() {
    let long_reason = "a".repeat(200);
    let s = build_iteration_commit_subject("a30-foo", 2, &long_reason);
    assert!(s.len() <= 80, "subject too long: {} chars: {s}", s.len());
    assert!(s.starts_with("iteration 2 of a30-foo: "), "subject: {s}");
}

#[test]
fn build_iteration_commit_subject_uses_first_line_of_reason() {
    let multi_line = "first line\nsecond line";
    let s = build_iteration_commit_subject("a30-foo", 3, multi_line);
    assert_eq!(s, "iteration 3 of a30-foo: first line");
}

/// Task 4.7: integration test. An `IterationRequested` outcome
/// dispatched through `handle_outcome` (a) commits the workspace
/// diff to the agent branch with the iteration-numbered subject,
/// (b) force-pushes the agent branch to the remote, AND (c) writes
/// `.iteration-pending.json` with the documented payload.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn iteration_requested_commits_pushes_and_writes_marker() {
    let (dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "a31-bar", "fixture reason");
    // Switch to agent-q so the iteration arm's commit + push hits the
    // expected branch. `recreate_branch` is idempotent here.
    git::recreate_branch(&ws, "agent-q").unwrap();
    // Establish the change's .in-progress lock — the arm must drop
    // it as part of its cleanup.
    queue::lock(&ws, "a31-bar").unwrap();
    // Modify a workspace file so there's a real diff to commit.
    std::fs::write(ws.join("artifact.txt"), "iteration 2 progress\n").unwrap();

    let repo = fixture_repo(&ws);
    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let outcome = Ok(ExecutorOutcome::IterationRequested {
        completed_tasks: vec!["1".into(), "2".into()],
        remaining_tasks: vec!["3".into()],
        reason: "task 3 needs a refactor I want to plan more carefully".into(),
        iteration_number: 2,
    });
    let step = handle_outcome(&paths, &ws, &repo, &github_cfg, None, "a31-bar", outcome)
        .await
        .unwrap();
    assert!(
        matches!(step, QueueStep::IterationPending),
        "expected IterationPending QueueStep; got {step:?}"
    );

    // (a) The marker was written with the documented payload.
    // It now lives under `<state>/iteration-pending/<basename>/<change>.json`
    // (state_dir, NOT the workspace), so read via DaemonPaths +
    // the workspace's basename — same resolution `handle_outcome`
    // used internally for the write.
    let test_basename = ws.file_name().and_then(|s| s.to_str()).unwrap();
    let marker = crate::iteration_pending::read_marker(&paths, test_basename, "a31-bar")
        .unwrap()
        .unwrap();
    assert_eq!(marker.iteration_number, 2);
    assert_eq!(
        marker.completed_tasks,
        vec!["1".to_string(), "2".to_string()]
    );
    assert_eq!(marker.remaining_tasks, vec!["3".to_string()]);
    assert_eq!(
        marker.reason,
        "task 3 needs a refactor I want to plan more carefully"
    );

    // (b) The agent-branch's HEAD subject is the iteration commit.
    let head_subject = std::process::Command::new("git")
        .args(["log", "-1", "--format=%s"])
        .current_dir(&ws)
        .output()
        .unwrap();
    let subject = String::from_utf8_lossy(&head_subject.stdout).to_string();
    assert!(
        subject.starts_with("iteration 2 of a31-bar:"),
        "agent-branch HEAD subject must be the iteration commit; got: {subject}"
    );

    // (c) The remote's agent-q ref also has the new commit (the
    // arm force-pushed). Look up the remote agent-q's log subjects.
    let remote = dir.path().join("remote");
    let remote_log = std::process::Command::new("git")
        .args(["log", "agent-q", "--format=%s"])
        .current_dir(&remote)
        .output()
        .unwrap();
    let remote_subjects = String::from_utf8_lossy(&remote_log.stdout).to_string();
    assert!(
        remote_log.status.success(),
        "agent-q must exist on the remote after force-push: {remote_subjects}"
    );
    assert!(
        remote_subjects.contains("iteration 2 of a31-bar:"),
        "remote agent-q must contain the iteration commit; got: {remote_subjects}"
    );

    // (d) The .in-progress lock was dropped.
    assert!(
        !ws.join("openspec/changes/a31-bar/.in-progress").exists(),
        ".in-progress must be dropped by the IterationRequested arm"
    );

    // (e) No PR-related routine was called — verify by absence of
    // any HTTP mock setup. This test does NOT set up mockito for
    // GitHub; if the arm tried to open a PR it would fail with a
    // connection error AND fall over before this assertion.
}

/// Task 6.5: Completed deletes `.iteration-pending.json`. Self-heal
/// AND main Completed paths both archive (which itself moves the
/// directory); the explicit deletion happens BEFORE the archive
/// rename, so the archived directory does not carry the marker.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_arm_deletes_iteration_pending_marker() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "a31-bar", "fixture reason");
    // Establish a stale marker (prior iteration's IterationRequested).
    crate::iteration_pending::write_marker(
        &paths,
        ws.file_name().and_then(|s| s.to_str()).unwrap(),
        "a31-bar",
        &crate::iteration_pending::IterationPendingMarker {
            completed_tasks: vec!["1".into(), "2".into()],
            remaining_tasks: vec!["3".into()],
            reason: "prior reason".into(),
            iteration_number: 2,
        },
    )
    .unwrap();
    queue::lock(&ws, "a31-bar").unwrap();
    // Make a real diff so the Completed arm reaches its archive +
    // commit branch.
    std::fs::write(ws.join("artifact.txt"), "final work\n").unwrap();

    let repo = fixture_repo(&ws);
    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let outcome = Ok(ExecutorOutcome::Completed { final_answer: None });
    let step = handle_outcome(&paths, &ws, &repo, &github_cfg, None, "a31-bar", outcome)
        .await
        .unwrap();
    assert!(
        matches!(step, QueueStep::Archived),
        "expected Archived; got {step:?}"
    );
    // Marker was deleted BEFORE the archive rename, so the
    // archived directory should NOT carry it either.
    assert!(
        !crate::iteration_pending::marker_exists(
            &paths,
            ws.file_name().and_then(|s| s.to_str()).unwrap(),
            "a31-bar",
        ),
        "iteration-pending marker must be removed on Completed"
    );
    // (sanity) the active dir is gone (it was archived).
    assert!(
        !ws.join("openspec/changes/a31-bar").exists(),
        "active change dir must have been archived"
    );
}

/// Task 6.5: SpecNeedsRevision deletes `.iteration-pending.json`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spec_needs_revision_arm_deletes_iteration_pending_marker() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "a31-bar", "fixture reason");
    crate::iteration_pending::write_marker(
        &paths,
        ws.file_name().and_then(|s| s.to_str()).unwrap(),
        "a31-bar",
        &crate::iteration_pending::IterationPendingMarker {
            completed_tasks: vec!["1".into()],
            remaining_tasks: vec!["2".into()],
            reason: "prior".into(),
            iteration_number: 2,
        },
    )
    .unwrap();
    queue::lock(&ws, "a31-bar").unwrap();

    let repo = fixture_repo(&ws);
    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let outcome = Ok(ExecutorOutcome::SpecNeedsRevision {
        unimplementable_tasks: vec![crate::executor::UnimplementableTask {
            task_id: "6.4".into(),
            task_text: "manual".into(),
            reason: "sandbox".into(),
        }],
        revision_suggestion: "do a thing".into(),
    });
    let step = handle_outcome(&paths, &ws, &repo, &github_cfg, None, "a31-bar", outcome)
        .await
        .unwrap();
    assert!(
        matches!(step, QueueStep::SpecRevisionMarked),
        "expected SpecRevisionMarked; got {step:?}"
    );
    assert!(
        !crate::iteration_pending::marker_exists(
            &paths,
            ws.file_name().and_then(|s| s.to_str()).unwrap(),
            "a31-bar",
        ),
        "iteration-pending marker must be removed on SpecNeedsRevision"
    );
}

/// Task 6.5: Failed arm leaves `.iteration-pending.json` untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_arm_preserves_iteration_pending_marker() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "a31-bar", "fixture reason");
    let marker = crate::iteration_pending::IterationPendingMarker {
        completed_tasks: vec!["1".into()],
        remaining_tasks: vec!["2".into()],
        reason: "prior".into(),
        iteration_number: 2,
    };
    crate::iteration_pending::write_marker(
        &paths,
        ws.file_name().and_then(|s| s.to_str()).unwrap(),
        "a31-bar",
        &marker,
    )
    .unwrap();
    queue::lock(&ws, "a31-bar").unwrap();

    let repo = fixture_repo(&ws);
    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let outcome = Ok(ExecutorOutcome::Failed {
        reason: "timeout".into(),
    });
    let step = handle_outcome(&paths, &ws, &repo, &github_cfg, None, "a31-bar", outcome)
        .await
        .unwrap();
    assert!(
        matches!(step, QueueStep::Failed { .. }),
        "expected Failed; got {step:?}"
    );
    let still = crate::iteration_pending::read_marker(
        &paths,
        ws.file_name().and_then(|s| s.to_str()).unwrap(),
        "a31-bar",
    )
    .unwrap()
    .unwrap();
    assert_eq!(still, marker, "Failed must NOT touch the marker");
}

/// Task 6.5: AskUser arm leaves `.iteration-pending.json` untouched.
/// AskUser without chatops_ctx returns `AskUserExitEarly` AND does
/// NOT touch the marker (the agent's question may resolve into a
/// continuation; the iteration context stays available).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ask_user_arm_preserves_iteration_pending_marker() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "a31-bar", "fixture reason");
    let marker = crate::iteration_pending::IterationPendingMarker {
        completed_tasks: vec!["1".into()],
        remaining_tasks: vec!["2".into()],
        reason: "prior".into(),
        iteration_number: 2,
    };
    crate::iteration_pending::write_marker(
        &paths,
        ws.file_name().and_then(|s| s.to_str()).unwrap(),
        "a31-bar",
        &marker,
    )
    .unwrap();
    queue::lock(&ws, "a31-bar").unwrap();

    let repo = fixture_repo(&ws);
    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let outcome = Ok(ExecutorOutcome::AskUser {
        question: "what next?".into(),
        resume_handle: crate::executor::ResumeHandle(serde_json::json!({})),
    });
    let step = handle_outcome(&paths, &ws, &repo, &github_cfg, None, "a31-bar", outcome)
        .await
        .unwrap();
    assert!(
        matches!(step, QueueStep::AskUserExitEarly),
        "expected AskUserExitEarly (no chatops_ctx); got {step:?}"
    );
    let still = crate::iteration_pending::read_marker(
        &paths,
        ws.file_name().and_then(|s| s.to_str()).unwrap(),
        "a31-bar",
    )
    .unwrap()
    .unwrap();
    assert_eq!(still, marker, "AskUser must NOT touch the marker");
}
