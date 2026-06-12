use super::*;

#[test]
fn synthesize_per_change_aggregates_verdict_worst() {
    use crate::code_reviewer::PerChangeReview;
    let per_change = vec![
        PerChangeReview {
            change_slug: "a".into(),
            report: ReviewReport {
                verdict: ReviewVerdict::Pass,
                markdown: "ok".into(),
                concerns: Vec::new(),
                per_change_sections: Vec::new(),
                attribution: None,
            },
        },
        PerChangeReview {
            change_slug: "b".into(),
            report: ReviewReport {
                verdict: ReviewVerdict::Concerns,
                markdown: "minor".into(),
                concerns: Vec::new(),
                per_change_sections: Vec::new(),
                attribution: None,
            },
        },
        PerChangeReview {
            change_slug: "c".into(),
            report: ReviewReport {
                verdict: ReviewVerdict::Block,
                markdown: "bad".into(),
                concerns: Vec::new(),
                per_change_sections: Vec::new(),
                attribution: None,
            },
        },
    ];
    let synth = crate::code_reviewer::synthesize_per_change_report(per_change);
    // Worst verdict wins (Block > Concerns > Pass).
    assert_eq!(synth.verdict, ReviewVerdict::Block);
    // Each section preserves the per-change verdict in its body.
    assert_eq!(synth.per_change_sections.len(), 3);
    assert!(
        synth.per_change_sections[0]
            .markdown
            .starts_with("VERDICT: Pass")
    );
    assert!(
        synth.per_change_sections[1]
            .markdown
            .starts_with("VERDICT: Concerns")
    );
    assert!(
        synth.per_change_sections[2]
            .markdown
            .starts_with("VERDICT: Block")
    );
}

#[test]
fn synthesize_per_change_stamps_change_slug_on_concerns() {
    use crate::code_reviewer::PerChangeReview;
    let mut c1 = revisable_concern("c1", "fix");
    c1.change_slug = None; // simulate freshly-parsed (untagged)
    let mut c2 = revisable_concern("c2", "fix");
    c2.change_slug = None;
    let per_change = vec![
        PerChangeReview {
            change_slug: "alpha".into(),
            report: ReviewReport {
                verdict: ReviewVerdict::Block,
                markdown: String::new(),
                concerns: vec![c1.clone()],
                per_change_sections: Vec::new(),
                attribution: None,
            },
        },
        PerChangeReview {
            change_slug: "beta".into(),
            report: ReviewReport {
                verdict: ReviewVerdict::Block,
                markdown: String::new(),
                concerns: vec![c2.clone()],
                per_change_sections: Vec::new(),
                attribution: None,
            },
        },
    ];
    let synth = crate::code_reviewer::synthesize_per_change_report(per_change);
    assert_eq!(synth.concerns.len(), 2);
    assert_eq!(synth.concerns[0].change_slug.as_deref(), Some("alpha"));
    assert_eq!(synth.concerns[1].change_slug.as_deref(), Some("beta"));
}

// a005 task 3.1: a review with N≥2 actionable concerns collects ALL of
// them (the whole set rides one aggregated run / one cap slot — no
// per-concern cap drop), so the dispatcher issues exactly one run.
#[test]
fn collect_reviewer_revisions_returns_all_actionable() {
    let r = make_report(
        ReviewVerdict::Block,
        vec![
            revisable_concern("a", "fix a"),
            revisable_concern("b", "fix b"),
            revisable_concern("c", "fix c"),
        ],
    );
    let taken = collect_reviewer_revisions(&r);
    assert_eq!(taken.len(), 3, "all actionable concerns ride one batch");
    let summaries: Vec<&str> = taken.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(summaries, vec!["a", "b", "c"]);
}

// a005: commentary concerns (no `should_request_revision`) and
// empty/whitespace `actionable_request`s are filtered out.
#[test]
fn collect_reviewer_revisions_filters_commentary_and_empty() {
    let r = make_report(
        ReviewVerdict::Block,
        vec![
            commentary_concern("style nit"),
            ReviewConcern {
                summary: "missing-body".into(),
                actionable_request: Some("   ".into()),
                should_request_revision: true,
                change_slug: None,
                ..Default::default()
            },
            revisable_concern("ok", "fix this"),
        ],
    );
    let taken = collect_reviewer_revisions(&r);
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0].summary, "ok");
}

// a005: no actionable concerns => empty (the WARN-logged misconfig path).
#[test]
fn collect_reviewer_revisions_no_actionable_returns_empty() {
    let r = make_report(
        ReviewVerdict::Block,
        vec![
            commentary_concern("style nit"),
            commentary_concern("preference"),
        ],
    );
    assert!(collect_reviewer_revisions(&r).is_empty());
}

