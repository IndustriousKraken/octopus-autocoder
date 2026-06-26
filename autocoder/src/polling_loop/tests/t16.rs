use super::*;

/// 2.4 (a12): with 1 pending change AND 0 eligible audits, only the
/// change processes; no audit work happens. (Sanity check that the
/// reorder did not accidentally couple the two phases.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_only_iteration_runs_no_audit_work() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "only-change", "solo pending");

    let log = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let executor = OrderRecordingExecutor { log: log.clone() };
    // Empty registry: no audits to run, so the scheduler is a no-op.
    let registry = crate::audits::AuditRegistry::default();

    let test_github = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
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
        None,
        u32::MAX,
        u32::MAX,
        &registry,
        None,
        &std::collections::HashMap::new(),
        &std::sync::Mutex::new(Vec::new()),
    )
    .await
    .expect("pass succeeds");

    assert_eq!(processed, vec!["only-change".to_string()]);
    let entries = log.lock().unwrap().clone();
    assert_eq!(entries, vec!["executor:only-change".to_string()]);
}

/// Pure-function test: title shape for an audit-only iteration.
#[test]
fn build_audit_only_pr_title_single_audit() {
    let subjects = vec!["audit: security_bug proposals (1 change(s))".to_string()];
    let title = build_audit_only_pr_title(&subjects);
    assert_eq!(title, "audit-only: 1 proposal(s) from security_bug");
}

/// Multiple audit commits aggregate counts AND list types in
/// first-seen order.
#[test]
fn build_audit_only_pr_title_aggregates_multiple_audits() {
    let subjects = vec![
        "audit: security_bug proposals (2 change(s))".to_string(),
        "audit: missing_tests proposals (3 change(s))".to_string(),
    ];
    let title = build_audit_only_pr_title(&subjects);
    assert_eq!(
        title,
        "audit-only: 5 proposal(s) from security_bug, missing_tests"
    );
}

/// a01: the planning-lanes `unit(s)` commit subject is recognized as an
/// audit-produced commit — counted in the title summary AND listed in the
/// audit-only body — exactly like the legacy `change(s)` form. (The
/// bug/gap audits now emit `(N unit(s))` because a finding can land in
/// either planning lane.)
#[test]
fn build_audit_only_pr_title_recognizes_unit_count_form() {
    let subjects = vec![
        "audit: security_bug proposals (1 unit(s))".to_string(),
        "audit: missing_tests proposals (2 change(s))".to_string(),
    ];
    // Both forms count toward the audit-only total (1 + 2 = 3) AND both are
    // bucketed as audit (not "other"), so the title takes the audit-only
    // shape rather than the mixed "across categories" fallback.
    let title = build_audit_only_pr_title(&subjects);
    assert_eq!(
        title,
        "audit-only: 3 proposal(s) from security_bug, missing_tests"
    );
    let body = build_audit_only_pr_body(&subjects);
    assert!(
        body.contains("- audit: security_bug proposals (1 unit(s))"),
        "the unit(s) subject must be listed verbatim in the audit-only body: {body}"
    );
}

/// Body explicitly states this is an audit-only PR, lists every
/// agent-branch commit subject, AND notes that the produced
/// directories will be picked up by the next iteration.
#[test]
fn build_audit_only_pr_body_lists_subjects_and_next_iter_note() {
    let subjects = vec![
        "audit: security_bug proposals (1 change(s))".to_string(),
        "audit: missing_tests proposals (2 change(s))".to_string(),
    ];
    let body = build_audit_only_pr_body(&subjects);
    assert!(
        body.contains("audit-produced proposals only"),
        "body must mark itself as audit-only: {body}"
    );
    assert!(
        body.contains("- audit: security_bug proposals (1 change(s))"),
        "body must list first subject: {body}"
    );
    assert!(
        body.contains("- audit: missing_tests proposals (2 change(s))"),
        "body must list second subject: {body}"
    );
    assert!(
        body.contains("next polling iteration will pick"),
        "body must explain next-iteration pickup: {body}"
    );
    // a49: default (no model) audit-only body carries no attribution.
    assert!(
        !body.contains("*Auditor"),
        "un-attributed audit-only body must have no attribution line: {body}"
    );
}

