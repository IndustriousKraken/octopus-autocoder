use super::*;

/// Self-heal precondition unmet: `openspec validate --strict` errors
/// because the spec is missing a Scenario block. Same fall-through to
/// Failed; no archive, no commit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn self_heal_falls_through_when_openspec_validate_fails() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    // tasks all done, but spec lacks Scenario → openspec validate fails.
    add_committed_self_heal_change(&ws, "invalid-spec", true, false);

    let pre_main = crate::git::rev_parse(&ws, "main").unwrap();

    let executor = CompletingExecutorNoDiff;
    let repo = fixture_repo(&ws);
    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let (processed, includes_self_heal) = run_pass_through_commits(
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
        &std::collections::HashSet::new(),
    )
    .await
    .expect("pass returns Failed via fall-through, not Err");
    assert!(processed.is_empty());
    assert!(!includes_self_heal);

    // Change must remain in pending and unarchived.
    assert!(ws.join("openspec/changes/invalid-spec").exists());
    let archive_root = ws.join("openspec/changes/archive");
    if archive_root.exists() {
        for entry in std::fs::read_dir(&archive_root).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            assert!(!name.ends_with("-invalid-spec"));
        }
    }
    let agent_sha = crate::git::rev_parse(&ws, "agent-q").unwrap();
    assert_eq!(agent_sha, pre_main);
}

/// A pass with normally-implemented changes only (no self-heal) must
/// NOT include the self-heal disclaimer paragraph in the PR body.
#[test]
fn self_heal_paragraph_omitted_when_no_self_heals_in_pass() {
    let tmp = tempfile::TempDir::new().unwrap();
    let processed = vec!["regular-change".to_string()];
    let body = build_pr_body(tmp.path(), &processed, false);
    assert!(
        !body.contains(
            "This PR archives one or more changes whose implementation was already present"
        ),
        "disclaimer paragraph must not appear when includes_self_heal=false; got: {body}"
    );
    assert!(
        body.contains("- regular-change"),
        "normal change listing must remain"
    );
}

/// max-changes-per-pr-limit: with 5 pending changes and a cap of 3, a
/// single pass commits exactly 3 archives and leaves the remaining 2
/// in the pending queue for the next iteration.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn walk_queue_stops_at_max_changes() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    for n in 1..=5 {
        add_committed_change(&ws, &format!("ch{n:02}"), &format!("fixture {n}"));
    }

    let executor = PerChangeArtifactExecutor;
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
        &executor,
        None,
        u32::MAX,
        3, // cap,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::collections::HashSet::new(),
    )
    .await
    .expect("pass succeeds");

    assert_eq!(processed.len(), 3, "exactly 3 changes commit in one pass");
    assert_eq!(
        processed,
        vec!["ch01".to_string(), "ch02".to_string(), "ch03".to_string()],
        "first three by queue order are processed"
    );
    // The remaining two are still pending.
    let still_pending = queue::list_pending(&paths, &ws).unwrap();
    assert_eq!(
        still_pending,
        vec!["ch04".to_string(), "ch05".to_string()],
        "the last two remain in the queue for the next iteration"
    );
}

/// max-changes-per-pr-limit: a cap of 1 ships exactly one archive per
/// pass; the rest of the queue waits for subsequent iterations.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn walk_queue_cap_of_1_ships_one_per_pass() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    for n in 1..=3 {
        add_committed_change(&ws, &format!("ch{n:02}"), &format!("fixture {n}"));
    }
    let executor = PerChangeArtifactExecutor;
    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
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
        &github_cfg,
        &executor,
        None,
        u32::MAX,
        1, // cap of 1,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::collections::HashSet::new(),
    )
    .await
    .expect("pass succeeds");
    assert_eq!(processed, vec!["ch01".to_string()], "cap=1 → one archive");
    let still_pending = queue::list_pending(&paths, &ws).unwrap();
    assert_eq!(
        still_pending,
        vec!["ch02".to_string(), "ch03".to_string()],
        "remaining changes wait for the next iteration"
    );
}

