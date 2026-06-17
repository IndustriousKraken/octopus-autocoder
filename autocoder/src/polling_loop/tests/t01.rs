use super::*;

/// 7.4: code-only outcome → NO PR, "no spec or issue content" reply, tree
/// clean, status TriageFailed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a43_audit_code_only_opens_no_pr() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td, paths) = crate::testing::test_daemon_paths();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src/foo.rs"), "agent code\n").unwrap();

    let _hook = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    let pr_mock = server
        .mock("POST", "/repos/owner/fixture/pulls")
        .expect(0)
        .create_async()
        .await;
    test_hooks::set_github_api_base(Some(server.url()));

    let chatops = Arc::new(RecordingChatOps {
        replies: std::sync::Mutex::new(Vec::new()),
    });
    let ctx = recording_ctx(&chatops);
    let mut state = audit_state();
    let res = process_completed_triage(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &triage_github_cfg(),
        Some(&ctx),
        &mut state,
        None,
    )
    .await;
    test_hooks::set_github_api_base(None);
    res.expect("handler must succeed");

    pr_mock.assert_async().await; // expect(0): no PR opened
    assert_eq!(
        state.status,
        crate::audits::threads::AuditThreadStatus::TriageFailed
    );
    assert!(
        !ws.join("src/foo.rs").exists(),
        "code write must be restored away"
    );
    assert_eq!(
        crate::git::status_porcelain(&ws).unwrap(),
        "",
        "working tree must be clean after the handler returns"
    );
    let replies = chatops.replies.lock().unwrap().clone();
    assert!(
        replies.iter().any(|r| r.contains("no spec or issue content produced")),
        "the no-content reply must be posted, got {replies:?}"
    );
}

/// 7.4 (chat): code-only → NO PR, TriageFailed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a43_chat_code_only_opens_no_pr() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td, paths) = crate::testing::test_daemon_paths();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src/foo.rs"), "agent code\n").unwrap();

    let _hook = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    let pr_mock = server
        .mock("POST", "/repos/owner/fixture/pulls")
        .expect(0)
        .create_async()
        .await;
    test_hooks::set_github_api_base(Some(server.url()));

    let chatops = Arc::new(RecordingChatOps {
        replies: std::sync::Mutex::new(Vec::new()),
    });
    let ctx = recording_ctx(&chatops);
    let mut state = proposal_state();
    let res = process_completed_proposal(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &triage_github_cfg(),
        Some(&ctx),
        &mut state,
        None,
    )
    .await;
    test_hooks::set_github_api_base(None);
    res.expect("handler must succeed");

    pr_mock.assert_async().await;
    assert_eq!(
        state.status,
        crate::proposal_requests::ProposalRequestStatus::TriageFailed
    );
    assert_eq!(
        crate::git::status_porcelain(&ws).unwrap(),
        "",
        "tree must be clean"
    );
    let replies = chatops.replies.lock().unwrap().clone();
    assert!(
        replies.iter().any(|r| r.contains("no spec content")),
        "no-spec reply expected"
    );
}

/// Empty-diff audit outcome → no PR, no-action reply carries the
/// executor's final summary, status Acted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a43_audit_empty_diff_posts_no_action_reply() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td, paths) = crate::testing::test_daemon_paths();

    let _hook = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    let pr_mock = server
        .mock("POST", "/repos/owner/fixture/pulls")
        .expect(0)
        .create_async()
        .await;
    test_hooks::set_github_api_base(Some(server.url()));

    let chatops = Arc::new(RecordingChatOps {
        replies: std::sync::Mutex::new(Vec::new()),
    });
    let ctx = recording_ctx(&chatops);
    let mut state = audit_state();
    let res = process_completed_triage(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &triage_github_cfg(),
        Some(&ctx),
        &mut state,
        Some("Nothing actionable in these findings."),
    )
    .await;
    test_hooks::set_github_api_base(None);
    res.expect("handler must succeed");

    pr_mock.assert_async().await;
    assert_eq!(
        state.status,
        crate::audits::threads::AuditThreadStatus::Acted
    );
    let replies = chatops.replies.lock().unwrap().clone();
    assert!(
        // Behavioral: the reply carries the executor's summary (fixture text),
        // not a hand-authored shipped phrase.
        replies
            .iter()
            .any(|r| r.contains("Nothing actionable in these findings.")),
        "no-action reply must carry the executor's summary, got {replies:?}"
    );
}