/// a49: when an audit IS configured with a daemon-known model, the
/// audit-produced PR section carries the
/// `*Auditor (<type>): <provider>/<model>*` attribution line.
#[test]
fn build_audit_only_pr_body_carries_attribution_when_provided() {
    let subjects = vec![
        "audit: security_bug proposals (1 change(s))".to_string(),
        "audit: missing_tests proposals (2 change(s))".to_string(),
    ];
    let attribution = crate::attribution::audit_attribution_line(
        "security_bug_audit",
        "anthropic/claude-opus-4-8",
    );
    let body = build_audit_only_pr_body_with_attribution(&subjects, Some(&attribution));
    assert!(
        body.contains("*Auditor (security_bug_audit): anthropic/claude-opus-4-8*"),
        "audit-produced PR section must carry the attribution line: {body}"
    );
    // The line lands inside the audit-produced-proposals section.
    let section_idx = body
        .find("## Audit-produced proposals")
        .expect("audit section present");
    let attr_idx = body
        .find("*Auditor (security_bug_audit):")
        .expect("attribution present");
    assert!(
        attr_idx > section_idx,
        "attribution follows the section header"
    );
}

#[test]
fn categorize_commit_subjects_buckets_canonical_shapes() {
    let subjects = vec![
        "audit: security_bug proposals (2 change(s))".to_string(),
        "iteration 2 of a35-foo: refactor scope-overflow".to_string(),
        "archive: a30-bar: implementation already in base".to_string(),
        "a31-baz: do the thing".to_string(),
        "Merge pull request #99 from a-branch".to_string(),
    ];
    let cats = categorize_commit_subjects(&subjects);
    assert_eq!(
        cats.audit,
        vec!["audit: security_bug proposals (2 change(s))".to_string()]
    );
    assert_eq!(
        cats.iteration_wip,
        vec!["iteration 2 of a35-foo: refactor scope-overflow".to_string()]
    );
    assert_eq!(
        cats.implementer,
        vec![
            "archive: a30-bar: implementation already in base".to_string(),
            "a31-baz: do the thing".to_string(),
        ]
    );
    assert_eq!(
        cats.other,
        vec!["Merge pull request #99 from a-branch".to_string()]
    );
}

/// All commits are audit → title is the canonical `audit-only:`
/// shape AND body has the "Audit-produced proposals" section only
/// (no other sections present, AND the "audit-produced proposals
/// only" framing IS included).
#[test]
fn audit_only_renderer_three_audit_zero_others() {
    let subjects = vec![
        "audit: security_bug proposals (1 change(s))".to_string(),
        "audit: missing_tests proposals (2 change(s))".to_string(),
    ];
    let title = build_audit_only_pr_title(&subjects);
    assert_eq!(
        title,
        "audit-only: 3 proposal(s) from security_bug, missing_tests"
    );
    let body = build_audit_only_pr_body(&subjects);
    assert!(
        body.contains("audit-produced proposals only"),
        "pure-audit body must keep canonical framing: {body}"
    );
    assert!(body.contains("## Audit-produced proposals"));
    assert!(!body.contains("## Iteration WIP"));
    assert!(!body.contains("## Implementer-archived changes"));
    assert!(!body.contains("## Other commits"));
}

