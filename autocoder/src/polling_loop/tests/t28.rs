//! durable-iteration-record: the iteration driver (`run_iteration_work`) stamps
//! a per-workspace iteration record at the SINGLE point the pass result is
//! handled. Idle, failed, and success-with-work iterations each OVERWRITE the
//! record with the matching outcome kind and a fresh `finished_at`.

use super::*;
use crate::iteration_record::{IterationOutcome, OutcomeKind, record_for};

/// Run one iteration through the real driver with the minimal arg set: no
/// reviewer, no chatops, audits disabled, revisions disabled.
#[allow(clippy::too_many_arguments)]
async fn drive_one_iteration(
    paths: &DaemonPaths,
    ws: &Path,
    executor: &dyn Executor,
    github: &GithubConfig,
) {
    let mut counter = 0u32;
    run_iteration_work(
        paths,
        ws,
        &fixture_repo(ws),
        executor,
        github,
        None,       // reviewer
        None,       // chatops
        false,      // want_rebuild
        &std::sync::Mutex::new(Vec::new()),
        2400u64,    // stuck_threshold_secs
        u32::MAX,   // perma_stuck_threshold
        u32::MAX,   // max_changes_per_pr
        0,          // revision_cap (disabled)
        Some(10),   // human_revise_cap
        &crate::audits::AuditRegistry::default(),
        None,       // audits_cfg
        &std::collections::HashMap::new(),
        &mut counter,
    )
    .await;
}

/// Seed a stale record so the assertions prove OVERWRITE, not first-write.
fn seed_stale_record(paths: &DaemonPaths, ws: &Path) {
    let stale = record_for(
        &Err(anyhow::anyhow!("weeks-old failure")),
        chrono::Utc::now() - chrono::Duration::days(20),
        1,
    );
    crate::iteration_record::write(paths, ws, &stale).unwrap();
}

/// An idle iteration (empty queue, no commits) overwrites the record with an
/// `idle` outcome and a fresh `finished_at`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_iteration_overwrites_record_with_idle() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td, paths) = crate::testing::test_daemon_paths();
    seed_stale_record(&paths, &ws);

    let github = open_pr_gate_ok_github();
    let _hook = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    let _gate = mock_open_pr_gate_empty(&mut server).await;
    test_hooks::set_github_api_base(Some(server.url()));

    let before = chrono::Utc::now();
    drive_one_iteration(&paths, &ws, &CompletingExecutorNoDiff, &github).await;
    test_hooks::set_github_api_base(None);

    let rec = crate::iteration_record::read(&paths, &ws).expect("record written");
    assert_eq!(rec.outcome_kind, OutcomeKind::Idle, "empty queue → idle");
    assert!(rec.finished_at >= before, "finished_at is fresh, not the 20-day-old seed");
}

/// A success-with-work iteration (one pending change archived) overwrites the
/// record with a `success_with_work` outcome that NAMES the change.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn success_iteration_overwrites_record_naming_the_change() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td, paths) = crate::testing::test_daemon_paths();
    seed_stale_record(&paths, &ws);
    add_committed_change(&ws, "a05-foo", "ship the thing");

    let github = open_pr_gate_ok_github();
    let _hook = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    let _gate = mock_open_pr_gate_empty(&mut server).await;
    let _pr = server
        .mock("POST", mockito::Matcher::Regex("/pulls".to_string()))
        .with_status(201)
        .with_body(r#"{"html_url":"https://github.com/owner/fixture/pull/1","number":1}"#)
        .create_async()
        .await;
    test_hooks::set_github_api_base(Some(server.url()));

    let before = chrono::Utc::now();
    let executor = CompletingExecutorWithDiff {
        artifact_name: "art.txt".into(),
        artifact_text: "x".into(),
    };
    drive_one_iteration(&paths, &ws, &executor, &github).await;
    test_hooks::set_github_api_base(None);

    let rec = crate::iteration_record::read(&paths, &ws).expect("record written");
    assert_eq!(rec.outcome_kind, OutcomeKind::SuccessWithWork);
    assert!(
        rec.outcome_summary.contains("a05-foo"),
        "outcome names the archived change: {}",
        rec.outcome_summary
    );
    assert!(rec.finished_at >= before, "finished_at is fresh");
}