/// Routing test: when `owner_tokens` maps the parsed URL owner to an
/// env var, the PR-creation HTTP call MUST carry that env var's value
/// in the `Authorization: Bearer` header — not `token_env`'s value.
/// This exercises the same composition `open_pull_request` does:
/// `parse_repo_url → resolve_token → create_pull_request_at`.
#[tokio::test]
async fn pr_creation_uses_owner_specific_token() {
    let var = "AUTOCODER_TEST_PR_ROUTING_TOKEN";
    let fallback = "AUTOCODER_TEST_PR_ROUTING_FALLBACK";
    // SAFETY: this test relies on a unique env-var name so it does not
    // collide with parallel tests; no cross-test mutation lock required.
    unsafe {
        std::env::set_var(var, "owner-specific-token-xyz");
        std::env::set_var(fallback, "should-not-be-used");
    }

    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/repos/fixture-owner/fixture-repo/pulls")
        .match_header("authorization", "Bearer owner-specific-token-xyz")
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"html_url":"https://github.com/fixture-owner/fixture-repo/pull/1","number":1}"#,
        )
        .create_async()
        .await;

    let mut map = std::collections::HashMap::new();
    map.insert(
        "fixture-owner".into(),
        crate::config::SecretSource::EnvVar(var.into()),
    );
    let github_cfg = GithubConfig {
        token_env: fallback.into(),
        token: None,
        owner_tokens: Some(map),
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };

    // Mirror open_pull_request's internal sequence.
    let (owner, repo_name) =
        crate::github::parse_repo_url("git@github.com:fixture-owner/fixture-repo.git")
            .expect("parse");
    let token = crate::github_credentials::resolve_token(&github_cfg, &owner)
        .expect("owner_tokens entry should resolve");

    crate::github::create_pull_request_at_for_test(
        &server.url(),
        &owner,
        &repo_name,
        "agent-q",
        "main",
        "t",
        "b",
        &token,
        None,
        false,
    )
    .await
    .expect("PR creation should succeed against mockito");

    mock.assert_async().await;

    unsafe {
        std::env::remove_var(var);
        std::env::remove_var(fallback);
    }
}

