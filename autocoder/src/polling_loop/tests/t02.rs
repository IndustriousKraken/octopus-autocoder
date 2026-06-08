use super::*;

#[test]
fn opportunistic_upstream_fetch_corrects_drifted_url() {
    let dir = tempfile::TempDir::new().unwrap();
    let bare = dir.path().join("bare.git");
    init_bare(&bare);
    let upstream_a = dir.path().join("upstream-a.git");
    init_bare(&upstream_a);
    let upstream_b = dir.path().join("upstream-b.git");
    init_bare(&upstream_b);
    let workspace = dir.path().join("workspace");
    init_clone(&bare, &workspace);
    // Pre-seed an `upstream` remote pointing at A.
    let st = std::process::Command::new("git")
        .args(["remote", "add", "upstream"])
        .arg(upstream_a.to_string_lossy().as_ref())
        .current_dir(&workspace)
        .status()
        .unwrap();
    assert!(st.success());
    // Configure upstream B in the repo.
    let mut repo = fixture_repo(&workspace);
    repo.upstream = Some(crate::config::UpstreamConfig {
        remote: "upstream".to_string(),
        branch: "main".to_string(),
        url: upstream_b.to_string_lossy().to_string(),
    });
    opportunistic_upstream_fetch(&workspace, &repo);
    let url = remote_url(&workspace, "upstream").unwrap();
    assert_eq!(url, upstream_b.to_string_lossy().to_string());
}

#[test]
fn opportunistic_upstream_fetch_failure_does_not_propagate() {
    // Point upstream.url at a path that isn't a git repo — fetch
    // will fail, function must log a WARN and return cleanly.
    let dir = tempfile::TempDir::new().unwrap();
    let bare = dir.path().join("bare.git");
    init_bare(&bare);
    let workspace = dir.path().join("workspace");
    init_clone(&bare, &workspace);
    let mut repo = fixture_repo(&workspace);
    repo.upstream = Some(crate::config::UpstreamConfig {
        remote: "upstream".to_string(),
        branch: "main".to_string(),
        url: "/dev/null/definitely-not-a-repo".to_string(),
    });
    // Should not panic AND should return normally.
    opportunistic_upstream_fetch(&workspace, &repo);
}

/// 13.3.2 / executor baseline: when the executor returns `Failed`,
/// autocoder unlocks the change AND does NOT archive it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_change_unlocks_and_does_not_archive() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "feature-a", "fixture reason");

    let executor = AlwaysFailingExecutor;
    let _ = run_one_pass_no_push(&ws, &executor).await; // Failed is a normal outcome

    // The change is still in the active queue (not archived).
    let pending = queue::list_pending(&paths, &ws).unwrap();
    assert_eq!(pending, vec!["feature-a".to_string()]);
    // No archive directory was created for it.
    let archive_root = ws.join("openspec/changes/archive");
    if archive_root.exists() {
        for entry in std::fs::read_dir(&archive_root).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            assert!(
                !name.contains("feature-a"),
                "Failed change must not be archived; found {name}"
            );
        }
    }
    // No `.in-progress` lock left behind.
    let lock = ws.join("openspec/changes/feature-a/.in-progress");
    assert!(!lock.exists(), "lock file should be cleared after Failed");
}

/// 13.4.1 / git-workflow-manager baseline: at start of each pass, the
/// agent branch is recreated to match the post-pull HEAD of the base
/// branch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn branch_init_resets_agent_to_base() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, _paths) = crate::testing::test_daemon_paths();
    // Empty queue is fine — we only care about the branch init step.

    let executor = CompletingExecutorNoDiff;
    run_one_pass_no_push(&ws, &executor)
        .await
        .expect("pass succeeds");

    // After init, agent-q must point at the same SHA as main.
    let main_sha = crate::git::rev_parse(&ws, "main").unwrap();
    let agent_sha = crate::git::rev_parse(&ws, "agent-q").unwrap();
    assert_eq!(
        main_sha, agent_sha,
        "agent-q must equal main after branch init step"
    );
}