/// A failed iteration (branch push fails on a broken remote) overwrites the
/// record with a `failed` outcome carrying a reason — the write happens even
/// though the pass returned `Err`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_iteration_overwrites_record_with_failed() {
    let (_dir, ws) = fixture_workspace_with_broken_remote("t28-failed");
    let (_td, paths) = crate::testing::test_daemon_paths();
    seed_stale_record(&paths, &ws);
    add_committed_change(&ws, "a06-bar", "work that will fail to push");

    let github = open_pr_gate_ok_github();
    let _hook = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    let _gate = mock_open_pr_gate_empty(&mut server).await;
    test_hooks::set_github_api_base(Some(server.url()));

    let before = chrono::Utc::now();
    let executor = CompletingExecutorWithDiff {
        artifact_name: "art.txt".into(),
        artifact_text: "x".into(),
    };
    drive_one_iteration(&paths, &ws, &executor, &github).await;
    test_hooks::set_github_api_base(None);

    let rec = crate::iteration_record::read(&paths, &ws).expect("record written even on Err");
    assert_eq!(rec.outcome_kind, OutcomeKind::Failed, "a push failure records failed");
    assert!(!rec.outcome_summary.is_empty(), "failed record carries a reason");
    assert!(rec.finished_at >= before, "finished_at is fresh");
}

/// Guard the outcome→record mapping directly: the five kinds map to the five
/// persisted kinds (mirrors the unit test in `iteration_record`, kept here so a
/// driver-level regression that swaps kinds is caught alongside the fixtures).
#[test]
fn outcome_kinds_map_to_persisted_kinds() {
    let now = chrono::Utc::now();
    assert_eq!(
        record_for(&Ok(IterationOutcome::Idle), now, 0).outcome_kind,
        OutcomeKind::Idle
    );
    assert_eq!(
        record_for(&Ok(IterationOutcome::AuditOnly), now, 0).outcome_kind,
        OutcomeKind::AuditOnly
    );
    assert_eq!(
        record_for(
            &Ok(IterationOutcome::Skipped { park: "open PR".into() }),
            now,
            0
        )
        .outcome_kind,
        OutcomeKind::Skipped
    );
}

/// A no-commit pass with a change waiting on a human answer records a
/// `skipped` park naming the block — NOT `idle` ("empty queue — nothing to
/// do"), which would misreport a human-blocked queue as empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waiting_blocked_iteration_records_skipped_park_not_idle() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td, paths) = crate::testing::test_daemon_paths();
    seed_stale_record(&paths, &ws);

    // A waiting change: enumerated by `list_waiting` via its `.question.json`.
    // With no chatops context the resume step cannot poll, so the change stays
    // waiting and blocks the pending walk — zero commits result.
    let waiting_dir = ws.join("openspec/changes/w1-waiting");
    std::fs::create_dir_all(&waiting_dir).unwrap();
    std::fs::write(waiting_dir.join(".question.json"), "{}").unwrap();

    let github = open_pr_gate_ok_github();
    let _hook = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    let _gate = mock_open_pr_gate_empty(&mut server).await;
    test_hooks::set_github_api_base(Some(server.url()));
    drive_one_iteration(&paths, &ws, &CompletingExecutorNoDiff, &github).await;
    test_hooks::set_github_api_base(None);

    let rec = crate::iteration_record::read(&paths, &ws).expect("record written");
    assert_eq!(
        rec.outcome_kind,
        OutcomeKind::Skipped,
        "waiting block is a park, not idle: {}",
        rec.outcome_summary
    );
    assert!(
        rec.outcome_summary.contains("queue blocked")
            && rec.outcome_summary.contains("w1-waiting"),
        "park names the block: {}",
        rec.outcome_summary
    );
}
