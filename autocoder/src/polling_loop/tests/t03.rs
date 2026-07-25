use super::*;

/// orchestrator-cli: when a resume returns `Completed` but the
/// executor did not modify the workspace, the change is NOT archived.
/// The question/answer files are cleared so the change leaves
/// "waiting" state, but it must come back as pending for the next
/// pass to retry rather than being silently completed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_with_empty_workspace_is_failed() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "ambig-change", "ambiguous fixture");

    // Pre-populate .question.json as if escalated in a prior iteration.
    let q = QuestionPayload {
        thread_ts: "2222222222.222222".into(),
        channel: "C_TEST".into(),
        resume_handle: serde_json::json!({"change": "ambig-change"}),
        asked_at: chrono::Utc::now(),
    };
    chatops::write_question_file(&ws, "ambig-change", &q).unwrap();
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

    let pre_main = crate::git::rev_parse(&ws, "main").unwrap();

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let _replies = server
        .mock(
            "GET",
            "/conversations.replies?channel=C_TEST&ts=2222222222.222222",
        )
        .with_status(200)
        .with_body(
            r#"{"ok":true,"messages":[
                {"user":"U_BOT","text":"❓ ...","ts":"2222222222.222222"},
                {"user":"U_HUMAN","text":"some reply","ts":"2222222223.0"}
            ]}"#,
        )
        .create_async()
        .await;

    // Executor whose resume returns Completed without touching the
    // workspace, then refuses to do work if `run()` is later called
    // (which the same pass will do, since the no-diff resume puts
    // the change back into pending state — that retry is production-
    // correct, we just don't want it to mask what the resume path
    // did in this test).
    struct ResumeReturnsCompletedNoDiff;
    #[async_trait::async_trait]
    impl Executor for ResumeReturnsCompletedNoDiff {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            Ok(ExecutorOutcome::Failed {
                reason: "retry after no-diff resume; not implementing in this fixture".into(),
            })
        }
        async fn resume(&self, _h: ResumeHandle, _a: &str) -> Result<ExecutorOutcome> {
            Ok(ExecutorOutcome::Completed { final_answer: None })
        }
    }
    let executor = ResumeReturnsCompletedNoDiff;
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
    let (processed, _, _) = run_pass_through_commits(
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
    .await
    .expect("pass succeeds");

    // No commits this pass — the resume produced no diff.
    assert!(
        processed.is_empty(),
        "no-diff resume must not be reported as committed"
    );

    // Change is NOT archived: active dir still present, archive
    // (if it exists) does not contain it.
    assert!(
        ws.join("openspec/changes/ambig-change").exists(),
        "change must remain in active changes after no-diff resume"
    );
    let archive = ws.join("openspec/changes/archive");
    if archive.exists() {
        for entry in std::fs::read_dir(&archive).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            assert!(
                !name.ends_with("-ambig-change"),
                "no-diff resume must not produce an archive entry, found {name}"
            );
        }
    }

    // Question + answer files cleared; change is back in pending,
    // not waiting.
    assert!(
        !ws.join("openspec/changes/ambig-change/.question.json")
            .exists(),
        ".question.json must be deleted after resume"
    );
    assert!(
        !ws.join("openspec/changes/ambig-change/.answer.json")
            .exists(),
        ".answer.json must be deleted after resume"
    );
    assert!(
        !queue::list_waiting(&ws)
            .unwrap()
            .contains(&"ambig-change".to_string()),
        "change must leave waiting state after resume"
    );
    assert!(
        queue::list_pending(&paths, &ws)
            .unwrap()
            .contains(&"ambig-change".to_string()),
        "change must return to pending for retry"
    );

    // No commit was made on agent-q (it should equal main's pre-pass
    // SHA after branch init).
    let agent_sha = crate::git::rev_parse(&ws, "agent-q").unwrap();
    assert_eq!(
        agent_sha, pre_main,
        "no-diff resume must not create a commit"
    );
}