/// 13.4.3 / git-workflow-manager baseline: commit subject is
/// `<change>: <first non-empty line of ## Why>`, truncated to 72 chars.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commit_subject_matches_spec_format() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, _paths) = crate::testing::test_daemon_paths();
    add_committed_change(
        &ws,
        "add-greetings",
        "Make the project greet users on startup",
    );

    let executor = CompletingExecutorWithDiff {
        artifact_name: "GREETINGS".into(),
        artifact_text: "hello world".into(),
    };
    run_one_pass_no_push(&ws, &executor)
        .await
        .expect("pass succeeds");

    // Inspect HEAD on agent-q. autocoder left us on agent-q after
    // recreate_branch + commit; verify subject directly.
    let out = std::process::Command::new("git")
        .args(["log", "-1", "--pretty=%s", "agent-q"])
        .current_dir(&ws)
        .output()
        .unwrap();
    assert!(out.status.success(), "git log failed");
    let subject = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert_eq!(
        subject, "add-greetings: Make the project greet users on startup",
        "subject should be `<change>: <first ## Why line>`"
    );
    assert!(
        subject.chars().count() <= 72,
        "subject should be ≤72 chars, got {} chars: {subject:?}",
        subject.chars().count()
    );
}

/// git-workflow-manager / orchestrator-cli: an executor that returns
/// `Completed` without modifying the workspace is treated as Failed.
/// The change is NOT archived, no commit is made, and the change is
/// unlocked so the next polling pass retries it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_with_empty_workspace_is_failed() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "no-op-change", "intentionally a no-op");

    let pre_main = crate::git::rev_parse(&ws, "main").unwrap();

    let executor = CompletingExecutorNoDiff;
    run_one_pass_no_push(&ws, &executor)
        .await
        .expect("pass succeeds");

    // Change is NOT archived: active directory must still exist and
    // the archive directory must NOT contain it.
    assert!(
        ws.join("openspec/changes/no-op-change").exists(),
        "no-op change must remain in active changes for retry"
    );
    let archive_root = ws.join("openspec/changes/archive");
    if archive_root.exists() {
        for entry in std::fs::read_dir(&archive_root).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            assert!(
                !name.ends_with("-no-op-change"),
                "no-op Completed must not produce an archive entry, found {name}"
            );
        }
    }

    // Lock removed → change is back in pending for the next pass.
    assert!(
        !ws.join("openspec/changes/no-op-change/.in-progress")
            .exists(),
        ".in-progress lock must be cleared so the change retries"
    );
    assert_eq!(
        queue::list_pending(&paths, &ws).unwrap(),
        vec!["no-op-change".to_string()],
        "change must be back in pending after a no-op Completed"
    );

    // No commit was made: agent-q must still equal main's pre-pass SHA.
    let agent_sha = crate::git::rev_parse(&ws, "agent-q").unwrap();
    assert_eq!(
        agent_sha, pre_main,
        "no-diff Completed must not create a commit"
    );
}

/// 13.4.2 / git-workflow-manager baseline: when `git pull --ff-only`
/// fails (base branch has diverged from origin), the iteration aborts
/// and the agent branch is NOT created or modified.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pull_conflict_aborts_iteration_without_touching_agent_branch() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, _paths) = crate::testing::test_daemon_paths();

    // Reach into the remote (the fixture's `remote/` sibling) to advance
    // origin/main with a commit our local doesn't have.
    let remote = ws.parent().unwrap().join("remote");
    std::fs::write(remote.join("REMOTE_ONLY.md"), "remote-side\n").unwrap();
    let st = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(&remote)
        .status()
        .unwrap();
    assert!(st.success());
    let st = std::process::Command::new("git")
        .args(["commit", "-q", "-m", "remote-side commit"])
        .current_dir(&remote)
        .status()
        .unwrap();
    assert!(st.success());

    // Now create a divergent local commit on main so pull --ff-only fails
    // (our local main is not an ancestor of origin/main and vice versa).
    std::fs::write(ws.join("LOCAL_ONLY.md"), "local-side\n").unwrap();
    let st = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(&ws)
        .status()
        .unwrap();
    assert!(st.success());
    let st = std::process::Command::new("git")
        .args(["commit", "-q", "-m", "local-side commit"])
        .current_dir(&ws)
        .status()
        .unwrap();
    assert!(st.success());

    // Sanity: agent-q must not exist yet.
    assert!(
        crate::git::rev_parse(&ws, "agent-q").is_err(),
        "agent-q must not exist before the pass"
    );

    let executor = CompletingExecutorNoDiff;
    let result = run_one_pass_no_push(&ws, &executor).await;
    assert!(result.is_err(), "pass must error when pull --ff-only fails");
    let msg = format!("{:#}", result.unwrap_err());
    assert!(
        msg.contains("git pull --ff-only failed") || msg.contains("non-fast-forward"),
        "error must surface the git failure verbatim, got: {msg}"
    );

    // Agent branch must remain absent after the aborted iteration.
    assert!(
        crate::git::rev_parse(&ws, "agent-q").is_err(),
        "agent-q must not be created when the iteration aborts at pull"
    );
}

