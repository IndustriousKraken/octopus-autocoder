use super::*;

/// Task 4.3: `handle_outcome` receiving `Aborted` from the stub
/// executor returns `QueueStep::Aborted` AND:
/// - drops `.in-progress`
/// - does NOT increment the failure counter
/// - does NOT write `.perma-stuck.json`
/// - leaves `.iteration-pending.json` (if any) untouched
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aborted_arm_drops_lock_and_skips_counter() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "a31-bar", "fixture reason");
    // Establish .in-progress so the arm has something to unlock.
    queue::lock(&ws, "a31-bar").unwrap();
    // Plant an iteration-pending marker — the Aborted arm must
    // leave it in place (mirrors the Failed-arm preservation
    // requirement so the next iteration's continuation context
    // survives a daemon restart mid-iteration).
    let basename = ws.file_name().and_then(|s| s.to_str()).unwrap().to_string();
    let marker = crate::iteration_pending::IterationPendingMarker {
        completed_tasks: vec!["1".into()],
        remaining_tasks: vec!["2".into()],
        reason: "prior".into(),
        iteration_number: 2,
    };
    crate::iteration_pending::write_marker(&paths, &basename, "a31-bar", &marker).unwrap();

    let repo = fixture_repo(&ws);
    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let outcome = Ok(ExecutorOutcome::Aborted {
        reason: "daemon shutdown (SIGTERM cascade)".into(),
    });
    let step = handle_outcome(&paths, &ws, &repo, &github_cfg, None, "a31-bar", outcome)
        .await
        .unwrap();
    assert!(
        matches!(step, QueueStep::Aborted),
        "expected QueueStep::Aborted; got {step:?}"
    );

    // (a) .in-progress dropped.
    assert!(
        !ws.join("openspec/changes/a31-bar/.in-progress").exists(),
        ".in-progress must be dropped by the Aborted arm"
    );

    // (b) The failure counter for the change is NOT recorded.
    let state = crate::failure_state::load(&paths, &ws).unwrap();
    assert!(
        !state.entries.contains_key("a31-bar"),
        "Aborted must NOT increment the failure counter; got {state:?}"
    );

    // (c) .perma-stuck.json is NOT written.
    assert!(
        !crate::perma_stuck::marker_exists(&ws, "a31-bar"),
        ".perma-stuck.json must NOT be written for Aborted"
    );

    // (d) The iteration-pending marker is preserved.
    let still = crate::iteration_pending::read_marker(&paths, &basename, "a31-bar")
        .unwrap()
        .unwrap();
    assert_eq!(
        still, marker,
        "Aborted must NOT touch the iteration-pending marker"
    );
}

/// Task 4.4 (integration): two consecutive `Aborted` outcomes for
/// the same change do NOT trigger perma-stuck (counter stays at 0;
/// marker absent). This is the regression assertion for the
/// production scenario: operator restarts the daemon twice in a
/// row mid-iteration, each restart triggers the SIGTERM cascade,
/// AND the change must not perma-stuck on either occurrence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_consecutive_aborted_outcomes_do_not_perma_stuck() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "a31-bar", "fixture reason");

    let repo = fixture_repo(&ws);
    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };

    for iteration in 0..2u32 {
        // Re-establish the .in-progress lock each pass; production
        // re-locks per pending iteration.
        queue::lock(&ws, "a31-bar").unwrap();
        let outcome = Ok(ExecutorOutcome::Aborted {
            reason: "daemon shutdown (SIGTERM cascade)".into(),
        });
        let step = handle_outcome(&paths, &ws, &repo, &github_cfg, None, "a31-bar", outcome)
            .await
            .unwrap_or_else(|e| panic!("Aborted arm errored on pass {iteration}: {e:#}"));
        assert!(
            matches!(step, QueueStep::Aborted),
            "pass {iteration}: expected QueueStep::Aborted; got {step:?}"
        );

        let state = crate::failure_state::load(&paths, &ws).unwrap();
        assert!(
            !state.entries.contains_key("a31-bar"),
            "pass {iteration}: counter must remain absent after Aborted; got {state:?}"
        );
        assert!(
            !crate::perma_stuck::marker_exists(&ws, "a31-bar"),
            "pass {iteration}: .perma-stuck.json must NOT be written"
        );
    }
}
