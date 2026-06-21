//! `octopus-md-agent-guide`: pass-level provisioning behavior.
//!
//! The unit-level idempotency / staleness / non-clobber tests live in
//! `crate::octopus_guide`'s own module (they drive `provision_on_agent_branch`
//! directly). These tests assert the INTEGRATION through the real pass path:
//! the guide commit is formed on the recreated agent branch when the feature
//! is scoped ON, and the committed files ride the existing `pr_open` seam
//! honoring `auto_submit_pr` with no guide-specific wiring.

use super::*;

/// Helper: run a pass through commit formation with the guide gate scoped to
/// `guide_enabled`, on a fixture workspace whose `origin` is a real bare-ish
/// remote (so base sync's `git pull --ff-only origin main` succeeds).
async fn run_pass_with_guide(
    ws: &Path,
    executor: &dyn Executor,
    guide_enabled: bool,
) -> Vec<String> {
    let (_td, paths) = crate::testing::test_daemon_paths();
    let repo = fixture_repo(ws);
    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let registry = crate::audits::AuditRegistry::default();
    let settings = std::collections::HashMap::new();
    let queued = std::sync::Mutex::new(Vec::new());
    let fut = run_pass_through_commits(
        &paths,
        ws,
        &repo,
        &github_cfg,
        executor,
        None,
        u32::MAX,
        u32::MAX,
        &registry,
        None,
        &settings,
        &queued,
    );
    let (processed, _, _) = crate::octopus_guide::scope(guide_enabled, fut)
        .await
        .expect("pass through commits succeeds");
    processed
}

fn tracked(ws: &Path, path: &str) -> bool {
    let out = std::process::Command::new("git")
        .args(["ls-files", path])
        .current_dir(ws)
        .output()
        .unwrap();
    !String::from_utf8_lossy(&out.stdout).trim().is_empty()
}

/// 6.1: guide missing + enabled → the pass forms a commit on the agent branch
/// carrying BOTH OCTOPUS.md and the managed AGENTS.md reference, staged for the
/// push + PR path. The files are NOT registered in `.git/info/exclude`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guide_enabled_pass_commits_octopus_and_agents_on_agent_branch() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let pre_main = crate::git::rev_parse(&ws, "main").unwrap();

    // Empty queue: the only commit the pass can produce is the guide commit.
    let executor = CompletingExecutorNoDiff;
    let processed = run_pass_with_guide(&ws, &executor, true).await;
    assert!(processed.is_empty(), "no change work: {processed:?}");

    // The agent branch advanced past base by exactly the guide commit.
    let agent_sha = crate::git::rev_parse(&ws, "agent-q").unwrap();
    assert_ne!(agent_sha, pre_main, "guide commit must advance agent branch");
    assert_eq!(
        crate::git::rev_list_count(&ws, "main..agent-q").unwrap(),
        1,
        "exactly one guide commit on the agent branch"
    );

    // Both files exist, are tracked (committed), and carry the canonical bytes.
    assert_eq!(
        std::fs::read_to_string(ws.join("OCTOPUS.md")).unwrap(),
        crate::octopus_guide::OCTOPUS_MD
    );
    assert!(tracked(&ws, "OCTOPUS.md"), "OCTOPUS.md must be committed");
    assert!(tracked(&ws, "AGENTS.md"), "AGENTS.md must be committed");
    let agents = std::fs::read_to_string(ws.join("AGENTS.md")).unwrap();
    assert!(agents.contains(crate::octopus_guide::AGENTS_REGION_START));
    assert!(agents.contains("OCTOPUS.md"));

    // The two files MUST NOT be excluded — they must be committable.
    let excludes =
        std::fs::read_to_string(ws.join(".git/info/exclude")).unwrap_or_default();
    assert!(
        !excludes.contains("OCTOPUS.md") && !excludes.contains("AGENTS.md"),
        "guide files must not be in .git/info/exclude: {excludes}"
    );
}

/// 6.2: disabled → the pass writes nothing and forms no commit; the agent
/// branch tree is unchanged by the feature.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guide_disabled_pass_writes_nothing_and_makes_no_commit() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let pre_main = crate::git::rev_parse(&ws, "main").unwrap();

    let executor = CompletingExecutorNoDiff;
    let processed = run_pass_with_guide(&ws, &executor, false).await;
    assert!(processed.is_empty());

    assert!(!ws.join("OCTOPUS.md").exists(), "OCTOPUS.md must not be written");
    assert!(!ws.join("AGENTS.md").exists(), "AGENTS.md must not be written");
    let agent_sha = crate::git::rev_parse(&ws, "agent-q").unwrap();
    assert_eq!(
        agent_sha, pre_main,
        "disabled feature must not advance the agent branch"
    );
}

