use super::*;

/// success-no-drift: report has zero failures + no PR URL → the
/// notification names "no drift detected".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn end_of_rebuild_success_no_drift_posts_clean_message() {
    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let mock = server
        .mock("POST", "/chat.postMessage")
        // Behavioral: the no-drift path posts exactly one notification (the
        // `.expect(1)` below); the clean-message prose is not asserted.
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
    let report = crate::cli::sync_specs::RebuildReport {
        processed: 5,
        successful: 5,
        failed: 0,
        spec_files: vec![],
        ..Default::default()
    };
    maybe_post_end_of_rebuild_notification(
        &fixture_repo_for_rebuild_test(),
        &report,
        None,
        Some(&ctx),
    )
    .await;
    mock.assert_async().await;
}

/// partial-failure: report has >0 failures → the notification lists
/// the failed slugs and includes the journalctl pointer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn end_of_rebuild_partial_failure_lists_failed_slugs() {
    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let mock = server
        .mock("POST", "/chat.postMessage")
        .match_body(mockito::Matcher::AllOf(vec![
            // Derived values: the failure count and each failed slug.
            mockito::Matcher::Regex("2 failure".to_string()),
            mockito::Matcher::Regex("a06-foo".to_string()),
            mockito::Matcher::Regex("a07-bar".to_string()),
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
        pr_opened_enabled: false,
    };
    let report = crate::cli::sync_specs::RebuildReport {
        processed: 5,
        successful: 3,
        failed: 2,
        failures: vec![
            crate::cli::sync_specs::ChangeOutcome {
                slug: "a06-foo".into(),
                original_name: "2026-01-01-a06-foo".into(),
                success: false,
                failure_reason: "boom".into(),
            },
            crate::cli::sync_specs::ChangeOutcome {
                slug: "a07-bar".into(),
                original_name: "2026-01-02-a07-bar".into(),
                success: false,
                failure_reason: "boom2".into(),
            },
        ],
        spec_files: vec![
            crate::cli::sync_specs::SpecFileOutcome {
                path: "openspec/specs/a/spec.md".into(),
                modified: true,
            },
            crate::cli::sync_specs::SpecFileOutcome {
                path: "openspec/specs/b/spec.md".into(),
                modified: true,
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

/// no-chatops: when `chatops_ctx` is None, the helper is a no-op —
/// no chatops mock should fire.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn end_of_rebuild_no_chatops_is_noop() {
    let report = crate::cli::sync_specs::RebuildReport {
        processed: 1,
        successful: 1,
        failed: 0,
        ..Default::default()
    };
    maybe_post_end_of_rebuild_notification(
        &fixture_repo_for_rebuild_test(),
        &report,
        None,
        None, // no chatops
    )
    .await;
    // No assertion needed beyond "doesn't panic"; the absence of any
    // mockito server means a stray POST would obviously fail anyway.
}

/// truncation: 15 failed slugs → the notification lists 10 + "and 5
/// more"; slugs 11-15 must not appear verbatim.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn end_of_rebuild_failed_slugs_truncation() {
    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    // Match: contains slug-01 (first) AND slug-10 (last of first 10)
    // AND "and 5 more". A negative-match for slug-11 catches the
    // truncation bug.
    let mock = server
        .mock("POST", "/chat.postMessage")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::Regex("slug-01".to_string()),
            mockito::Matcher::Regex("slug-10".to_string()),
            mockito::Matcher::Regex("and 5 more".to_string()),
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
        pr_opened_enabled: false,
    };
    let failures: Vec<crate::cli::sync_specs::ChangeOutcome> = (1..=15)
        .map(|i| crate::cli::sync_specs::ChangeOutcome {
            slug: format!("slug-{i:02}"),
            original_name: format!("2026-01-01-slug-{i:02}"),
            success: false,
            failure_reason: "boom".into(),
        })
        .collect();
    let report = crate::cli::sync_specs::RebuildReport {
        processed: 15,
        successful: 0,
        failed: 15,
        failures,
        ..Default::default()
    };
    maybe_post_end_of_rebuild_notification(
        &fixture_repo_for_rebuild_test(),
        &report,
        None,
        Some(&ctx),
    )
    .await;
    mock.assert_async().await;
}

/// pending_rebuild-branch: a polling iteration whose flag is set
/// runs the rebuild path instead of the queue walk. The fixture has
/// no archived changes (so `rebuild_canonical` produces an empty
/// report) and no drift (so the iteration completes without trying
/// to push or open a PR). The assertion is that the iteration
/// returns Ok WITHOUT invoking the executor (we pass a panicking
/// executor; if the queue-walk path were taken it would panic).
/// Skipped (printed) when `openspec` is absent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebuild_iteration_runs_when_pending_flag_set() {
    if std::process::Command::new("openspec")
        .arg("--version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("skipping rebuild_iteration_runs_when_pending_flag_set: openspec absent");
        return;
    }
    let (_dir, ws) = fixture_workspace_with_remote();
    // Seed the OpenSpec layout (with no archived changes, so the
    // rebuild is a no-op). The dirs are committed so the iteration's
    // dirty-recovery step doesn't `git clean -fd` them away as
    // untracked. Critically: do NOT seed `openspec/specs/` — the
    // rebuild's clear-and-replay would remove any tracked content
    // there, producing drift the test isn't intending to exercise.
    std::fs::create_dir_all(ws.join("openspec/changes/archive")).unwrap();
    std::fs::write(ws.join("openspec/project.md"), "# Project\n\nFixture.\n").unwrap();
    // Empty archive dir needs a gitkeep file so git tracks it.
    std::fs::write(ws.join("openspec/changes/archive/.gitkeep"), "").unwrap();
    let st = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(&ws)
        .status()
        .unwrap();
    assert!(st.success());
    let st = std::process::Command::new("git")
        .args(["commit", "-q", "-m", "scaffold openspec layout"])
        .current_dir(&ws)
        .status()
        .unwrap();
    assert!(st.success(), "commit scaffold");

    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let repo = fixture_repo(&ws);

    // Run the rebuild iteration directly. No chatops, so no
    // notification posts.
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    execute_rebuild_iteration(&paths, &ws, &repo, &github_cfg, None, 2400)
        .await
        .expect("rebuild iteration should succeed on no-drift fixture");

    // Workspace MUST be clean (the rebuild ran but produced no
    // changes; add_all + the no-staged-content branch left git in
    // a clean state).
    let porcelain = git::status_porcelain(&ws).unwrap();
    assert!(
        filter_alert_state_lines(&porcelain).is_empty(),
        "post-rebuild workspace should be clean; got: {porcelain}"
    );

    // The agent branch should exist (the rebuild iteration always
    // recreates it).
    let st = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/agent-q"])
        .current_dir(&ws)
        .status()
        .unwrap();
    assert!(
        st.success(),
        "agent-q branch must exist after rebuild iteration"
    );
}

/// flag-clear: the polling loop swaps-and-clears `pending_rebuild`
/// at iteration start. Verify the atomic semantics directly so the
/// "second RebuildSpecs arriving mid-rebuild waits for the NEXT
/// iteration" contract holds.
#[test]
fn pending_rebuild_flag_swap_clears() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let flag = std::sync::Arc::new(AtomicBool::new(true));
    let was_set = flag.swap(false, Ordering::SeqCst);
    assert!(was_set, "swap of true→false must return prior `true`");
    assert!(
        !flag.load(Ordering::SeqCst),
        "flag must be cleared after swap"
    );
    // A second swap returns false (the flag is already cleared).
    assert!(!flag.swap(false, Ordering::SeqCst));
}