/// 5.1a: same-repo block. If after the waiting-processing step the
/// waiting set is STILL non-empty, the pending pass MUST NOT run for
/// this iteration.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_repo_block_skips_pending_when_still_waiting() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "still-waiting", "stuck on a question");
    add_committed_change(&ws, "would-be-pending", "should not be touched");

    // .question.json on `still-waiting`.
    let q = QuestionPayload {
        thread_ts: "1111.1111".into(),
        channel: "C_TEST".into(),
        resume_handle: serde_json::json!({}),
        asked_at: chrono::Utc::now(),
    };
    chatops::write_question_file(&ws, "still-waiting", &q).unwrap();
    let run_git = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .args(args)
            .current_dir(&ws)
            .status()
            .unwrap();
        assert!(st.success());
    };
    run_git(&["add", "-A"]);
    run_git(&["commit", "-q", "-m", "persist question"]);

    // Slack returns no human reply yet → change stays waiting.
    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let _ = server
        .mock("GET", "/conversations.replies?channel=C_TEST&ts=1111.1111")
        .with_status(200)
        .with_body(
            r#"{"ok":true,"messages":[
                {"user":"U_BOT","text":"❓ ...","ts":"1111.1111"}
            ]}"#,
        )
        .create_async()
        .await;

    // An executor that would PANIC if invoked — it must NOT be called
    // for `would-be-pending` since the same-repo block applies.
    struct MustNotRunExecutor;
    #[async_trait::async_trait]
    impl Executor for MustNotRunExecutor {
        async fn run(&self, _w: &Path, change: &str) -> Result<ExecutorOutcome> {
            panic!("executor must not run on pending `{change}` while another change is waiting");
        }
        async fn resume(&self, _h: ResumeHandle, _a: &str) -> Result<ExecutorOutcome> {
            unreachable!()
        }
    }

    let executor = MustNotRunExecutor;
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
    let (processed, _, _) = run_pass_through_commits(
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
    .await
    .expect("pass succeeds without running pending");
    assert!(processed.is_empty(), "no work this iteration");
    // Still waiting.
    assert_eq!(
        queue::list_waiting(&ws).unwrap(),
        vec!["still-waiting".to_string()]
    );
}

/// Verifies the orchestrator-cli "Queue resumes after waiting set
/// empties" scenario: when the human reply arrives AND the resume
/// completes, the same iteration proceeds to process pending changes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_resumes_after_waiting_set_empties() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "was-waiting", "fixture for waiting");
    add_committed_change(&ws, "fresh-pending", "fresh fixture");

    // Pre-populate .question.json for `was-waiting`.
    let q = QuestionPayload {
        thread_ts: "9999.9999".into(),
        channel: "C_TEST".into(),
        resume_handle: serde_json::json!({
            "change": "was-waiting",
            "workspace": ws,
        }),
        asked_at: chrono::Utc::now(),
    };
    chatops::write_question_file(&ws, "was-waiting", &q).unwrap();
    let run_git = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .args(args)
            .current_dir(&ws)
            .status()
            .unwrap();
        assert!(st.success());
    };
    run_git(&["add", "-A"]);
    run_git(&["commit", "-q", "-m", "persist marker"]);

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    // Reply arrives.
    let _ = server
        .mock("GET", "/conversations.replies?channel=C_TEST&ts=9999.9999")
        .with_status(200)
        .with_body(
            r#"{"ok":true,"messages":[
                {"user":"U_BOT","text":"❓ ...","ts":"9999.9999"},
                {"user":"U_HUMAN","text":"go ahead","ts":"9999.0001"}
            ]}"#,
        )
        .create_async()
        .await;

    // Executor: resumes was-waiting (produces a file), runs fresh-pending
    // (produces a different file). Both Completed-with-diff.
    let ws_for_exec = ws.clone();
    struct ResumeAndRunBoth {
        ws: std::path::PathBuf,
        invocations: std::sync::Mutex<Vec<String>>,
    }
    #[async_trait::async_trait]
    impl Executor for ResumeAndRunBoth {
        async fn run(&self, _w: &Path, change: &str) -> Result<ExecutorOutcome> {
            self.invocations
                .lock()
                .unwrap()
                .push(format!("run:{change}"));
            std::fs::write(self.ws.join(format!("RUN_{change}.txt")), "from run")?;
            Ok(ExecutorOutcome::Completed { final_answer: None })
        }
        async fn resume(&self, _h: ResumeHandle, _a: &str) -> Result<ExecutorOutcome> {
            self.invocations.lock().unwrap().push("resume".to_string());
            std::fs::write(self.ws.join("RESUMED.txt"), "from resume")?;
            Ok(ExecutorOutcome::Completed { final_answer: None })
        }
    }
    let executor = ResumeAndRunBoth {
        ws: ws_for_exec,
        invocations: std::sync::Mutex::new(Vec::new()),
    };

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
    let (processed, _, _) = run_pass_through_commits(
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
    .await
    .expect("pass succeeds");

    // Both changes processed in this single iteration: the resumed one
    // AND the fresh pending one. Both archived.
    assert_eq!(
        processed
            .iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>(),
        ["was-waiting", "fresh-pending"]
            .iter()
            .map(|s| s.to_string())
            .collect::<std::collections::HashSet<_>>(),
        "both changes must process in the same iteration once waiting empties"
    );
    // Resume was called BEFORE the fresh-pending run (waiting-first
    // iteration order).
    let inv = executor.invocations.lock().unwrap().clone();
    let resume_idx = inv.iter().position(|s| s == "resume").unwrap();
    let pending_idx = inv.iter().position(|s| s == "run:fresh-pending").unwrap();
    assert!(
        resume_idx < pending_idx,
        "resume must run BEFORE pending: invocations={inv:?}"
    );
}