/// halt-queue-walk-on-non-archive: a `Failed` outcome halts the walk
/// regardless of cap. Changes later in the queue may depend on the
/// failed one and SHALL NOT be attempted until the next iteration.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn walk_queue_halts_on_failed_change() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    // ch01 succeeds, ch02 fails, ch03 and ch04 would succeed but the
    // walk must halt at the failure.
    add_committed_change(&ws, "ch01", "succeeds first");
    add_committed_change(&ws, "ch02-fails", "fails second");
    add_committed_change(&ws, "ch03", "should not be attempted");
    add_committed_change(&ws, "ch04", "should not be attempted");

    struct ArchiveThenFailThenWouldArchive;
    #[async_trait::async_trait]
    impl Executor for ArchiveThenFailThenWouldArchive {
        async fn run(&self, workspace: &Path, change: &str) -> Result<ExecutorOutcome> {
            if change == "ch02-fails" {
                return Ok(ExecutorOutcome::Failed {
                    reason: "fixture failure".into(),
                });
            }
            std::fs::write(
                workspace.join(format!("artifact-{change}.txt")),
                format!("contents for {change}\n"),
            )?;
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

    let executor = ArchiveThenFailThenWouldArchive;
    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
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
        &github_cfg,
        &executor,
        None,
        u32::MAX,
        10, // cap intentionally generous; halt must come from the failure,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::collections::HashSet::new(),
    )
    .await
    .expect("pass succeeds");
    assert_eq!(
        processed,
        vec!["ch01".to_string()],
        "only ch01 archived; ch02-fails halts the walk before ch03/ch04"
    );
    // ch02-fails still pending (failed once, retries next iteration).
    // ch03 and ch04 still pending (walker never reached them).
    let still_pending = queue::list_pending(&paths, &ws).unwrap();
    assert!(
        still_pending.contains(&"ch02-fails".to_string()),
        "failed change still pending for retry: {still_pending:?}"
    );
    assert!(
        still_pending.contains(&"ch03".to_string()),
        "untouched ch03 still pending: {still_pending:?}"
    );
    assert!(
        still_pending.contains(&"ch04".to_string()),
        "untouched ch04 still pending: {still_pending:?}"
    );
    // ch03 must not have a failure-state entry — it was never attempted.
    let state = failure_state::load(&paths, &ws).unwrap();
    assert!(
        !state.entries.contains_key("ch03"),
        "ch03 must not have a failure-state entry — walker never reached it; got: {:?}",
        state.entries
    );
}

/// halt-queue-walk-on-non-archive: an `Escalated` outcome (AskUser
/// posted to chatops) halts the walk regardless of cap. Later
/// pending changes wait for the next iteration.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn walk_queue_halts_on_escalated_change() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "ch01", "succeeds first");
    add_committed_change(&ws, "ch02-asks", "asks a question");
    add_committed_change(&ws, "ch03", "should not be attempted");

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let _post = server
        .mock("POST", "/chat.postMessage")
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"2222222222.111111"}"#)
        .create_async()
        .await;

    struct ArchiveThenAskThenWouldArchive {
        ws: std::path::PathBuf,
    }
    #[async_trait::async_trait]
    impl Executor for ArchiveThenAskThenWouldArchive {
        async fn run(&self, workspace: &Path, change: &str) -> Result<ExecutorOutcome> {
            if change == "ch02-asks" {
                return Ok(ExecutorOutcome::AskUser {
                    question: "Halt me?".to_string(),
                    resume_handle: ResumeHandle(
                        serde_json::json!({"change": change, "workspace": self.ws}),
                    ),
                });
            }
            std::fs::write(
                workspace.join(format!("artifact-{change}.txt")),
                format!("contents for {change}\n"),
            )?;
            Ok(ExecutorOutcome::Completed { final_answer: None })
        }
        async fn resume(&self, _h: ResumeHandle, _a: &str) -> Result<ExecutorOutcome> {
            unreachable!("resume not exercised in this test")
        }
    }

    let executor = ArchiveThenAskThenWouldArchive { ws: ws.clone() };
    let chatops_ctx = ChatOpsContext {
        chatops: chatops.clone(),
        channel: "C_TEST".to_string(),
        start_work_enabled: true,
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
    let (processed, _) = run_pass_through_commits(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &test_github,
        &executor,
        Some(&chatops_ctx),
        u32::MAX,
        10, // cap intentionally generous; halt must come from escalation,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::collections::HashSet::new(),
    )
    .await
    .expect("pass succeeds");
    assert_eq!(
        processed,
        vec!["ch01".to_string()],
        "only ch01 archived; ch02-asks halts the walk before ch03"
    );
    // ch02-asks is now waiting (has .question.json).
    assert!(
        ws.join("openspec/changes/ch02-asks/.question.json")
            .is_file(),
        "ch02-asks must have .question.json after escalation"
    );
    // ch03 is still pending — walker never reached it.
    let still_pending = queue::list_pending(&paths, &ws).unwrap();
    assert!(
        still_pending.contains(&"ch03".to_string()),
        "untouched ch03 still pending: {still_pending:?}"
    );
}