/// Mixed: audit + iteration WIP. Title uses generic mixed shape
/// (NOT `audit-only:`), body has both sections.
#[test]
fn audit_only_renderer_mixed_audit_and_iteration_wip() {
    let subjects = vec![
        "audit: security_bug proposals (1 change(s))".to_string(),
        "audit: missing_tests proposals (1 change(s))".to_string(),
        "iteration 2 of a35-foo: bar".to_string(),
    ];
    let title = build_audit_only_pr_title(&subjects);
    assert!(
        // Behavioral: the mixed-content title reports the derived commit
        // count (3 subjects); assert that, not the hand-authored shape.
        title.contains("3 commit(s)"),
        "mixed-content title must report the commit count: {title}"
    );
    assert!(title.contains("audit"));
    assert!(title.contains("iteration WIP"));
    let body = build_audit_only_pr_body(&subjects);
    assert!(
        !body.contains("audit-produced proposals only"),
        "mixed body must NOT carry the pure-audit framing: {body}"
    );
    assert!(body.contains("## Audit-produced proposals"));
    assert!(body.contains("## Iteration WIP"));
    assert!(body.contains("- audit: security_bug proposals (1 change(s))"));
    assert!(body.contains("- iteration 2 of a35-foo: bar"));
}

/// Defensive: zero audit commits AND one iteration WIP (this
/// combination shouldn't reach the renderer in production —
/// a38's suppression rule blocks the PR. But the renderer must
/// still produce a sensible body if invoked directly via test).
#[test]
fn audit_only_renderer_zero_audit_iteration_wip_only() {
    let subjects = vec!["iteration 2 of a35-foo: scope-overflow".to_string()];
    let title = build_audit_only_pr_title(&subjects);
    assert!(
        !title.starts_with("audit-only: "),
        "no-audit title must NOT use audit-only shape: {title}"
    );
    let body = build_audit_only_pr_body(&subjects);
    assert!(
        !body.contains("audit-produced proposals only"),
        "no-audit body must NOT claim audit-produced proposals: {body}"
    );
    assert!(
        !body.contains("## Audit-produced proposals"),
        "audit-produced section must be absent when no audit commits: {body}"
    );
    assert!(
        body.contains("## Iteration WIP"),
        "iteration WIP section must be present: {body}"
    );
    assert!(body.contains("- iteration 2 of a35-foo: scope-overflow"));
}

/// No subjects readable → fallback title + body that don't claim
/// audit-produced framing.
#[test]
fn audit_only_renderer_empty_subjects() {
    let subjects: Vec<String> = Vec::new();
    let title = build_audit_only_pr_title(&subjects);
    assert_eq!(
        title,
        "audit-only: agent-branch commits without implementer changes"
    );
    let body = build_audit_only_pr_body(&subjects);
    assert!(!body.contains("audit-produced proposals only"));
}