/// 5.2: AskUser on a pending change → posts to Slack, writes
/// `.question.json`, unlocks the change, change is excluded from
/// pending and shows up in `list_waiting`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn askuser_on_pending_escalates_to_chatops() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "ambig-change", "ambiguous fixture");

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let _post = server
        .mock("POST", "/chat.postMessage")
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1234567890.123456"}"#)
        .create_async()
        .await;

    let executor = AskThenComplete { ws: ws.clone() };
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
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::collections::HashSet::new(),
    )
    .await
    .expect("pass succeeds");
    // No commits this pass — the change is now waiting.
    assert!(processed.is_empty(), "no commits on a pure-AskUser pass");

    // `.question.json` was written; change is gone from pending,
    // present in waiting; no `.in-progress` lingers.
    let q_path = ws.join("openspec/changes/ambig-change/.question.json");
    assert!(q_path.is_file(), ".question.json must be written");
    assert!(
        !ws.join("openspec/changes/ambig-change/.in-progress")
            .exists()
    );
    assert_eq!(
        queue::list_pending(&paths, &ws).unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(
        queue::list_waiting(&ws).unwrap(),
        vec!["ambig-change".to_string()]
    );

    // Persisted payload carries thread_ts and the executor's resume
    // handle.
    let q = chatops::read_question_file(&ws, "ambig-change").unwrap();
    assert_eq!(q.thread_ts, "1234567890.123456");
    assert_eq!(q.channel, "C_TEST");
    assert_eq!(q.resume_handle["change"], "ambig-change");
}

/// 5.1: a waiting change with a human reply gets resumed; on a
/// successful resume with a diff the change is archived and the pass
/// reports it as processed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waiting_change_resumes_and_archives_on_reply() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "ambig-change", "ambiguous fixture");

    // Pre-populate .question.json simulating an earlier-iteration
    // escalation.
    let q = QuestionPayload {
        thread_ts: "1234567890.123456".into(),
        channel: "C_TEST".into(),
        resume_handle: serde_json::json!({
            "change": "ambig-change",
            "workspace": ws,
        }),
        asked_at: chrono::Utc::now(),
    };
    chatops::write_question_file(&ws, "ambig-change", &q).unwrap();
    // Commit the .question.json so the workspace stays clean for the
    // pre-pass dirty check. (In production this file would persist
    // across iterations naturally; here we commit to satisfy the
    // fixture-time clean check.)
    let run_git = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .args(args)
            .current_dir(&ws)
            .status()
            .unwrap();
        assert!(st.success());
    };
    run_git(&["add", "-A"]);
    run_git(&["commit", "-q", "-m", "persist question marker"]);

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let _replies = server
        .mock(
            "GET",
            "/conversations.replies?channel=C_TEST&ts=1234567890.123456",
        )
        .with_status(200)
        .with_body(
            r#"{"ok":true,"messages":[
                {"user":"U_BOT","text":"❓ ...","ts":"1234567890.123456"},
                {"user":"U_HUMAN","text":"SAMPLE","ts":"1234567891.0"}
            ]}"#,
        )
        .create_async()
        .await;

    let executor = AskThenComplete { ws: ws.clone() };
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
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::collections::HashSet::new(),
    )
    .await
    .expect("pass succeeds");

    // Change resumed, produced a diff, was committed + archived.
    assert_eq!(processed, vec!["ambig-change".to_string()]);
    // .question.json and .answer.json both gone.
    assert!(
        !ws.join("openspec/changes/ambig-change/.question.json")
            .exists()
    );
    assert!(
        !ws.join("openspec/changes/ambig-change/.answer.json")
            .exists()
    );
    assert!(
        !queue::list_waiting(&ws)
            .unwrap()
            .contains(&"ambig-change".to_string())
    );
    // Archived under date prefix.
    let archive = ws.join("openspec/changes/archive");
    let names: Vec<String> = std::fs::read_dir(&archive)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        names.iter().any(|n| n.ends_with("-ambig-change")),
        "expected archived ambig-change in {names:?}"
    );
}