/// 6.3: already-current → the pass forms no additional guide commit (no churn).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guide_already_current_pass_makes_no_commit() {
    let (_dir, ws) = fixture_workspace_with_remote();

    // First pass provisions the guide.
    let executor = CompletingExecutorNoDiff;
    run_pass_with_guide(&ws, &executor, true).await;
    let after_first = crate::git::rev_parse(&ws, "agent-q").unwrap();
    assert_eq!(crate::git::rev_list_count(&ws, "main..agent-q").unwrap(), 1);

    // Second pass: base sync recreates agent-q from main (which still lacks the
    // guide, since nothing merged), so the guide is re-provisioned — but as a
    // single deterministic commit, never a churned duplicate. Assert the agent
    // branch carries exactly one guide commit, not two.
    run_pass_with_guide(&ws, &executor, true).await;
    assert_eq!(
        crate::git::rev_list_count(&ws, "main..agent-q").unwrap(),
        1,
        "re-provisioning must remain a single deterministic guide commit"
    );

    // When the guide IS present on the base-synced tree (simulate a merged
    // guide by committing it to main directly), a pass produces NO commit.
    let run = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .args(args)
            .current_dir(&ws)
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?} failed");
    };
    run(&["checkout", "-q", "main"]);
    std::fs::write(ws.join("OCTOPUS.md"), crate::octopus_guide::OCTOPUS_MD).unwrap();
    std::fs::write(
        ws.join("AGENTS.md"),
        crate::octopus_guide::compose_agents_md(None),
    )
    .unwrap();
    run(&["add", "-A"]);
    run(&["commit", "-q", "-m", "merge guide into base"]);
    let _ = after_first; // base now carries the guide.

    let processed = run_pass_with_guide(&ws, &executor, true).await;
    assert!(processed.is_empty());
    assert_eq!(
        crate::git::rev_list_count(&ws, "main..agent-q").unwrap(),
        0,
        "already-current base → no guide commit, no churn"
    );
}

/// 6.6: with the guide files committed on the agent branch, the existing
/// `open_pull_request` seam honors `auto_submit_pr` with no guide-specific
/// wiring. `false` → BranchPushedNoPr (no PR API call, a branch-pushed
/// notification); `true` → the PR-open API call fires.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn committed_guide_files_ride_auto_submit_pr_false_as_branch_pushed_no_pr() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td, paths) = crate::testing::test_daemon_paths();

    // Provision the guide onto the agent branch (the precondition pr_open
    // consumes: ordinary committed content on agent-q).
    let executor = CompletingExecutorNoDiff;
    run_pass_with_guide(&ws, &executor, true).await;
    assert!(tracked(&ws, "OCTOPUS.md"));

    let _hook = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    // auto_submit_pr: false MUST NOT call the PR endpoint.
    let pr_mock = server
        .mock("POST", "/repos/owner/fixture/pulls")
        .expect(0)
        .create_async()
        .await;
    test_hooks::set_github_api_base(Some(server.url()));

    let chatops = Arc::new(NotifRecordingChatOps {
        notifications: std::sync::Mutex::new(Vec::new()),
    });
    let ctx = notif_ctx(&chatops);

    let mut repo = fixture_repo(&ws);
    repo.auto_submit_pr = false;

    let res = crate::polling_loop::pr_open::open_pull_request(
        &paths,
        &repo,
        &triage_github_cfg(),
        &[],   // no change work — the guide commit is the only content
        false, // includes_self_heal
        None,  // review_report
        None,  // reviewer
        0,     // revision_cap
        false, // draft
        &[],   // reviewer_revision_concerns
        Some(&ctx),
        &ws,
        None,
        None,
    )
    .await;
    test_hooks::set_github_api_base(None);
    res.expect("auto_submit_pr: false must return Ok without a PR call");

    pr_mock.assert_async().await; // expect(0): no PR opened
    let notifs = chatops.notifications.lock().unwrap().clone();
    assert!(
        notifs.iter().any(|n| n.contains("branch pushed")),
        "BranchPushedNoPr notification expected, got: {notifs:?}"
    );
}

/// 6.6 (true branch): with `auto_submit_pr: true` the guide-carrying agent
/// branch rides the PR-open API call (the endpoint is hit).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn committed_guide_files_ride_auto_submit_pr_true_to_pr_open() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td, paths) = crate::testing::test_daemon_paths();

    let executor = CompletingExecutorNoDiff;
    run_pass_with_guide(&ws, &executor, true).await;
    assert!(tracked(&ws, "OCTOPUS.md"));

    let _hook = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    let pr_mock = server
        .mock("POST", "/repos/owner/fixture/pulls")
        .expect(1)
        .with_status(201)
        .with_body(
            r#"{"number":7,"html_url":"https://github.com/owner/fixture/pull/7"}"#,
        )
        .create_async()
        .await;
    test_hooks::set_github_api_base(Some(server.url()));

    let chatops = Arc::new(NotifRecordingChatOps {
        notifications: std::sync::Mutex::new(Vec::new()),
    });
    let ctx = notif_ctx(&chatops);

    let mut repo = fixture_repo(&ws);
    repo.auto_submit_pr = true;

    let res = crate::polling_loop::pr_open::open_pull_request(
        &paths,
        &repo,
        &triage_github_cfg(),
        &[],
        false,
        None,
        None,
        0,
        false,
        &[],
        Some(&ctx),
        &ws,
        None,
        None,
    )
    .await;
    test_hooks::set_github_api_base(None);
    res.expect("auto_submit_pr: true must open the PR via the seam");

    pr_mock.assert_async().await; // expect(1): the PR endpoint was called
}