/// Regression-prevention end-to-end test for the audit-only PR
/// flow. Fixture: workspace with no pending changes + a mock audit
/// that writes a proposal directory AND commits it on the agent
/// branch. Expected behaviour: the iteration's push reaches the
/// fixture remote AND the PR-creation HTTP call is invoked with the
/// audit-only title + body. Against the pre-fix code (early-return
/// on `processed.is_empty()`), the push step is unreachable and
/// the mockito mock's `.expect(1)` assertion fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_only_iteration_pushes_and_opens_pr() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    // Rename workspace so its basename is unique vs other tests that
    // share `fixture_workspace_with_remote`'s default name. The
    // busy-marker keys off workspace basename only.
    let ws = {
        let renamed = ws.parent().unwrap().join("workspace-audit-only-pr-test");
        std::fs::rename(&ws, &renamed).unwrap();
        renamed
    };
    // No pending changes at iteration start. The fixture audit
    // creates one openspec/changes/secure-test-1 directory and
    // commits it on the agent branch.

    let log = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let probe = OrderRecordingAudit {
        audit_type: "security_bug",
        log: log.clone(),
        creates_changes: vec!["secure-test-1".to_string()],
        write_policy: crate::audits::WritePolicy::OpenSpecOnly,
    };
    let registry = crate::audits::AuditRegistry::with_audits(vec![
        Arc::new(probe) as Arc<dyn crate::audits::Audit>
    ]);
    let queued = std::sync::Mutex::new(vec![crate::polling_loop::QueuedAudit { audit_type: "security_bug".to_string(), origin: None }]);

    // Serialize: tests sharing the github-api-base test hook must not
    // race on the process-wide static.
    let _hook_guard = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    // The PR-existence pre-check queries `/pulls` and must return
    // an empty list so the iteration proceeds past the open-PR
    // short-circuit.
    let _list_mock = server
        .mock(
            "GET",
            mockito::Matcher::Regex("^/repos/owner/fixture/pulls".to_string()),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create_async()
        .await;
    // PR-creation: assert head + base + title + body shape.
    let pr_mock = server
        .mock("POST", "/repos/owner/fixture/pulls")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::PartialJsonString(r#"{"head":"agent-q","base":"main"}"#.to_string()),
            mockito::Matcher::Regex("audit-only:".to_string()),
            mockito::Matcher::Regex("audit-only: 1 proposal\\(s\\) from security_bug".to_string()),
            mockito::Matcher::Regex(
                "audit: security_bug proposals \\(1 change\\(s\\)\\)".to_string(),
            ),
        ]))
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body(r#"{"html_url":"https://github.com/owner/fixture/pull/42","number":42}"#)
        .expect(1)
        .create_async()
        .await;

    test_hooks::set_github_api_base(Some(server.url()));

    // Inline token so credential resolution succeeds.
    let github_cfg = GithubConfig {
        token_env: "X".into(),
        token: Some(crate::config::SecretSource::Inline {
            value: "inline-test-token".into(),
        }),
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };

    let executor = AlwaysFailingExecutor; // unused: no pending changes

    let stuck_secs = 2400u64;
    let result = execute_one_pass(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &executor,
        &github_cfg,
        None,
        None,
        stuck_secs,
        u32::MAX,
        u32::MAX,
        0,  // revision_cap: disabled in tests
        Some(10), // human_revise_cap: irrelevant (dispatcher disabled)
        &registry,
        None,
        &std::collections::HashMap::new(),
        &queued,
    )
    .await;

    // Clear the test hook BEFORE asserting so a panic in an assertion
    // does not leave the override installed for the next test that
    // happens to acquire the lock.
    test_hooks::set_github_api_base(None);

    result.expect("audit-only iteration must succeed end-to-end");

    // The audit must have run.
    let entries = log.lock().unwrap().clone();
    assert!(
        entries.iter().any(|e| e == "audit:security_bug"),
        "audit must have run; log was: {entries:?}"
    );

    // The PR-creation HTTP call MUST have been invoked: this is the
    // regression assertion. Against the pre-fix code (early-return
    // on `processed.is_empty()`), the iteration returns before the
    // push step AND before this PR call. The mockito `.expect(1)`
    // assertion then fails.
    pr_mock.assert_async().await;

    // Push reached the fixture remote: the audit's commit must be on
    // `origin/agent-q` AND the agent-branch ref on the remote must
    // contain the new proposal directory.
    let remote = _dir.path().join("remote");
    let remote_log = std::process::Command::new("git")
        .args(["log", "agent-q", "--format=%s"])
        .current_dir(&remote)
        .output()
        .expect("git log on remote agent-q");
    assert!(
        remote_log.status.success(),
        "agent-q must exist on the fixture remote after push"
    );
    let subjects = String::from_utf8_lossy(&remote_log.stdout).to_string();
    assert!(
        subjects.contains("audit: security_bug proposals (1 change(s))"),
        "audit's commit subject must be present on remote agent-q; got: {subjects}"
    );
}