// a005 revision: concerns were surfaced but none are revisable → the
// template-misconfiguration WARN fires (this is the genuine "flag flipped
// but no actionable fields" case).
#[test]
#[tracing_test::traced_test]
fn collect_reviewer_revisions_warns_when_concerns_present_but_none_revisable() {
    let r = make_report(
        ReviewVerdict::Block,
        vec![
            commentary_concern("style nit"),
            commentary_concern("preference"),
        ],
    );
    assert!(collect_reviewer_revisions(&r).is_empty());
    assert!(
        logs_contain("verify the reviewer prompt template emits these fields"),
        "a report with concerns but none revisable must WARN about the template"
    );
}

// a005 revision: a completely clean review (zero concerns) is NOT a
// template misconfiguration, so it must NOT log the misleading WARN —
// otherwise every clean PR under `auto_revise: actionable` spams the log.
#[test]
#[tracing_test::traced_test]
fn collect_reviewer_revisions_clean_review_does_not_warn() {
    let r = make_report(ReviewVerdict::Pass, vec![]);
    assert!(collect_reviewer_revisions(&r).is_empty());
    assert!(
        !logs_contain("verify the reviewer prompt template emits these fields"),
        "a clean review with zero concerns must NOT log the template WARN"
    );
}

// a005 task 3.3: default `block` does NOT fire on a Concerns verdict but
// DOES on a Block verdict — and on Block carries every concern.
#[test]
fn reviewer_revisions_block_default_fires_only_on_block() {
    let reviewer = reviewer_with_auto_revise(crate::config::AutoRevise::Block);
    let concerns = vec![
        revisable_concern("a", "fix a"),
        revisable_concern("b", "fix b"),
    ];
    // Concerns verdict → no dispatch.
    let r_concerns = make_report(ReviewVerdict::Concerns, concerns.clone());
    let taken = reviewer_revisions_for_review(&reviewer, &r_concerns, false, 5);
    assert!(taken.is_empty(), "block default must not fire on Concerns");
    // Block verdict → dispatch, carrying all concerns.
    let r_block = make_report(ReviewVerdict::Block, concerns);
    let taken = reviewer_revisions_for_review(&reviewer, &r_block, true, 5);
    assert_eq!(
        taken.len(),
        2,
        "block fires on Block, carrying all concerns"
    );
}

// a005 task 3.4: `actionable` fires on a Concerns verdict (restores the
// pre-a005 fire-regardless-of-verdict behavior).
#[test]
fn reviewer_revisions_actionable_fires_on_concerns() {
    let reviewer = reviewer_with_auto_revise(crate::config::AutoRevise::Actionable);
    let r = make_report(
        ReviewVerdict::Concerns,
        vec![revisable_concern("a", "fix a")],
    );
    let taken = reviewer_revisions_for_review(&reviewer, &r, false, 5);
    assert_eq!(taken.len(), 1, "actionable fires regardless of verdict");
}

// a005 task 3.4: `off` never fires, even on a Block verdict.
#[test]
fn reviewer_revisions_off_never_fires() {
    let reviewer = reviewer_with_auto_revise(crate::config::AutoRevise::Off);
    let r = make_report(ReviewVerdict::Block, vec![revisable_concern("a", "fix a")]);
    let taken = reviewer_revisions_for_review(&reviewer, &r, true, 5);
    assert!(taken.is_empty(), "off must never fire");
}

// a005: `max_auto_revisions_per_pr == 0` disables reviewer-initiated
// revisions entirely, even when the tri-state would otherwise fire.
#[test]
fn reviewer_revisions_zero_cap_disables() {
    let reviewer = reviewer_with_auto_revise(crate::config::AutoRevise::Actionable);
    let r = make_report(ReviewVerdict::Block, vec![revisable_concern("a", "fix a")]);
    let taken = reviewer_revisions_for_review(&reviewer, &r, true, 0);
    assert!(taken.is_empty(), "revision_cap 0 disables the feature");
}

/// Regression guard: the PR-open state init must SOURCE the re-review cap
/// from the reviewer (NOT hardcode `Some(5)`). With the a47 default
/// (`reviewer.max_code_reviews_per_pr` unset → `None`), a freshly-opened
/// PR's state must carry `code_review_cap: None` (unlimited) — otherwise
/// every daemon-opened PR is silently re-capped at 5 reruns.
#[test]
fn pr_open_state_init_sources_unlimited_review_cap_from_reviewer() {
    let reviewer = reviewer_with_review_cap(None);
    let now = chrono::Utc::now();
    let state = initial_revision_state_at_pr_open(
        42,
        "agent-q".to_string(),
        now,
        5,
        Some(&reviewer),
        "deadbeef".to_string(),
    );
    assert_eq!(
        state.code_review_cap, None,
        "unset reviewer cap must yield None (unlimited), not the old hardcoded Some(5)"
    );
    assert_eq!(
        state.revision_cap, 5,
        "auto-revision cap is sourced from the passed value"
    );
    assert_eq!(state.auto_revisions_applied, 0);
    assert_eq!(state.code_reviews_applied, 0);
    assert_eq!(state.original_review_head_sha.as_deref(), Some("deadbeef"));
    assert_eq!(state.last_seen_comment_at, now);
}

