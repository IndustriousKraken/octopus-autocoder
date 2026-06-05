use super::*;

#[test]
fn truncate_reason_passthrough_when_under_or_equal_to_cap() {
    let input: String = "a".repeat(PERMA_STUCK_REASON_EXCERPT_MAX);
    let out = truncate_reason(&input);
    assert_eq!(out, input);
    assert!(!out.ends_with('…'));
}

#[test]
fn truncate_reason_truncates_and_appends_ellipsis_when_over_cap() {
    let input: String = "a".repeat(PERMA_STUCK_REASON_EXCERPT_MAX + 50);
    let out = truncate_reason(&input);
    assert_eq!(out.chars().count(), PERMA_STUCK_REASON_EXCERPT_MAX + 1);
    assert!(out.ends_with('…'), "expected trailing ellipsis: {out:?}");
}

#[test]
fn truncate_reason_respects_char_boundary_on_multibyte_input() {
    let input: String = "é".repeat(PERMA_STUCK_REASON_EXCERPT_MAX + 50);
    let out = truncate_reason(&input);
    assert_eq!(out.chars().count(), PERMA_STUCK_REASON_EXCERPT_MAX + 1);
    assert!(out.ends_with('…'));
}

/// 5.1: pending change with both `openspec/changes/foo/` AND
/// `openspec/changes/archive/<today>-foo/` present on disk is
/// excluded from the queue walk. The executor is never invoked,
/// exactly one chatops post fires under `ArchiveCollision`, the
/// iteration's processed list is empty, and the per-change failure
/// counter is NOT incremented (collision is structural, not a
/// perma-stuck-counting failure).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archive_collision_excludes_change_and_alerts() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "foo", "fixture");
    pre_create_dated_archive_entry(&ws, "foo");

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

    let (processed, _self_heal) = run_pass_through_commits(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &github_cfg,
        &UnreachableExecutorForCollision,
        Some(&chatops_ctx),
        u32::MAX,
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::collections::HashSet::new(),
    )
    .await
    .expect("iteration should complete Ok with the change excluded");

    // (a) executor never invoked — guaranteed by Unreachable*::run panic
    //     (the test would have panicked already if it had been called).
    // (c) processed list empty (no commits)
    assert!(
        processed.is_empty(),
        "no changes processed when the only pending change collides; got {processed:?}"
    );
    // (b) exactly one chatops post under ArchiveCollision
    mock.assert_async().await;
    let state = crate::alert_state::AlertState::load_or_default(&paths, &ws);
    assert!(
        state
            .alerts
            .contains_key(&crate::alert_state::AlertCategory::ArchiveCollision),
        "ArchiveCollision entry must be persisted after the alert post"
    );
    // (d) failure-state counter for `foo` is NOT incremented
    let fs = crate::failure_state::load(&paths, &ws).unwrap();
    assert!(
        fs.entries.get("foo").is_none(),
        "collision is structural, not a perma-stuck-counting failure; got: {:?}",
        fs.entries
    );
}

/// 5.2: a mixed pending set — one colliding change, one clean —
/// processes the clean one normally and excludes the colliding one
/// with a single chatops post.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archive_collision_does_not_block_other_changes() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    // `bar` sorts before `foo` and gets processed first; `foo` is
    // also added but skipped via the collision pre-flight.
    add_committed_change(&ws, "bar", "clean change");
    add_committed_change(&ws, "foo", "colliding change");
    pre_create_dated_archive_entry(&ws, "foo");

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
        start_work_enabled: false, // disable to keep mock count to 1
        failure_alerts_enabled: true,
        pr_opened_enabled: false,
    };
    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };

    /// Recording executor: succeeds on `bar`, panics on any other name.
    /// Proves the queue walk only invoked the executor for the non-
    /// colliding change.
    struct RecordingExecutor;
    #[async_trait::async_trait]
    impl Executor for RecordingExecutor {
        async fn run(&self, workspace: &Path, change: &str) -> Result<ExecutorOutcome> {
            if change != "bar" {
                panic!("executor must only be invoked for `bar`; got `{change}`");
            }
            std::fs::write(workspace.join("artifact-bar.txt"), "bar contents\n")?;
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

    let (processed, _) = run_pass_through_commits(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &github_cfg,
        &RecordingExecutor,
        Some(&chatops_ctx),
        u32::MAX,
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::collections::HashSet::new(),
    )
    .await
    .expect("iteration should succeed");

    assert_eq!(
        processed,
        vec!["bar".to_string()],
        "only the non-colliding change should be processed; got {processed:?}"
    );
    // `foo` excluded with the alert; `bar` archived → counter not touched.
    mock.assert_async().await;
    let fs = crate::failure_state::load(&paths, &ws).unwrap();
    assert!(
        fs.entries.get("foo").is_none(),
        "collided change must not move the failure counter"
    );
    assert!(
        fs.entries.get("bar").is_none(),
        "successfully-archived change must not have a failure entry"
    );
}

/// 5.5: archive-collision regression. Both paths present →
/// two consecutive iterations exclude the change every time; the
/// chatops alert fires ONCE (24h throttle catches the second
/// iteration); the executor is invoked ZERO times across both; the
/// failure-state counter stays at 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archive_collision_two_iterations_throttle_alert_and_zero_executor_invocations() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "stuck-change", "fixture");
    pre_create_dated_archive_entry(&ws, "stuck-change");

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let mock = server
        .mock("POST", "/chat.postMessage")
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .expect(1) // exactly once across BOTH iterations
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

    for _ in 0..2 {
        let (processed, _) = run_pass_through_commits(
            &paths,
            &ws,
            &fixture_repo(&ws),
            &github_cfg,
            &UnreachableExecutorForCollision,
            Some(&chatops_ctx),
            u32::MAX,
            u32::MAX,
            &crate::audits::AuditRegistry::default(),
            None,
            &std::collections::HashMap::new(),
            &std::collections::HashSet::new(),
        )
        .await
        .expect("iteration succeeds");
        assert!(processed.is_empty(), "no commits in a pure-collision pass");
    }

    mock.assert_async().await;
    let fs = crate::failure_state::load(&paths, &ws).unwrap();
    assert!(
        fs.entries.get("stuck-change").is_none(),
        "collision is not a perma-stuck-counting event across iterations"
    );
}