/// Task 2.3: with one `.iteration-pending.json` marker present AND
/// an audit that produces a commit on agent-q, the audit-only-PR
/// path is suppressed: `git::push_force_with_lease` is NOT invoked,
/// `github::create_pull_request` is NOT invoked, AND the iteration
/// returns Ok(()) cleanly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_only_pr_suppressed_when_iteration_pending_marker_present() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    // Unique basename so list_pending_changes' state-dir lookup
    // does not collide with other tests' markers.
    let ws = {
        let renamed = ws.parent().unwrap().join("workspace-a38-suppression-test");
        std::fs::rename(&ws, &renamed).unwrap();
        renamed
    };
    let basename = ws.file_name().and_then(|s| s.to_str()).unwrap().to_string();

    // Plant the iteration-pending marker BEFORE the iteration runs
    // — this is the regression-shape: a prior iteration left the
    // marker on disk, the current iteration sees iteration-pending
    // state at the post-commit-count gate AND must suppress.
    crate::iteration_pending::write_marker(
        &paths,
        &basename,
        "a35-thread-daemon-paths-globals-removal",
        &crate::iteration_pending::IterationPendingMarker {
            completed_tasks: vec!["1".into()],
            remaining_tasks: vec!["2".into()],
            reason: "prior".into(),
            iteration_number: 2,
        },
    )
    .unwrap();

    // Audit produces one commit on agent-q so commit_count > 0 at
    // the gate (otherwise the iteration short-circuits before the
    // suppression check ever runs).
    let log = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let probe = OrderRecordingAudit {
        audit_type: "security_bug",
        log: log.clone(),
        creates_changes: vec!["secure-test-2".to_string()],
        write_policy: crate::audits::WritePolicy::OpenSpecOnly,
    };
    let registry = crate::audits::AuditRegistry::with_audits(vec![
        Arc::new(probe) as Arc<dyn crate::audits::Audit>
    ]);
    let queued = std::sync::Mutex::new(vec![crate::polling_loop::QueuedAudit { audit_type: "security_bug".to_string(), origin: None }]);

    // Mockito: GET /pulls is the iteration's open-PR pre-check
    // (runs BEFORE the audit + commit-count gate, must return []
    // so the iteration proceeds far enough to reach the
    // suppression rule). POST /pulls is the PR-creation call —
    // assert .expect(0) since suppression must block it.
    let _hook_guard = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    let _list_mock = server
        .mock(
            "GET",
            mockito::Matcher::Regex("^/repos/owner/fixture/pulls".to_string()),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create_async()
        .await;
    let pr_mock = server
        .mock("POST", "/repos/owner/fixture/pulls")
        .with_status(201)
        .with_body(r#"{"html_url":"x","number":1}"#)
        .expect(0)
        .create_async()
        .await;
    test_hooks::set_github_api_base(Some(server.url()));

    let github_cfg = GithubConfig {
        token_env: "X".into(),
        token: Some(crate::config::SecretSource::Inline {
            value: "inline-test-token".into(),
        }),
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };

    let executor = AlwaysFailingExecutor; // unused: no pending changes
    let stuck_secs = 2400u64;
    let result = execute_one_pass(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &executor,
        &github_cfg,
        None,
        None,
        stuck_secs,
        u32::MAX,
        u32::MAX,
        0,
        Some(10), // human_revise_cap: irrelevant (dispatcher disabled)
        &registry,
        None,
        &std::collections::HashMap::new(),
        &queued,
    )
    .await;
    test_hooks::set_github_api_base(None);
    result.expect("suppressed iteration must return Ok(())");

    // Audit ran (so commit_count > 0 at the gate).
    let entries = log.lock().unwrap().clone();
    assert!(
        entries.iter().any(|e| e == "audit:security_bug"),
        "audit must have run; log was: {entries:?}"
    );

    // Audit's commit IS present locally on agent-q (the audit
    // committed during its run; suppression only skips push + PR).
    let local_log = std::process::Command::new("git")
        .args(["log", "agent-q", "--format=%s"])
        .current_dir(&ws)
        .output()
        .unwrap();
    let local_subjects = String::from_utf8_lossy(&local_log.stdout).to_string();
    assert!(
        local_subjects.contains("audit: security_bug proposals (1 change(s))"),
        "audit's commit must be present on LOCAL agent-q; got: {local_subjects}"
    );

    // PR-creation HTTP call was NOT invoked — the POST mock's
    // .expect(0) fires if a regression of the suppression rule
    // tries to open a PR despite the marker.
    pr_mock.assert_async().await;

    // The marker is still present (we never wrote a Completed or
    // SpecNeedsRevision outcome for it).
    assert!(
        crate::iteration_pending::marker_exists(
            &paths,
            &basename,
            "a35-thread-daemon-paths-globals-removal"
        ),
        "iteration-pending marker must persist across the suppressed iteration"
    );
}
