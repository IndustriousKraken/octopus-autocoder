use super::*;

// spec-revision-pr-parks-change: the blocking-markers gate parks a pending
// (unmarked) change when an open PR exists on its spec-revision branch
// `<agent_branch>-spec-revision-<change>`. These exercise `handle_blocking_markers_gate`
// directly against a mockito `find_pr_by_head` (GET /pulls?state=open&head=...).

/// A JSON body for one open PR on the queried head branch (the `find_pr_by_head`
/// / `list_open_prs_for_head` response shape).
fn one_open_pr_body(number: u64, head_ref: &str) -> String {
    format!(
        r#"[{{
            "number": {number},
            "title": "spec revision",
            "state": "open",
            "html_url": "https://github.com/owner/fixture/pull/{number}",
            "body": null,
            "created_at": "2026-07-12T10:00:00Z",
            "head": {{"ref": "{head_ref}"}},
            "base": {{"ref": "main"}}
        }}]"#
    )
}

/// Call the gate with the standard test-fixture argument set.
async fn run_gate(
    paths: &DaemonPaths,
    ws: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    pending: &[String],
) -> bool {
    handle_blocking_markers_gate(
        paths,
        ws,
        repo,
        github_cfg,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        None,
        &std::sync::Mutex::new(Vec::new()),
        pending,
        0,
    )
    .await
    .expect("gate must not error")
}

/// 2.1: a pending change with an open spec-revision PR (and NO marker) is
/// blocked, and the gate writes no `.needs-spec-revision.json`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_spec_revision_pr_parks_change_without_marker_write() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "parked-change", "fixture");

    let _hook = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    let pr_mock = server
        .mock("GET", "/repos/owner/fixture/pulls")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("state".into(), "open".into()),
            mockito::Matcher::UrlEncoded(
                "head".into(),
                "owner:agent-q-spec-revision-parked-change".into(),
            ),
        ]))
        .with_status(200)
        .with_body(one_open_pr_body(42, "agent-q-spec-revision-parked-change"))
        .expect(1)
        .create_async()
        .await;
    test_hooks::set_github_api_base(Some(server.url()));

    let repo = fixture_repo(&ws);
    let blocked = run_gate(
        &paths,
        &ws,
        &repo,
        &triage_github_cfg(),
        &["parked-change".to_string()],
    )
    .await;
    test_hooks::set_github_api_base(None);

    assert!(blocked, "an open spec-revision PR must park the change (block)");
    assert!(
        !ws.join("openspec/changes/parked-change/.needs-spec-revision.json").exists(),
        "parking must NOT write a .needs-spec-revision.json marker"
    );
    pr_mock.assert_async().await; // the spec-revision branch WAS queried
}

/// 2.2: a change carrying a `.needs-spec-revision.json` marker short-circuits on
/// the marker — the gate blocks WITHOUT making any forge call for the PR check.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn marker_present_short_circuits_before_forge_call() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "marked-change", "fixture");
    // Plant the marker the gate must short-circuit on.
    std::fs::write(
        ws.join("openspec/changes/marked-change/.needs-spec-revision.json"),
        "{}",
    )
    .unwrap();

    let _hook = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    // The PR endpoint must NEVER be hit — the marker check is the early exit.
    let pr_mock = server
        .mock("GET", "/repos/owner/fixture/pulls")
        .expect(0)
        .create_async()
        .await;
    test_hooks::set_github_api_base(Some(server.url()));

    let repo = fixture_repo(&ws);
    let blocked = run_gate(
        &paths,
        &ws,
        &repo,
        &triage_github_cfg(),
        &["marked-change".to_string()],
    )
    .await;
    test_hooks::set_github_api_base(None);

    assert!(blocked, "the marker must block the queue");
    pr_mock.assert_async().await; // expect(0): no spec-revision PR query was made
}

/// 2.3: a forge error on the spec-revision-branch query fails OPEN — the gate
/// proceeds as if no spec-revision PR exists (does not block, writes no marker).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forge_error_fails_open_does_not_park() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "err-change", "fixture");

    let _hook = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    let pr_mock = server
        .mock("GET", "/repos/owner/fixture/pulls")
        .match_query(mockito::Matcher::Any)
        .with_status(500)
        .with_body("upstream boom")
        .expect(1)
        .create_async()
        .await;
    test_hooks::set_github_api_base(Some(server.url()));

    let repo = fixture_repo(&ws);
    let blocked = run_gate(
        &paths,
        &ws,
        &repo,
        &triage_github_cfg(),
        &["err-change".to_string()],
    )
    .await;
    test_hooks::set_github_api_base(None);

    assert!(
        !blocked,
        "a transient forge error must fail open (not park the change)"
    );
    assert!(
        !ws.join("openspec/changes/err-change/.needs-spec-revision.json").exists(),
        "fail-open must NOT write a marker"
    );
    pr_mock.assert_async().await;
}