/// max-changes-per-pr-limit: a resumed waiting change that archives
/// counts toward the per-iteration cap. With one waiting + two pending
/// and `max_changes_per_pr = 2`, the pass ships exactly two commits
/// (the resumed archive + the first pending archive); the second
/// pending change is deferred to the next iteration.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_one_pass_resumed_change_counts_toward_cap() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "was-waiting", "fixture for waiting");
    add_committed_change(&ws, "pending-one", "first fresh pending");
    add_committed_change(&ws, "pending-two", "second fresh pending");

    // Pre-populate .question.json for `was-waiting` so the resume path
    // engages.
    let q = QuestionPayload {
        thread_ts: "7777.7777".into(),
        channel: "C_TEST".into(),
        resume_handle: serde_json::json!({
            "change": "was-waiting",
            "workspace": ws,
        }),
        asked_at: chrono::Utc::now(),
    };
    chatops::write_question_file(&ws, "was-waiting", &q).unwrap();
    let run_git = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .args(args)
            .current_dir(&ws)
            .status()
            .unwrap();
        assert!(st.success());
    };
    run_git(&["add", "-A"]);
    run_git(&["commit", "-q", "-m", "persist marker"]);

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    // Human reply arrives so the resume engages.
    let _ = server
        .mock("GET", "/conversations.replies?channel=C_TEST&ts=7777.7777")
        .with_status(200)
        .with_body(
            r#"{"ok":true,"messages":[
                {"user":"U_BOT","text":"❓ ...","ts":"7777.7777"},
                {"user":"U_HUMAN","text":"go ahead","ts":"7777.0001"}
            ]}"#,
        )
        .create_async()
        .await;

    // Executor: resume writes a file for the waiting change; run
    // writes a per-change file for fresh pending changes. Both
    // Completed-with-diff.
    let ws_for_exec = ws.clone();
    struct ResumeAndRunPerChange {
        ws: std::path::PathBuf,
        invocations: std::sync::Mutex<Vec<String>>,
    }
    #[async_trait::async_trait]
    impl Executor for ResumeAndRunPerChange {
        async fn run(&self, _w: &Path, change: &str) -> Result<ExecutorOutcome> {
            self.invocations
                .lock()
                .unwrap()
                .push(format!("run:{change}"));
            std::fs::write(
                self.ws.join(format!("RUN_{change}.txt")),
                format!("artifact for {change}"),
            )?;
            Ok(ExecutorOutcome::Completed { final_answer: None })
        }
        async fn resume(&self, _h: ResumeHandle, _a: &str) -> Result<ExecutorOutcome> {
            self.invocations.lock().unwrap().push("resume".to_string());
            std::fs::write(self.ws.join("RESUMED.txt"), "from resume")?;
            Ok(ExecutorOutcome::Completed { final_answer: None })
        }
    }
    let executor = ResumeAndRunPerChange {
        ws: ws_for_exec,
        invocations: std::sync::Mutex::new(Vec::new()),
    };

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
    let (processed, _, _) = run_pass_through_commits(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &test_github,
        &executor,
        Some(&chatops_ctx),
        u32::MAX,
        2, // cap of 2 across resume + pending,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::sync::Mutex::new(Vec::new()),
    )
    .await
    .expect("pass succeeds");

    assert_eq!(
        processed.len(),
        2,
        "cap of 2 must ship exactly 2 commits: resumed + one pending"
    );
    assert_eq!(
        processed[0], "was-waiting",
        "resumed change processed first"
    );
    assert_eq!(
        processed[1], "pending-one",
        "first pending change processed next"
    );

    let inv = executor.invocations.lock().unwrap().clone();
    assert!(
        !inv.contains(&"run:pending-two".to_string()),
        "second pending must NOT have run (cap stopped the walk); invocations={inv:?}"
    );

    // The undelivered pending change is still in the queue for the
    // next iteration.
    let still_pending = queue::list_pending(&paths, &ws).unwrap();
    assert!(
        still_pending.contains(&"pending-two".to_string()),
        "deferred change still pending: {still_pending:?}"
    );
}