/// commit-trailing-archive: after a single-change archive pass, the
/// working tree MUST be clean. The original bug left the archive
/// rename uncommitted, causing the next iteration's dirty check to
/// trip.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archived_change_leaves_clean_working_tree() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "only-change", "fixture for trailing-archive");
    let executor = PerChangeArtifactExecutor;
    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
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
        &github_cfg,
        &executor,
        None,
        u32::MAX,
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::collections::HashSet::new(),
    )
    .await
    .expect("pass succeeds");
    assert_eq!(processed, vec!["only-change".to_string()]);
    let porcelain = crate::git::status_porcelain(&ws).unwrap();
    assert!(
        porcelain.trim().is_empty(),
        "working tree must be clean after archive; got:\n{porcelain}"
    );
}

/// commit-trailing-archive: HEAD's commit MUST contain both the
/// executor's implementation file AND the archive rename of
/// proposal.md / tasks.md.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commit_contains_both_impl_and_archive_rename() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "feature-x", "trailing archive test");
    let executor = PerChangeArtifactExecutor;
    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
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
        &github_cfg,
        &executor,
        None,
        u32::MAX,
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::collections::HashSet::new(),
    )
    .await
    .expect("pass succeeds");

    // diff-tree --name-status HEAD^..HEAD shows the files changed in
    // the new commit. Use `-M` to detect renames so the archive move
    // shows as a single `R` entry rather than D+A.
    let out = std::process::Command::new("git")
        .args([
            "diff-tree",
            "--no-commit-id",
            "--name-status",
            "-r",
            "-M",
            "HEAD^..HEAD",
        ])
        .current_dir(&ws)
        .output()
        .unwrap();
    assert!(out.status.success(), "diff-tree failed");
    let body = String::from_utf8_lossy(&out.stdout).to_string();

    // Implementation artifact must appear.
    assert!(
        body.contains("artifact-feature-x.txt"),
        "commit missing executor artifact; diff-tree output:\n{body}"
    );
    // Archive move must appear (either as a rename or as D+A pair).
    let has_rename = body.lines().any(|l| {
        l.starts_with("R")
            && l.contains("openspec/changes/feature-x/proposal.md")
            && l.contains("openspec/changes/archive/")
    });
    let has_delete_and_add = body
        .lines()
        .any(|l| l.starts_with("D\topenspec/changes/feature-x/"))
        && body
            .lines()
            .any(|l| l.starts_with("A\topenspec/changes/archive/") && l.contains("-feature-x/"));
    assert!(
        has_rename || has_delete_and_add,
        "commit must contain archive rename of openspec/changes/feature-x/; diff-tree output:\n{body}"
    );
}

/// recover-dirty-workspace-mid-iteration: a workspace dirty at
/// `run_pass_through_commits` time triggers auto-recovery
/// (`git reset --hard origin/<base> + git clean -fd`). When recovery
/// cleans the dirt, the iteration proceeds normally AND no chatops
/// alert fires (the operator does not need to be notified about a
/// self-healed condition).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dirty_workspace_recovers_and_iteration_proceeds() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    // Seed a dirty state: untracked file under openspec/.
    // `git clean -fd` (the recovery step) will remove this.
    std::fs::create_dir_all(ws.join("openspec/changes/leftover")).unwrap();
    std::fs::write(
        ws.join("openspec/changes/leftover/proposal.md"),
        "## Why\nleftover\n",
    )
    .unwrap();

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    // No alert should fire — recovery handles the dirt silently.
    let mock = server
        .mock("POST", "/chat.postMessage")
        .expect(0)
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
    struct UnreachableExecutor;
    #[async_trait::async_trait]
    impl Executor for UnreachableExecutor {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            // After `git clean -fd` the leftover dir is gone, so the
            // queue walk has nothing to do and the executor is never
            // invoked. If this panics, the test reveals a regression.
            unreachable!("post-recovery queue must be empty; executor should not be invoked")
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
        result.is_ok(),
        "iteration should succeed after recovery; got: {result:?}"
    );
    // The dirty untracked dir must be gone.
    assert!(
        !ws.join("openspec/changes/leftover").exists(),
        "git clean -fd should have removed the untracked dir"
    );
    // No state file was written because no alert fired.
    assert!(
        !ws.join(".alert-state.json").exists(),
        "no alert, no state file write"
    );
    mock.assert_async().await;
}
