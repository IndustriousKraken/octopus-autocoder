//! spec-revision-pr-parks-change: the blocking-markers gate parks a pending
//! change whose spec revision is in flight (an open PR on
//! `<agent_branch>-spec-revision-<change>`), without re-running the spec gate
//! or re-writing the `.needs-spec-revision.json` marker the spec-revision
//! executor just cleared.
use super::*;

/// One open-PR JSON element shaped like the `PrSummary` the forge deserializes
/// (`find_pr_by_head` → `list_open_prs_for_head`).
fn open_pr_json(number: u64, head_ref: &str) -> String {
    format!(
        r#"[{{"number":{number},"title":"revise spec","html_url":"https://github.com/upstream-owner/upstream-repo/pull/{number}","state":"open","created_at":"2026-07-14T00:00:00Z","head":{{"ref":"{head_ref}"}},"base":{{"ref":"main"}}}}]"#
    )
}

/// 2.1: a pending change with NO marker but an open spec-revision PR is parked.
/// The gate returns blocking AND writes no `.needs-spec-revision.json`.
#[tokio::test]
async fn spec_revision_open_pr_parks_change_without_marker() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/repos/upstream-owner/upstream-repo/pulls")
        .match_query(mockito::Matcher::UrlEncoded(
            "head".into(),
            "upstream-owner:agent-q-spec-revision-my-change".into(),
        ))
        .with_status(200)
        .with_body(open_pr_json(42, "agent-q-spec-revision-my-change"))
        .expect(1)
        .create_async()
        .await;

    let (_td, paths) = crate::testing::test_daemon_paths();
    let ws = tempfile::TempDir::new().unwrap();
    // The change dir exists but carries NO blocking marker.
    std::fs::create_dir_all(ws.path().join("openspec/changes/my-change")).unwrap();

    let _hook = test_hooks::lock();
    test_hooks::set_github_api_base(Some(server.url()));
    let blocked = handle_blocking_markers_gate(
        &paths,
        ws.path(),
        &open_pr_test_repo(),
        &open_pr_test_github(&server.url()),
        &["my-change".to_string()],
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        None,
        &std::sync::Mutex::new(Vec::new()),
        0,
    )
    .await
    .expect("gate runs");
    test_hooks::set_github_api_base(None);

    assert!(blocked, "an open spec-revision PR must park the change");
    assert!(
        !ws.path()
            .join("openspec/changes/my-change/.needs-spec-revision.json")
            .exists(),
        "the gate must NOT re-write the needs-spec-revision marker"
    );
    mock.assert_async().await;
}

/// 2.2: a change with BOTH a `.needs-spec-revision.json` marker AND an open
/// spec-revision PR short-circuits on the marker — the forge is never queried.
#[tokio::test]
async fn marker_short_circuits_before_spec_revision_pr_check() {
    let mut server = mockito::Server::new_async().await;
    // If the gate wrongly queried the forge, this would match — expect(0)
    // asserts the marker check is the early-exit (no PR call is made).
    let mock = server
        .mock("GET", "/repos/upstream-owner/upstream-repo/pulls")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_body(open_pr_json(42, "agent-q-spec-revision-my-change"))
        .expect(0)
        .create_async()
        .await;

    let (_td, paths) = crate::testing::test_daemon_paths();
    let ws = tempfile::TempDir::new().unwrap();
    let change_dir = ws.path().join("openspec/changes/my-change");
    std::fs::create_dir_all(&change_dir).unwrap();
    std::fs::write(change_dir.join(".needs-spec-revision.json"), "{}").unwrap();

    let _hook = test_hooks::lock();
    test_hooks::set_github_api_base(Some(server.url()));
    let blocked = handle_blocking_markers_gate(
        &paths,
        ws.path(),
        &open_pr_test_repo(),
        &open_pr_test_github(&server.url()),
        &["my-change".to_string()],
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        None,
        &std::sync::Mutex::new(Vec::new()),
        0,
    )
    .await
    .expect("gate runs");
    test_hooks::set_github_api_base(None);

    assert!(blocked, "the marker alone blocks the queue");
    mock.assert_async().await; // expect(0): no forge call for the PR check
}

/// 2.3: a forge query error for the spec-revision branch fails open — the gate
/// proceeds as if no spec-revision PR exists and does NOT park the change.
#[tokio::test]
async fn spec_revision_pr_query_error_fails_open() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", mockito::Matcher::Any)
        .with_status(500)
        .with_body(r#"{"message":"server error"}"#)
        .create_async()
        .await;

    let (_td, paths) = crate::testing::test_daemon_paths();
    let ws = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(ws.path().join("openspec/changes/my-change")).unwrap();

    let _hook = test_hooks::lock();
    test_hooks::set_github_api_base(Some(server.url()));
    let blocked = handle_blocking_markers_gate(
        &paths,
        ws.path(),
        &open_pr_test_repo(),
        &open_pr_test_github(&server.url()),
        &["my-change".to_string()],
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        None,
        &std::sync::Mutex::new(Vec::new()),
        0,
    )
    .await
    .expect("gate runs (fail-open, not an error)");
    test_hooks::set_github_api_base(None);

    assert!(
        !blocked,
        "a transient forge error must fail open (no park), not block the queue"
    );
}