/// When the operator set an opt-in re-review ceiling, the PR-open init
/// carries it through as `Some(n)`.
#[test]
fn pr_open_state_init_sources_set_review_cap_from_reviewer() {
    let reviewer = reviewer_with_review_cap(Some(3));
    let state = initial_revision_state_at_pr_open(
        7,
        "agent-q".to_string(),
        chrono::Utc::now(),
        12,
        Some(&reviewer),
        "cafe".to_string(),
    );
    assert_eq!(state.code_review_cap, Some(3));
    // The auto-revision cap reflects the configured value, not a hardcoded 5.
    assert_eq!(state.revision_cap, 12);
}

/// No reviewer configured → the re-review cap is `None` (unlimited).
#[test]
fn pr_open_state_init_no_reviewer_yields_unlimited_review_cap() {
    let state = initial_revision_state_at_pr_open(
        9,
        "agent-q".to_string(),
        chrono::Utc::now(),
        0,
        None,
        "f00d".to_string(),
    );
    assert_eq!(state.code_review_cap, None);
    assert_eq!(state.revision_cap, 0);
}

// a005: a multi-concern review posts EXACTLY ONE aggregated
// `<!-- reviewer-revision -->` comment carrying every concern as a
// numbered list — not one comment per concern.
#[tokio::test]
async fn post_reviewer_revision_comments_posts_one_aggregated_comment() {
    let mut server = mockito::Server::new_async().await;
    let _user = server
        .mock("GET", "/user")
        .with_status(200)
        .with_body(r#"{"login":"my-bot"}"#)
        .create_async()
        .await;
    // Exactly ONE comment POST, whose body carries the marker, the
    // mention, AND both requests as a numbered list.
    let mock = server
        .mock("POST", "/repos/owner/repo/issues/77/comments")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::Regex("<!-- reviewer-revision -->".to_string()),
            mockito::Matcher::Regex("@my-bot revise".to_string()),
            mockito::Matcher::Regex(r"1\. fix find_user".to_string()),
            mockito::Matcher::Regex(r"2\. restore the audit hook".to_string()),
        ]))
        .with_status(201)
        .with_body(r#"{"id":1}"#)
        .expect(1)
        .create_async()
        .await;

    let concerns = vec![
        revisable_concern("find_user error context", "fix find_user"),
        revisable_concern("audit hook removed", "restore the audit hook"),
    ];
    post_reviewer_revision_comments(&server.url(), "owner", "repo", 77, &concerns, "test-token")
        .await;
    // `.expect(1)` + assert ⇒ exactly one aggregated POST, not N.
    mock.assert_async().await;
}