/// 5.3: when the per-change processing function returns Err from a
/// non-executor source (here: a fixture executor that returns
/// Completed but the post-executor `queue::archive` fails because
/// the dated archive path was pre-staged during the iteration), the
/// failure counter for that change increments by 1.
///
/// We exercise the wrapper directly via a small stub: the executor
/// creates a file BUT also pre-creates the dated archive directory
/// at runtime, so `handle_outcome`'s `queue::archive` call returns
/// Err and propagates out of `process_one_pending_change`. The Err
/// arm of `walk_queue` then calls `handle_failure_counter`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_executor_archive_failure_increments_counter() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "racy", "fixture");

    // Sanity: no failure entries yet.
    let fs0 = crate::failure_state::load(&paths, &ws).unwrap();
    assert!(fs0.entries.get("racy").is_none());

    /// Executor: writes a diff (so we get past the no-diff path)
    /// AND, during its run, pre-creates the dated archive entry so
    /// the subsequent `queue::archive` call inside `handle_outcome`
    /// fails with "archive destination already exists".
    struct ArchiveColliderExecutor;
    #[async_trait::async_trait]
    impl Executor for ArchiveColliderExecutor {
        async fn run(&self, workspace: &Path, change: &str) -> Result<ExecutorOutcome> {
            // Produce a real diff so we don't take the no-diff path.
            std::fs::write(
                workspace.join(format!("artifact-{change}.txt")),
                format!("contents for {change}\n"),
            )?;
            // Race the archive step: create the dated dir now.
            let collision = queue::archive_collision_path(workspace, change);
            std::fs::create_dir_all(&collision).unwrap();
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

    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };

    // No chatops, no preflight alert. The pre-flight check sees no
    // collision at the top of the iteration (the dated dir gets
    // created INSIDE the executor's run), so the change passes the
    // pre-flight; the post-executor archive then collides.
    let _ = run_pass_through_commits(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &github_cfg,
        &ArchiveColliderExecutor,
        None,
        u32::MAX,
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::collections::HashSet::new(),
    )
    .await;

    let fs = crate::failure_state::load(&paths, &ws).unwrap();
    let entry = fs
        .entries
        .get("racy")
        .expect("post-executor archive failure must increment the per-change counter");
    assert_eq!(
        entry.count, 1,
        "non-executor Err from process_one_pending_change must record exactly one failure"
    );
    assert!(
        entry.last_reason.contains("post-executor") || entry.last_reason.contains("already exists"),
        "reason should name the post-executor origin; got: {}",
        entry.last_reason
    );
}

/// 5.4: an iteration-level failure (dirty-workspace recovery error)
/// MUST NOT increment any per-change counter — the failure is
/// outside `walk_queue` entirely and has its own iteration-level
/// `AlertCategory::WorkspaceDirtyMidIteration`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn iteration_level_failure_does_not_increment_per_change_counter() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    // A change that COULD trigger the per-change counter if its
    // processing ever ran. Adding it lets us assert "no entry"
    // unambiguously rather than just "the file doesn't exist."
    add_committed_change(&ws, "would-be-affected", "fixture");
    // Dirty state same as dirty_workspace_recovery_failure_still_alerts:
    // an unstaged untracked dir under openspec/changes/ that the
    // pre-pass dirty check will see, with a base_branch that doesn't
    // exist on origin so recovery FAILS.
    std::fs::create_dir_all(ws.join("openspec/changes/leftover")).unwrap();
    std::fs::write(
        ws.join("openspec/changes/leftover/proposal.md"),
        "## Why\nleftover\n",
    )
    .unwrap();

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let _mock = server
        .mock("POST", "/chat.postMessage")
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
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
    let mut repo = fixture_repo(&ws);
    repo.base_branch = "nonexistent-branch".into();

    let result = run_pass_through_commits(
        &paths,
        &ws,
        &repo,
        &github_cfg,
        &UnreachableExecutorForCollision,
        Some(&chatops_ctx),
        u32::MAX,
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::collections::HashSet::new(),
    )
    .await;
    assert!(
        result.is_err(),
        "iteration must surface the recovery failure"
    );

    // The iteration-level alert fired (WorkspaceDirtyMidIteration)…
    let state = crate::alert_state::AlertState::load_or_default(&paths, &ws);
    assert!(
        state
            .alerts
            .contains_key(&crate::alert_state::AlertCategory::WorkspaceDirtyMidIteration),
        "iteration-level failure must route through WorkspaceDirtyMidIteration"
    );
    // …but no per-change counter moved.
    let fs = crate::failure_state::load(&paths, &ws).unwrap();
    assert!(
        fs.entries.is_empty(),
        "iteration-level failure must not increment any per-change counter; got: {:?}",
        fs.entries
    );
}