/// archive-rides-completion-commit (task 2.1): a waiting change that
/// resumes to Completed-with-diff records exactly ONE new commit whose
/// tree carries the dated archive entry and NOT the change's active
/// directory, and the working tree is clean afterwards. This locks in the
/// archive-before-commit ordering on the resume path (matching the pending
/// path), so no committed tree on the agent branch ever shows the
/// completed change still active under `openspec/changes/`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_completion_commit_captures_archive_move() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "resume-target", "resume then archive");

    // Pre-populate .question.json so the change is enumerated as waiting.
    let q = QuestionPayload {
        thread_ts: "5555.5555".into(),
        channel: "C_TEST".into(),
        resume_handle: serde_json::json!({"change": "resume-target", "workspace": ws}),
        asked_at: chrono::Utc::now(),
    };
    chatops::write_question_file(&ws, "resume-target", &q).unwrap();
    let git_ok = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .args(args)
            .current_dir(&ws)
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?} failed");
    };
    git_ok(&["add", "-A"]);
    git_ok(&["commit", "-q", "-m", "persist marker"]);

    let pre_main = crate::git::rev_parse(&ws, "main").unwrap();

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let _ = server
        .mock("GET", "/conversations.replies?channel=C_TEST&ts=5555.5555")
        .with_status(200)
        .with_body(
            r#"{"ok":true,"messages":[
                {"user":"U_BOT","text":"❓ ...","ts":"5555.5555"},
                {"user":"U_HUMAN","text":"go ahead","ts":"5555.0001"}
            ]}"#,
        )
        .create_async()
        .await;

    // Resume writes a real artifact (so the tree carries executor output),
    // then returns Completed-with-diff.
    struct ResumeWritesArtifact {
        ws: std::path::PathBuf,
    }
    #[async_trait::async_trait]
    impl Executor for ResumeWritesArtifact {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            unreachable!("run must not be called; the change resumes")
        }
        async fn resume(&self, _h: ResumeHandle, _a: &str) -> Result<ExecutorOutcome> {
            std::fs::write(self.ws.join("RESUMED_ARTIFACT.txt"), "resume output")?;
            Ok(ExecutorOutcome::Completed { final_answer: None })
        }
    }
    let executor = ResumeWritesArtifact { ws: ws.clone() };
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
    let (processed, _, _) = run_pass_through_commits(
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
    .await
    .expect("pass succeeds");

    assert_eq!(
        processed,
        vec!["resume-target".to_string()],
        "the resumed change must archive"
    );

    let git_out = |args: &[&str]| -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&ws)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    };

    // Exactly one new commit on agent-q relative to main's pre-pass tip.
    let count: usize = git_out(&["rev-list", "--count", "main..agent-q"])
        .trim()
        .parse()
        .unwrap();
    assert_eq!(count, 1, "resume completion must record exactly one commit");
    // (belt-and-suspenders: the pass did not touch main)
    assert_eq!(crate::git::rev_parse(&ws, "main").unwrap(), pre_main);

    // The single commit's tree carries the dated archive entry...
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let tree = git_out(&["ls-tree", "-r", "--name-only", "agent-q"]);
    let archived_prefix = format!("openspec/changes/archive/{today}-resume-target/");
    assert!(
        tree.lines().any(|p| p.starts_with(&archived_prefix)),
        "committed tree must contain the dated archive entry; tree=\n{tree}"
    );
    // ...and does NOT carry the active change directory.
    assert!(
        !tree
            .lines()
            .any(|p| p.starts_with("openspec/changes/resume-target/")),
        "committed tree must NOT contain the active change dir; tree=\n{tree}"
    );

    // The working tree is clean afterwards — no dangling archive rename.
    let porcelain = git_out(&["status", "--porcelain"]);
    assert!(
        porcelain.trim().is_empty(),
        "working tree must be clean after resume completion; got:\n{porcelain}"
    );
}