// a005: a single-concern review keeps the historical one-line shape
// (`@<bot> revise <request>`, no numbered list), and a POST failure is
// handled gracefully (the helper returns normally).
#[tokio::test]
async fn post_reviewer_revision_comments_single_concern_one_line_shape() {
    let mut server = mockito::Server::new_async().await;
    let _user = server
        .mock("GET", "/user")
        .with_status(200)
        .with_body(r#"{"login":"my-bot"}"#)
        .create_async()
        .await;
    let mock = server
        .mock("POST", "/repos/owner/repo/issues/88/comments")
        .match_body(mockito::Matcher::PartialJsonString(
            r#"{"body":"<!-- reviewer-revision -->\n@my-bot revise fix the thing"}"#.to_string(),
        ))
        // A 500 still exercises the graceful-failure path.
        .with_status(500)
        .with_body(r#"{"error":"transient"}"#)
        .expect(1)
        .create_async()
        .await;

    let concerns = vec![revisable_concern("a", "fix the thing")];
    post_reviewer_revision_comments(&server.url(), "owner", "repo", 88, &concerns, "test-token")
        .await;
    mock.assert_async().await;
}

/// 2.4 (a12): with 2 pending changes AND 1 eligible audit, pending
/// changes are processed FIRST, then the audit runs. Both phases
/// commit on agent-q so a single iteration's PR carries both.
///
/// The audit is made eligible via the `queued_audit_types` set
/// (bypasses cadence) — equivalent for ordering purposes to a
/// cadence-driven eligible audit, and avoids constructing a full
/// `AuditsConfig` just to set a cadence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_changes_process_before_audits() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "change-one", "first pending");
    add_committed_change(&ws, "change-two", "second pending");

    let log = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let executor = OrderRecordingExecutor { log: log.clone() };
    let probe = OrderRecordingAudit {
        audit_type: "ordering_probe_a",
        log: log.clone(),
        creates_changes: Vec::new(),
        write_policy: crate::audits::WritePolicy::None,
    };
    let registry = crate::audits::AuditRegistry::with_audits(vec![
        Arc::new(probe) as Arc<dyn crate::audits::Audit>
    ]);
    let queued = std::sync::Mutex::new(vec![crate::polling_loop::QueuedAudit { audit_type: "ordering_probe_a".to_string(), origin: None }]);

    let test_github = GithubConfig {
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
        &test_github,
        &executor,
        None,
        u32::MAX,
        u32::MAX,
        &registry,
        None,
        &std::collections::HashMap::new(),
        &queued,
    )
    .await
    .expect("pass succeeds");

    assert_eq!(processed.len(), 2, "both pending changes must be processed");

    let entries = log.lock().unwrap().clone();
    // Both executor entries must precede the audit entry.
    let audit_idx = entries
        .iter()
        .position(|e| e == "audit:ordering_probe_a")
        .expect("audit must have run");
    let exec_indices: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| e.starts_with("executor:").then_some(i))
        .collect();
    assert_eq!(exec_indices.len(), 2, "executor ran for both changes");
    for i in &exec_indices {
        assert!(
            *i < audit_idx,
            "executor invocations must precede the audit invocation; log was: {entries:?}"
        );
    }

    // Both kinds of commits must be present on agent-q (change-one,
    // change-two artifacts + their archive moves; archive landing
    // means the queue is empty).
    assert_eq!(
        queue::list_pending(&paths, &ws).unwrap(),
        Vec::<String>::new(),
        "both pending changes must be archived this iteration"
    );
}

/// 2.4 (a12): with 0 pending changes AND 1 eligible audit that
/// creates 2 new proposals, the audit's creation commit ships in
/// THIS iteration's PR but the 2 generated changes wait for the
/// NEXT iteration's `list_pending` (they appear as pending on disk
/// after the iteration but the executor was never invoked on them).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_generated_changes_wait_one_iteration_for_implementer() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    // No pending changes at iteration start. The audit will create
    // two openspec/changes/<name>/ directories below.

    let log = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let executor = OrderRecordingExecutor { log: log.clone() };
    let probe = OrderRecordingAudit {
        audit_type: "ordering_probe_b",
        log: log.clone(),
        creates_changes: vec![
            "tests-generated-one".to_string(),
            "tests-generated-two".to_string(),
        ],
        write_policy: crate::audits::WritePolicy::OpenSpecOnly,
    };
    let registry = crate::audits::AuditRegistry::with_audits(vec![
        Arc::new(probe) as Arc<dyn crate::audits::Audit>
    ]);
    let queued = std::sync::Mutex::new(vec![crate::polling_loop::QueuedAudit { audit_type: "ordering_probe_b".to_string(), origin: None }]);

    let pre_main = crate::git::rev_parse(&ws, "main").unwrap();
    let test_github = GithubConfig {
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
        &test_github,
        &executor,
        None,
        u32::MAX,
        u32::MAX,
        &registry,
        None,
        &std::collections::HashMap::new(),
        &queued,
    )
    .await
    .expect("pass succeeds");

    assert!(
        processed.is_empty(),
        "no pending changes existed at iteration start; the implementer must not have run"
    );

    let entries = log.lock().unwrap().clone();
    assert_eq!(
        entries,
        vec!["audit:ordering_probe_b".to_string()],
        "executor must not have been invoked on the audit's generated changes this iteration"
    );

    // Audit's creation commit must be on agent-q (the head moved
    // past pre_main on the agent branch).
    let agent_sha = crate::git::rev_parse(&ws, "agent-q").unwrap();
    assert_ne!(
        agent_sha, pre_main,
        "audit's commit must land on agent-q so it ships in this iteration's PR"
    );

    // The two new proposals are on disk and now show up in
    // list_pending — so the NEXT iteration's queue walk picks them
    // up.
    let mut pending = queue::list_pending(&paths, &ws).unwrap();
    pending.sort();
    assert_eq!(
        pending,
        vec![
            "tests-generated-one".to_string(),
            "tests-generated-two".to_string()
        ],
        "audit-generated proposals must be pending for the next iteration"
    );
}