/// In fork-PR mode the PR's `head` is `<fork-owner>:<branch>` and the
/// API call still goes to the upstream repo's /pulls endpoint.
#[tokio::test]
async fn pr_uses_cross_repo_head_in_fork_mode() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/repos/upstream-org/repo/pulls")
        .match_body(mockito::Matcher::PartialJsonString(
            r#"{"head":"machine-user:agent-q","base":"main"}"#.to_string(),
        ))
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body(r#"{"html_url":"https://github.com/upstream-org/repo/pull/1","number":1}"#)
        .create_async()
        .await;

    // Mirror the open_pull_request flow with fork_owner set.
    let github_cfg = GithubConfig {
        token_env: "X".into(),
        token: Some(crate::config::SecretSource::Inline {
            value: "inline-token".into(),
        }),
        owner_tokens: None,
        fork_owner: Some("machine-user".into()),
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let (owner, repo_name) =
        crate::github::parse_repo_url("git@github.com:upstream-org/repo.git").unwrap();
    let token = crate::github_credentials::resolve_token(&github_cfg, &owner).unwrap();
    let head = format!(
        "{}:{}",
        github_cfg.fork_owner.as_deref().unwrap(),
        "agent-q"
    );

    crate::github::create_pull_request_at_for_test(
        &server.url(),
        &owner,
        &repo_name,
        &head,
        "main",
        "t",
        "b",
        &token,
        None,
        false,
    )
    .await
    .expect("cross-repo PR succeeds");

    mock.assert_async().await;
}

#[test]
fn detect_lazy_archive_returns_true_for_archive_only_renames() {
    let status = "R  openspec/changes/foo/proposal.md -> openspec/changes/archive/2026-05-14-foo/proposal.md\nR  openspec/changes/foo/tasks.md -> openspec/changes/archive/2026-05-14-foo/tasks.md\n";
    assert!(is_lazy_archive(status));
}

#[test]
fn detect_lazy_archive_returns_false_when_real_implementation_present() {
    // Archive rename PLUS a modification to a source file → real work.
    let status = "R  openspec/changes/foo/proposal.md -> openspec/changes/archive/2026-05-14-foo/proposal.md\n M src/foo.rs\n";
    assert!(!is_lazy_archive(status));
}

#[test]
fn detect_lazy_archive_returns_false_for_added_files() {
    let status = "A  src/new_module.rs\n";
    assert!(!is_lazy_archive(status));
}

#[test]
fn detect_lazy_archive_returns_false_when_workspace_clean() {
    assert!(!is_lazy_archive(""));
}

#[test]
fn detect_lazy_archive_returns_false_for_rename_outside_archive() {
    // Renames are fine if they're not into archive/ — agent legitimately
    // moving files around as part of implementation.
    let status = "R  old/path.rs -> new/path.rs\n";
    assert!(!is_lazy_archive(status));
}

#[test]
fn has_executor_changes_false_when_only_question_file_deletion() {
    // Real-world porcelain from a no-diff resume: autocoder itself
    // deleted .question.json before calling resume; the leading
    // column-1 space is trimmed by `status_porcelain`, leaving the
    // line starting with the second status column.
    let status = "D openspec/changes/foo/.question.json";
    assert!(!has_executor_changes(status, "foo"));
}

#[test]
fn has_executor_changes_false_when_only_answer_and_question_metafiles() {
    let status = " D openspec/changes/foo/.question.json\n?? openspec/changes/foo/.answer.json";
    assert!(!has_executor_changes(status, "foo"));
}

#[test]
fn has_executor_changes_true_when_resume_wrote_artifact() {
    // The executor created an artifact alongside the meta-file
    // deletion → real work happened.
    let status = " D openspec/changes/foo/.question.json\n?? src/new_thing.rs";
    assert!(has_executor_changes(status, "foo"));
}

#[test]
fn has_executor_changes_false_on_empty_status() {
    assert!(!has_executor_changes("", "foo"));
}

#[test]
fn has_executor_changes_true_for_rename_with_non_meta_path() {
    let status = "R  old/path.rs -> new/path.rs";
    assert!(has_executor_changes(status, "foo"));
}

#[test]
fn first_line_of_why_section() {
    let text = "## Why\nSwitch from sync to async\n\n## What Changes\n- thing\n";
    let line = first_line_of_section(text, "## Why").unwrap();
    assert_eq!(line, "Switch from sync to async");
}

#[test]
fn first_line_of_why_skips_blank_lines() {
    let text = "## Why\n\n   \n  Real content here  \n## What Changes\n";
    let line = first_line_of_section(text, "## Why").unwrap();
    assert_eq!(line, "Real content here");
}

#[test]
fn first_line_of_section_returns_none_when_missing() {
    let text = "## What Changes\n- x\n";
    assert!(first_line_of_section(text, "## Why").is_none());
}

#[test]
fn build_commit_subject_truncates_to_72_chars() {
    let dir = tempfile::TempDir::new().unwrap();
    let ws = dir.path();
    let change = "make-the-thing-better";
    let proposal = ws.join("openspec/changes").join(change).join("proposal.md");
    std::fs::create_dir_all(proposal.parent().unwrap()).unwrap();
    let long = "A".repeat(200);
    std::fs::write(&proposal, format!("## Why\n{long}\n")).unwrap();
    let subject = build_commit_subject(ws, change).unwrap();
    assert_eq!(subject.chars().count(), 72);
    assert!(subject.starts_with("make-the-thing-better: "));
}

#[test]
fn build_commit_subject_falls_back_to_change_name_when_no_why() {
    let dir = tempfile::TempDir::new().unwrap();
    let ws = dir.path();
    let proposal = ws.join("openspec/changes/c/proposal.md");
    std::fs::create_dir_all(proposal.parent().unwrap()).unwrap();
    std::fs::write(&proposal, "## What Changes\n- thing\n").unwrap();
    let subject = build_commit_subject(ws, "c").unwrap();
    assert_eq!(subject, "c: c");
}

/// Task 4.1: default-disabled (no `[out]` ctx scoped) → the gate is a no-op,
/// returns no section, AND posts no chatops note (PR assembly unchanged).
#[tokio::test]
async fn out_gate_disabled_produces_no_section() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let repo = fixture_repo(&ws);
    let chatops = std::sync::Arc::new(NotifRecordingChatOps {
        notifications: Default::default(),
    });
    let ctx = notif_ctx(&chatops);
    // No `code_implements_spec::scope` wrapping → `current()` is None.
    let section = run_spec_verification_gate(&ws, &repo, &["c1".to_string()], Some(&ctx)).await;
    assert!(section.is_none(), "disabled gate must produce no section");
    assert!(
        chatops.notifications.lock().unwrap().is_empty(),
        "no chatops note when disabled"
    );
}