/// archive-rides-completion-commit (task 2.2): when the archive step fails
/// on the resume completion path, NO completion commit is recorded and the
/// change stays at its active path. The failure short-circuits (via `?`)
/// before any staging or commit — matching the pending path's semantics.
///
/// The failure is driven by a real `openspec archive` abort: the change's
/// spec delta MODIFIES a requirement that has no canonical spec, which
/// openspec refuses to apply (`Aborted. No files were changed.`), and
/// `queue::archive_at`'s post-condition helper surfaces as an `Err`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_archive_failure_records_no_commit() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();

    // A waiting change whose MODIFIED delta targets a nonexistent
    // canonical spec — `openspec archive` aborts on it.
    let dir = ws.join("openspec/changes/bad-archive");
    std::fs::create_dir_all(dir.join("specs/somecap")).unwrap();
    std::fs::write(
        dir.join("proposal.md"),
        "## Why\nresume archive-failure fixture, padded so it clears the length warning cleanly.\n",
    )
    .unwrap();
    std::fs::write(dir.join("tasks.md"), "- [x] 1.1 done\n").unwrap();
    std::fs::write(
        dir.join("specs/somecap/spec.md"),
        "## MODIFIED Requirements\n\n### Requirement: Nonexistent thing\nThe system SHALL do a thing never canonically specified.\n\n#### Scenario: whatever\n- **WHEN** x\n- **THEN** y\n",
    )
    .unwrap();

    let q = QuestionPayload {
        thread_ts: "4444.4444".into(),
        channel: "C_TEST".into(),
        resume_handle: serde_json::json!({"change": "bad-archive", "workspace": ws}),
        asked_at: chrono::Utc::now(),
    };
    chatops::write_question_file(&ws, "bad-archive", &q).unwrap();
    let git_ok = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .args(args)
            .current_dir(&ws)
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?} failed");
    };
    git_ok(&["add", "-A"]);
    git_ok(&["commit", "-q", "-m", "persist bad-archive"]);

    let pre_main = crate::git::rev_parse(&ws, "main").unwrap();

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    let _ = server
        .mock("GET", "/conversations.replies?channel=C_TEST&ts=4444.4444")
        .with_status(200)
        .with_body(
            r#"{"ok":true,"messages":[
                {"user":"U_BOT","text":"❓ ...","ts":"4444.4444"},
                {"user":"U_HUMAN","text":"go ahead","ts":"4444.0001"}
            ]}"#,
        )
        .create_async()
        .await;

    // Resume writes an artifact (so the completion branch — not the
    // no-diff branch — runs), then returns Completed. The archive step
    // then aborts. `run` returns Failed for the post-resume pending
    // re-pickup so that phase makes no commit either.
    struct ResumeThenBadArchive {
        ws: std::path::PathBuf,
    }
    #[async_trait::async_trait]
    impl Executor for ResumeThenBadArchive {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            Ok(ExecutorOutcome::Failed {
                reason: "not implementing after a failed-archive resume".into(),
            })
        }
        async fn resume(&self, _h: ResumeHandle, _a: &str) -> Result<ExecutorOutcome> {
            std::fs::write(self.ws.join("RESUMED_ARTIFACT.txt"), "resume output")?;
            Ok(ExecutorOutcome::Completed { final_answer: None })
        }
    }
    let executor = ResumeThenBadArchive { ws: ws.clone() };
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
    let (processed, _, _) = run_pass_through_commits(
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
    .await
    .expect("pass succeeds");

    assert!(
        processed.is_empty(),
        "a failed-archive resume must archive nothing; got {processed:?}"
    );

    // No completion commit: agent-q sits at main's pre-pass tip.
    let git_out = |args: &[&str]| -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&ws)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    let count: usize = git_out(&["rev-list", "--count", "main..agent-q"])
        .trim()
        .parse()
        .unwrap();
    assert_eq!(count, 0, "a failed archive must record no completion commit");
    assert_eq!(crate::git::rev_parse(&ws, "agent-q").unwrap(), pre_main);

    // The change stays at its active path (archive did not move it) and no
    // dated archive entry was produced.
    assert!(
        ws.join("openspec/changes/bad-archive").is_dir(),
        "the change must stay at its active path after a failed archive"
    );
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    assert!(
        !ws.join(format!("openspec/changes/archive/{today}-bad-archive"))
            .exists(),
        "a failed archive must not produce a dated archive entry"
    );
}