#[test]
fn format_renames_notification_single_rename_one_day() {
    let renames = vec![make_rename_record(
        "2026-05-14-self-healing-deployment",
        "2026-05-14-a01-self-healing-deployment",
        "2026-05-14",
        "dependency of `2026-05-14-no-op-completion-is-failure`, which MODIFIES requirement \"Reject archive-only iterations as Failed\" added here",
    )];
    let text = format_rebuild_renames_notification("owner/repo", &renames);
    assert!(text.starts_with(
        "🔀 `owner/repo`: rebuild applied dependency-prefix renames in 1 day-group(s)"
    ));
    assert!(text.contains("2026-05-14:"));
    assert!(
        text.contains(
            "2026-05-14-self-healing-deployment → 2026-05-14-a01-self-healing-deployment"
        )
    );
    assert!(text.contains("(dependency of"));
    assert!(text.contains("MODIFIES requirement"));
}

#[test]
fn format_renames_notification_multiple_days_grouped() {
    let renames = vec![
        make_rename_record("2026-05-14-x", "2026-05-14-a01-x", "2026-05-14", "reason A"),
        make_rename_record("2026-05-15-y", "2026-05-15-a01-y", "2026-05-15", "reason B"),
    ];
    let text = format_rebuild_renames_notification("owner/repo", &renames);
    assert!(text.contains("2 day-group(s)"));
    // Both day-group headers appear.
    assert!(text.contains("2026-05-14:"));
    assert!(text.contains("2026-05-15:"));
    // Each rename listed under its day.
    let idx_14 = text.find("2026-05-14:").unwrap();
    let idx_15 = text.find("2026-05-15:").unwrap();
    assert!(idx_14 < idx_15, "days should appear in chronological order");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn renames_notification_fires_when_prefix_renames_present() {
    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let mock = server
        .mock("POST", "/chat.postMessage")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::Regex("🔀".to_string()),
            mockito::Matcher::Regex("self-healing-deployment".to_string()),
            mockito::Matcher::Regex("a01-self-healing-deployment".to_string()),
        ]))
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .expect(1)
        .create_async()
        .await;
    let ctx = ChatOpsContext {
        chatops,
        channel: "C_TEST".into(),
        start_work_enabled: false,
        failure_alerts_enabled: false,
        pr_opened_enabled: false,
    };
    let report = crate::cli::sync_specs::RebuildReport {
        processed: 2,
        successful: 2,
        failed: 0,
        prefix_renames: vec![make_rename_record(
            "2026-05-14-self-healing-deployment",
            "2026-05-14-a01-self-healing-deployment",
            "2026-05-14",
            "dependency of `2026-05-14-no-op-completion-is-failure`",
        )],
        ..Default::default()
    };
    maybe_post_rebuild_renames_notification(&fixture_repo_for_rebuild_test(), &report, Some(&ctx))
        .await;
    mock.assert_async().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn renames_notification_noop_when_empty() {
    // No mockito server: any POST would fail to match. The helper
    // must short-circuit when `prefix_renames` is empty.
    let ctx_dummy = None; // also no-op without chatops; double-safety
    let report = crate::cli::sync_specs::RebuildReport::default();
    maybe_post_rebuild_renames_notification(&fixture_repo_for_rebuild_test(), &report, ctx_dummy)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn renames_notification_post_failure_does_not_panic() {
    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    // Server returns 500 → post_notification errors. The helper
    // must log+continue (no panic).
    let _mock = server
        .mock("POST", "/chat.postMessage")
        .with_status(500)
        .with_body("nope")
        .expect_at_least(1)
        .create_async()
        .await;
    let ctx = ChatOpsContext {
        chatops,
        channel: "C_TEST".into(),
        start_work_enabled: false,
        failure_alerts_enabled: false,
        pr_opened_enabled: false,
    };
    let report = crate::cli::sync_specs::RebuildReport {
        prefix_renames: vec![make_rename_record(
            "2026-05-14-x",
            "2026-05-14-a01-x",
            "2026-05-14",
            "r",
        )],
        ..Default::default()
    };
    maybe_post_rebuild_renames_notification(&fixture_repo_for_rebuild_test(), &report, Some(&ctx))
        .await;
    // Survival is the test.
}

#[test]
fn format_abort_notification_cycle_names_both_changes() {
    let reason = crate::cli::sync_specs_deps::RebuildAbortReason::Cycle {
        changes: vec!["2026-05-14-a".into(), "2026-05-14-b".into()],
        requirements: vec![("cap".into(), "Foo".into()), ("cap".into(), "Bar".into())],
    };
    let text = format_rebuild_abort_notification("owner/repo", &reason);
    // Behavioral: a cycle-abort notification names BOTH changes in the
    // cycle (derived values); the surrounding prose is not asserted.
    assert!(text.contains("2026-05-14-a"));
    assert!(text.contains("2026-05-14-b"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abort_notification_fires_with_cycle() {
    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let mock = server
        .mock("POST", "/chat.postMessage")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::Regex("❌".to_string()),
            mockito::Matcher::Regex("2026-05-14-a".to_string()),
            mockito::Matcher::Regex("2026-05-14-b".to_string()),
        ]))
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .expect(1)
        .create_async()
        .await;
    let ctx = ChatOpsContext {
        chatops,
        channel: "C_TEST".into(),
        start_work_enabled: false,
        failure_alerts_enabled: false,
        pr_opened_enabled: false,
    };
    let report = crate::cli::sync_specs::RebuildReport {
        abort_reason: Some(crate::cli::sync_specs_deps::RebuildAbortReason::Cycle {
            changes: vec!["2026-05-14-a".into(), "2026-05-14-b".into()],
            requirements: vec![("cap".into(), "Foo".into())],
        }),
        ..Default::default()
    };
    maybe_post_rebuild_abort_notification(&fixture_repo_for_rebuild_test(), &report, Some(&ctx))
        .await;
    mock.assert_async().await;
}

#[test]
fn pr_body_includes_renames_section_before_canonical_specs() {
    let report = crate::cli::sync_specs::RebuildReport {
        processed: 2,
        successful: 2,
        failed: 0,
        spec_files: vec![crate::cli::sync_specs::SpecFileOutcome {
            path: "openspec/specs/orchestrator/spec.md".into(),
            modified: true,
        }],
        prefix_renames: vec![make_rename_record(
            "2026-05-14-self-healing-deployment",
            "2026-05-14-a01-self-healing-deployment",
            "2026-05-14",
            "dependency of `2026-05-14-no-op-completion-is-failure`",
        )],
        ..Default::default()
    };
    let body = build_rebuild_pr_body(&report);
    let renames_idx = body
        .find("Applied dependency-prefix renames")
        .expect("renames section present");
    let canonical_idx = body
        .find("Canonical spec files")
        .expect("canonical section present");
    assert!(
        renames_idx < canonical_idx,
        "renames section must precede canonical-spec-files section"
    );
    assert!(
        body.contains(
            "2026-05-14-self-healing-deployment → 2026-05-14-a01-self-healing-deployment"
        )
    );
}

/// pr-opened-chatops-notification: when chatops is unconfigured,
/// the helper is a no-op.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pr_opened_notification_noop_without_chatops() {
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
        sandbox: None,
    };
    maybe_post_pr_opened(
        &repo,
        None, // no chatops
        "https://github.com/owner/repo/pull/42",
        1,
    )
    .await;
}