/// Task 4.3: an `implemented` verdict renders a clean `## Spec Verification`
/// section AND posts NO chatops note.
#[tokio::test]
async fn out_gate_implemented_renders_section_no_chatops() {
    let (_dir, ws) = fixture_workspace_with_remote();
    seed_agent_branch_with_change(&ws);
    let repo = fixture_repo(&ws);
    let chatops = std::sync::Arc::new(NotifRecordingChatOps {
        notifications: Default::default(),
    });
    let ctx = notif_ctx(&chatops);
    let gate_ctx = std::sync::Arc::new(out_gate_ctx(Some(serde_json::json!({
        "verdict": "implemented", "summary": "satisfied", "gaps": []
    }))));
    let section = crate::code_implements_spec::scope(
        Some(gate_ctx),
        run_spec_verification_gate(&ws, &repo, &["c1".to_string()], Some(&ctx)),
    )
    .await;
    let section = section.expect("implemented verdict renders a section");
    assert!(section.starts_with("## Spec Verification"));
    assert!(
        chatops.notifications.lock().unwrap().is_empty(),
        "implemented must post no chatops note"
    );
}

/// Task 4.4: a `gaps_found` verdict renders the gaps in the section AND
/// posts a chatops note. This function only returns the section AND posts
/// the heads-up — it opens NO revision and the caller always proceeds to PR
/// creation (no block).
#[tokio::test]
async fn out_gate_gaps_found_renders_section_and_posts_chatops() {
    let (_dir, ws) = fixture_workspace_with_remote();
    seed_agent_branch_with_change(&ws);
    let repo = fixture_repo(&ws);
    let chatops = std::sync::Arc::new(NotifRecordingChatOps {
        notifications: Default::default(),
    });
    let ctx = notif_ctx(&chatops);
    let gate_ctx = std::sync::Arc::new(out_gate_ctx(Some(serde_json::json!({
        "verdict": "gaps_found",
        "summary": "one gap",
        "gaps": [
            { "requirement": "A", "scenario": null, "status": "missing", "evidence": "no code realizes it" }
        ]
    }))));
    let section = crate::code_implements_spec::scope(
        Some(gate_ctx),
        run_spec_verification_gate(&ws, &repo, &["c1".to_string()], Some(&ctx)),
    )
    .await;
    let section = section.expect("gaps_found verdict renders a section");
    assert!(section.starts_with("## Spec Verification"));
    assert!(
        section.contains("no code realizes it"),
        "section lists the gap evidence: {section}"
    );
    assert_eq!(
        chatops.notifications.lock().unwrap().len(),
        1,
        "gaps_found posts exactly one advisory chatops note"
    );
}