#[test]
fn rebuild_pr_body_all_success_omits_failures_and_parenthetical() {
    let report = crate::cli::sync_specs::RebuildReport {
        processed: 3,
        successful: 3,
        failed: 0,
        rolled_back: 0,
        spec_files: vec![crate::cli::sync_specs::SpecFileOutcome {
            path: "openspec/specs/example/spec.md".into(),
            modified: true,
        }],
        ..Default::default()
    };
    let body = build_rebuild_pr_body(&report);
    assert!(
        body.contains("Replayed 3 archived change(s) chronologically; 3 succeeded, 0 failed.\n"),
        "summary line wrong, got:\n{body}"
    );
    assert!(
        !body.contains("rolled back to archive"),
        "no rolled-back parenthetical when zero, got:\n{body}"
    );
    assert!(
        !body.contains("**Failed changes**"),
        "no failures section when zero failures, got:\n{body}"
    );
}

#[test]
fn rebuild_pr_body_partial_failure_with_rollback_includes_count_and_header() {
    let report = crate::cli::sync_specs::RebuildReport {
        processed: 5,
        successful: 3,
        failed: 2,
        rolled_back: 2,
        failures: vec![
            crate::cli::sync_specs::ChangeOutcome {
                slug: "broken-modified-ref".into(),
                original_name: "2026-05-15-broken-modified-ref".into(),
                success: false,
                failure_reason:
                    "openspec refused to apply: broken-modified-ref MODIFIED failed for header \"### Requirement: X\" - not found; full output: ..."
                        .into(),
            },
            crate::cli::sync_specs::ChangeOutcome {
                slug: "another-bad".into(),
                original_name: "2026-05-16-another-bad".into(),
                success: false,
                failure_reason: "openspec refused to apply: another-bad MODIFIED failed; full output: ...".into(),
            },
        ],
        spec_files: vec![],
        ..Default::default()
    };
    let body = build_rebuild_pr_body(&report);
    assert!(
        body.contains(
            "Replayed 5 archived change(s) chronologically; 3 succeeded, 2 failed (2 rolled back to archive).\n"
        ),
        "summary line wrong, got:\n{body}"
    );
    assert!(
        body.contains(
            "**Failed changes** (rolled back to archive — see failure reasons below for the openspec output explaining each):\n"
        ),
        "failures-section header wrong, got:\n{body}"
    );
    assert!(
        !body.contains("left at active path"),
        "stale 'left at active path' wording must be gone, got:\n{body}"
    );
    assert!(
        body.contains("- `broken-modified-ref`: openspec refused to apply:"),
        "per-change line missing the headline, got:\n{body}"
    );
}

#[test]
fn rebuild_pr_body_rollback_gap_shows_smaller_rolled_back_count() {
    // 2 failed, only 1 rolled back (the other had a rollback-of-rollback
    // collision and ended up with "rollback ALSO failed" baked into its
    // failure_reason per the atomicity contract).
    let report = crate::cli::sync_specs::RebuildReport {
        processed: 5,
        successful: 3,
        failed: 2,
        rolled_back: 1,
        failures: vec![
            crate::cli::sync_specs::ChangeOutcome {
                slug: "rolled-back-ok".into(),
                original_name: "2026-05-15-rolled-back-ok".into(),
                success: false,
                failure_reason: "openspec refused to apply: foo; full output: ...".into(),
            },
            crate::cli::sync_specs::ChangeOutcome {
                slug: "rollback-also-failed".into(),
                original_name: "2026-05-16-rollback-also-failed".into(),
                success: false,
                failure_reason:
                    "openspec refused to apply: bar; rollback ALSO failed: AlreadyExists".into(),
            },
        ],
        spec_files: vec![],
        ..Default::default()
    };
    let body = build_rebuild_pr_body(&report);
    assert!(
        body.contains(
            "Replayed 5 archived change(s) chronologically; 3 succeeded, 2 failed (1 rolled back to archive).\n"
        ),
        "summary line wrong, got:\n{body}"
    );
    let unrolled_line = body
        .lines()
        .find(|l| l.contains("rollback-also-failed"))
        .expect("entry for unrolled-back slug must appear in failures list");
    assert!(
        unrolled_line.contains("rollback ALSO failed"),
        "unrolled-back entry should still surface 'rollback ALSO failed', got: {unrolled_line}"
    );
}