/// A session that yields no verdict (the gate could not run) → FAIL CLOSED to a
/// VISIBLE state: render an explicit `## Spec Verification: FAILED TO RUN`
/// section (NOT silence, NOT a pass). Still advisory — no chatops note, and the
/// caller still creates the PR (the gate never blocks).
#[tokio::test]
async fn out_gate_no_submission_renders_failed_to_run() {
    let (_dir, ws) = fixture_workspace_with_remote();
    seed_agent_branch_with_change(&ws);
    let repo = fixture_repo(&ws);
    let chatops = std::sync::Arc::new(NotifRecordingChatOps {
        notifications: Default::default(),
    });
    let ctx = notif_ctx(&chatops);
    // `Some(None)` → the canned runner simulates "agent never submitted".
    let gate_ctx = std::sync::Arc::new(out_gate_ctx(None));
    let section = crate::code_implements_spec::scope(
        Some(gate_ctx),
        run_spec_verification_gate(&ws, &repo, &["c1".to_string()], Some(&ctx)),
    )
    .await;
    let section =
        section.expect("fail-closed: the [out] gate renders FAILED TO RUN, not silence");
    assert!(
        section.contains("FAILED TO RUN"),
        "section reports the gate could not run: {section}"
    );
    assert!(
        section.contains("NOT verified"),
        "section makes clear it is NOT a pass: {section}"
    );
    assert!(
        chatops.notifications.lock().unwrap().is_empty(),
        "no chatops note for a failed-to-run advisory gate (it's surfaced in the PR section)"
    );
}

/// Task 4.4 (no-block): an audit-only iteration (empty `processed`) skips
/// the gate entirely even when enabled — there are no implementer changes to
/// verify — so no section is produced.
#[tokio::test]
async fn out_gate_skips_audit_only_iteration() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let repo = fixture_repo(&ws);
    let chatops = std::sync::Arc::new(NotifRecordingChatOps {
        notifications: Default::default(),
    });
    let ctx = notif_ctx(&chatops);
    let gate_ctx = std::sync::Arc::new(out_gate_ctx(Some(serde_json::json!({
        "verdict": "implemented", "summary": "x", "gaps": []
    }))));
    let section = crate::code_implements_spec::scope(
        Some(gate_ctx),
        // Empty `processed` → audit-only; the gate is a no-op.
        run_spec_verification_gate(&ws, &repo, &[], Some(&ctx)),
    )
    .await;
    assert!(
        section.is_none(),
        "audit-only iteration produces no section"
    );
}

#[test]
fn opportunistic_upstream_fetch_no_block_no_action() {
    // Upstream unconfigured: function is a no-op.
    let dir = tempfile::TempDir::new().unwrap();
    let bare = dir.path().join("bare.git");
    init_bare(&bare);
    let workspace = dir.path().join("workspace");
    init_clone(&bare, &workspace);
    let repo = fixture_repo(&workspace);
    // Capture pre-state: no `upstream` remote.
    assert!(remote_url(&workspace, "upstream").is_none());
    opportunistic_upstream_fetch(&workspace, &repo);
    // Still no `upstream` remote — function did nothing.
    assert!(remote_url(&workspace, "upstream").is_none());
}

#[test]
fn opportunistic_upstream_fetch_adds_remote_and_fetches() {
    let dir = tempfile::TempDir::new().unwrap();
    let bare = dir.path().join("bare.git");
    init_bare(&bare);
    let upstream_bare = dir.path().join("upstream.git");
    init_bare(&upstream_bare);
    let workspace = dir.path().join("workspace");
    init_clone(&bare, &workspace);
    let mut repo = fixture_repo(&workspace);
    repo.upstream = Some(crate::config::UpstreamConfig {
        remote: "upstream".to_string(),
        branch: "main".to_string(),
        url: upstream_bare.to_string_lossy().to_string(),
    });
    opportunistic_upstream_fetch(&workspace, &repo);
    let url = remote_url(&workspace, "upstream").expect("upstream remote should be added");
    assert_eq!(url, upstream_bare.to_string_lossy().to_string());
}
