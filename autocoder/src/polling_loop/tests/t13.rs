use super::*;

/// commit-trailing-archive: after a multi-change pass, the working
/// tree MUST be clean (one commit per change, each containing its
/// own archive move).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_change_pass_clean_after_each() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    for n in 1..=3 {
        add_committed_change(&ws, &format!("ch{n:02}"), &format!("fixture {n}"));
    }
    let executor = PerChangeArtifactExecutor;
    let github_cfg = GithubConfig {
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
        &github_cfg,
        &executor,
        None,
        u32::MAX,
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::collections::HashSet::new(),
    )
    .await
    .expect("pass succeeds");
    assert_eq!(processed.len(), 3, "all three archived");

    // Working tree must be clean.
    let porcelain = crate::git::status_porcelain(&ws).unwrap();
    assert!(
        porcelain.trim().is_empty(),
        "working tree must be clean after multi-change pass; got:\n{porcelain}"
    );

    // Exactly 3 new commits on agent-q ahead of main.
    let out = std::process::Command::new("git")
        .args(["rev-list", "--count", "main..HEAD"])
        .current_dir(&ws)
        .output()
        .unwrap();
    let count: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
    assert_eq!(count, 3, "three commits ahead of main, one per change");
}

/// 1000 draws with `startup_jitter_max_secs = 30` MUST all be in
/// `[0, 30]`, and the sample MUST contain both endpoints. With a
/// uniform 0..=30 draw and 1000 samples the probability of missing
/// either endpoint is `(30/31)^1000 ≈ 10^-14`.
#[test]
fn startup_jitter_in_range() {
    let mut saw_zero = false;
    let mut saw_thirty = false;
    for _ in 0..1000 {
        let v = pick_startup_jitter_secs(30);
        assert!(v <= 30, "draw {v} must be in [0, 30]");
        if v == 0 {
            saw_zero = true;
        }
        if v == 30 {
            saw_thirty = true;
        }
    }
    assert!(saw_zero, "1000 draws should produce at least one 0");
    assert!(saw_thirty, "1000 draws should produce at least one 30");
}

/// A `0` ceiling MUST short-circuit to `0` without consulting the
/// RNG (and definitely without panicking on a degenerate range).
#[test]
fn startup_jitter_zero_returns_zero() {
    for _ in 0..100 {
        assert_eq!(pick_startup_jitter_secs(0), 0);
    }
}

/// For `base = 300, pct = 10` the helper draws in `[270, 330]`
/// (300 ± 30). 1000 samples MUST stay inside the band AND the mean
/// MUST be within ±5 of 300 — a uniform distribution centred on 300
/// will, with overwhelming probability, satisfy this.
#[test]
fn jittered_sleep_duration_within_band() {
    let mut sum: u64 = 0;
    for _ in 0..1000 {
        let d = jittered_sleep_duration(300, 10);
        let s = d.as_secs();
        assert!((270..=330).contains(&s), "draw {s} must be in [270, 330]");
        sum += s;
    }
    let mean = sum as f64 / 1000.0;
    assert!(
        (mean - 300.0).abs() <= 5.0,
        "mean {mean} must be within ±5 of 300"
    );
}

/// `pct = 0` MUST produce exactly `base_secs` every time — the
/// arithmetic short-circuit lets operators opt out of jitter for
/// deterministic test timing.
#[test]
fn jittered_sleep_duration_zero_pct_is_exact() {
    for _ in 0..100 {
        let d = jittered_sleep_duration(300, 0);
        assert_eq!(d, Duration::from_secs(300));
    }
}

/// `base = 10, pct = 100` means the negative offset can be up to
/// `-10` (i.e. equal to the entire interval). Result MUST stay in
/// `[0, 20]` and MUST NOT panic on the underflow boundary.
#[test]
fn jittered_sleep_duration_no_underflow_when_pct_is_100() {
    for _ in 0..1000 {
        let d = jittered_sleep_duration(10, 100);
        let s = d.as_secs();
        assert!(s <= 20, "draw {s} must be in [0, 20]");
    }
    // The boundary case: ensure the helper doesn't panic with the
    // most-aggressive percentage on the smallest interval.
    let _ = jittered_sleep_duration(1, 100);
    let _ = jittered_sleep_duration(0, 100);
}

/// Cancellation while the task is in its startup-jitter sleep MUST
/// be observed within 200 ms; the task MUST NOT iterate. Uses a
/// dummy executor and noisy holders since none should be touched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_exits_during_startup_jitter() {
    struct UnreachableExecutor;
    #[async_trait::async_trait]
    impl Executor for UnreachableExecutor {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            unreachable!("startup-jitter cancellation must prevent first iteration");
        }
        async fn resume(
            &self,
            _h: crate::executor::ResumeHandle,
            _a: &str,
        ) -> Result<ExecutorOutcome> {
            unreachable!()
        }
    }

    let dir = tempfile::TempDir::new().unwrap();
    let mut repo = fixture_repo(dir.path());
    // Configure a huge poll_interval so any post-jitter sleep would
    // also block — if the test passes, we must be exiting from the
    // jitter sleep, not the iter sleep.
    repo.poll_interval_sec = 86_400;
    let repo_holder = Arc::new(ArcSwap::from_pointee(repo));
    let executor: Arc<dyn Executor> = Arc::new(UnreachableExecutor);
    let github_holder: GithubHolder = Arc::new(ArcSwap::from_pointee(GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    }));
    let reviewer_holder: ReviewerHolder = Arc::new(ArcSwap::from_pointee(None));
    let chatops_holder: ChatOpsHolder = Arc::new(ArcSwap::from_pointee(None));
    let cache_holder: CacheHolder =
        Arc::new(ArcSwap::from_pointee(crate::config::CacheConfig::default()));
    let cancel = CancellationToken::new();

    let task_cancel = cancel.clone();
    let (_td_paths, paths_inner) = crate::testing::test_daemon_paths();
    let paths_for_run = std::sync::Arc::new(paths_inner);
    let handle = tokio::spawn(async move {
        run(
            paths_for_run,
            repo_holder,
            executor,
            github_holder,
            reviewer_holder,
            chatops_holder,
            cache_holder,
            1_000_000,
            u32::MAX,
            None,
            0,  // revision_cap: disabled in tests
            10, // human_revise_cap: irrelevant (dispatcher disabled)
            60, // startup_jitter_max_secs: large window
            0,  // inter_iteration_jitter_pct: irrelevant
            std::sync::Arc::new(crate::audits::AuditRegistry::default()),
            None,
            std::sync::Arc::new(std::collections::HashMap::new()),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            std::sync::Arc::new(std::sync::Mutex::new(None)),
            std::sync::Arc::new(tokio::sync::Notify::new()),
            task_cancel,
        )
        .await;
    });

    // Cancel immediately — the task should exit during the
    // startup-jitter sleep, not after a multi-second wait.
    cancel.cancel();
    let start = std::time::Instant::now();
    tokio::time::timeout(Duration::from_millis(2000), handle)
        .await
        .expect("run must exit within 2s after cancel during startup jitter")
        .expect("polling task must not panic");
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(500),
        "cancellation should be observed within 500 ms; took {elapsed:?}"
    );
}

#[test]
#[allow(non_snake_case)]
fn build_pr_title_single_change_humanizes_aNN_prefix() {
    let input = vec!["a06-refactor-portal-handlers-to-fromref".to_string()];
    assert_eq!(
        build_pr_title(&input),
        "a06: refactor portal handlers to fromref",
    );
}

#[test]
fn build_pr_title_single_change_without_prefix() {
    let input = vec!["fix-bug-in-thing".to_string()];
    assert_eq!(build_pr_title(&input), "fix bug in thing");
}

#[test]
fn build_pr_title_multi_change_uses_first_and_count() {
    let input = vec![
        "a04-foo-thing".to_string(),
        "a05-bar-thing".to_string(),
        "a06-baz-thing".to_string(),
    ];
    assert_eq!(build_pr_title(&input), "a04: foo thing (+2 more)");
}

#[test]
fn build_pr_title_caps_overlong() {
    let mut slug = String::from("a06-");
    for _ in 0..50 {
        slug.push_str("verylong-");
    }
    let input = vec![slug];
    let title = build_pr_title(&input);
    assert!(
        title.chars().count() <= 80,
        "title should be capped at 80 chars; got {} chars: {title:?}",
        title.chars().count()
    );
    assert!(
        title.ends_with('…'),
        "truncated title should end with ellipsis; got {title:?}"
    );
}

#[test]
#[allow(non_snake_case)]
fn humanize_slug_strips_aNN_prefix_into_label() {
    assert_eq!(humanize_slug("a06-x-y"), "a06: x y");
    assert_eq!(humanize_slug("b13-foo-bar"), "b13: foo bar");
    assert_eq!(humanize_slug("foo-bar"), "foo bar");
}

#[test]
fn build_pr_body_inlines_why_from_archived_proposal() {
    let tmp = tempfile::TempDir::new().unwrap();
    write_fixture_archive(
        tmp.path(),
        "2026-05-18-fix-thing",
        "## Why\n\nThing was broken because of reasons.\n\n## What Changes\n\nstuff\n",
    );
    let body = build_pr_body(tmp.path(), &["fix-thing".to_string()], false);
    assert!(body.contains("## fix-thing"), "body: {body}");
    assert!(
        body.contains("Thing was broken because of reasons."),
        "body: {body}"
    );
    assert!(
        body.contains("Changes implemented in this pass"),
        "body: {body}"
    );
}

#[test]
fn build_pr_body_falls_back_when_proposal_missing() {
    let tmp = tempfile::TempDir::new().unwrap();
    // No archive directory at all.
    let body = build_pr_body(tmp.path(), &["fix-thing".to_string()], false);
    assert!(body.contains("## fix-thing"), "body: {body}");
    assert!(
        body.contains("_(no proposal.md available)_"),
        "body: {body}"
    );
}

#[test]
fn build_pr_body_handles_multiple_changes() {
    let tmp = tempfile::TempDir::new().unwrap();
    write_fixture_archive(
        tmp.path(),
        "2026-05-18-a04-foo",
        "## Why\n\nFoo rationale.\n\n## What Changes\n\nx\n",
    );
    write_fixture_archive(
        tmp.path(),
        "2026-05-18-a05-bar",
        "## Why\n\nBar rationale.\n\n## What Changes\n\nx\n",
    );
    write_fixture_archive(
        tmp.path(),
        "2026-05-18-a06-baz",
        "## Why\n\nBaz rationale.\n\n## What Changes\n\nx\n",
    );
    let changes = vec![
        "a04-foo".to_string(),
        "a05-bar".to_string(),
        "a06-baz".to_string(),
    ];
    let body = build_pr_body(tmp.path(), &changes, false);

    // Each per-change heading appears in input order.
    let foo_pos = body.find("## a04-foo").expect("a04-foo heading");
    let bar_pos = body.find("## a05-bar").expect("a05-bar heading");
    let baz_pos = body.find("## a06-baz").expect("a06-baz heading");
    assert!(foo_pos < bar_pos && bar_pos < baz_pos);

    // Each section contains its own Why text.
    assert!(body.contains("Foo rationale."));
    assert!(body.contains("Bar rationale."));
    assert!(body.contains("Baz rationale."));
}

#[test]
fn build_pr_body_preserves_self_heal_disclaimer() {
    let tmp = tempfile::TempDir::new().unwrap();
    write_fixture_archive(
        tmp.path(),
        "2026-05-18-fix-thing",
        "## Why\n\nA reason.\n\n## What Changes\n\nx\n",
    );
    let body = build_pr_body(tmp.path(), &["fix-thing".to_string()], true);
    assert!(
        // Structural: a self-heal body opens with an italic disclaimer
        // paragraph (terminated by `_\n\n` below); assert its shape, not prose.
        body.starts_with('_'),
        "body must begin with the self-heal disclaimer paragraph; got: {body}"
    );
    let disclaimer_end = body.find("_\n\n").expect("disclaimer paragraph terminator");
    let after_disclaimer = &body[disclaimer_end..];
    assert!(
        after_disclaimer.contains("## fix-thing"),
        "per-change section must follow disclaimer; got: {body}"
    );
}

#[test]
fn build_pr_body_extracts_only_why_section() {
    let tmp = tempfile::TempDir::new().unwrap();
    write_fixture_archive(
        tmp.path(),
        "2026-05-18-fix-thing",
        "## Why\nWhy text.\n## What Changes\nDifferent text.\n## Impact\nMore text.\n",
    );
    let body = build_pr_body(tmp.path(), &["fix-thing".to_string()], false);
    assert!(body.contains("Why text."), "body: {body}");
    assert!(
        !body.contains("Different text."),
        "body must not include non-Why sections; got: {body}"
    );
    assert!(
        !body.contains("More text."),
        "body must not include non-Why sections; got: {body}"
    );
}

#[test]
#[tracing_test::traced_test]
fn read_change_why_archive_path_wins_without_warn() {
    let tmp = tempfile::TempDir::new().unwrap();
    write_fixture_archive(
        tmp.path(),
        "2026-05-18-fix-thing",
        "## Why\n\nArchive rationale.\n\n## What Changes\n\nx\n",
    );
    let why = read_change_why(tmp.path(), "fix-thing");
    assert!(
        why.as_deref()
            .map(|s| s.contains("Archive rationale."))
            .unwrap_or(false),
        "expected archive why; got: {why:?}"
    );
    assert!(
        !logs_contain("proposal read from active path"),
        "no fallback WARN expected on archive hit"
    );
}

#[test]
#[tracing_test::traced_test]
fn read_change_why_falls_back_to_active_with_warn() {
    let tmp = tempfile::TempDir::new().unwrap();
    // No archive fixture.
    write_fixture_active_proposal(
        tmp.path(),
        "fix-thing",
        "## Why\n\nActive-path rationale.\n\n## What Changes\n\nx\n",
    );
    let why = read_change_why(tmp.path(), "fix-thing");
    assert!(
        why.as_deref()
            .map(|s| s.contains("Active-path rationale."))
            .unwrap_or(false),
        "expected active-path why; got: {why:?}"
    );
    assert!(
        logs_contain("proposal read from active path"),
        "expected fallback WARN naming the change"
    );
    assert!(logs_contain("fix-thing"), "WARN must name the change slug");
}

#[test]
#[tracing_test::traced_test]
fn read_change_why_active_without_why_section_returns_none_no_warn() {
    let tmp = tempfile::TempDir::new().unwrap();
    // No archive fixture; active proposal lacks a `## Why` heading.
    write_fixture_active_proposal(
        tmp.path(),
        "fix-thing",
        "## What Changes\n\nstuff but no why\n",
    );
    let why = read_change_why(tmp.path(), "fix-thing");
    assert!(why.is_none(), "expected None; got: {why:?}");
    assert!(
        !logs_contain("proposal read from active path"),
        "WARN should not fire when fallback extracts no content"
    );
}

#[test]
#[tracing_test::traced_test]
fn read_change_why_both_paths_missing_returns_none_no_warn() {
    let tmp = tempfile::TempDir::new().unwrap();
    let why = read_change_why(tmp.path(), "fix-thing");
    assert!(why.is_none(), "expected None; got: {why:?}");
    assert!(
        !logs_contain("proposal read from active path"),
        "WARN should not fire when both paths miss"
    );
}

#[test]
#[tracing_test::traced_test]
fn read_change_why_archive_present_overrides_active_no_warn() {
    let tmp = tempfile::TempDir::new().unwrap();
    write_fixture_archive(
        tmp.path(),
        "2026-05-18-fix-thing",
        "## Why\n\nArchive rationale.\n\n## What Changes\n\nx\n",
    );
    write_fixture_active_proposal(
        tmp.path(),
        "fix-thing",
        "## Why\n\nActive rationale.\n\n## What Changes\n\nx\n",
    );
    let why = read_change_why(tmp.path(), "fix-thing");
    let text = why.expect("expected archive why");
    assert!(
        text.contains("Archive rationale."),
        "archive path must win; got: {text}"
    );
    assert!(
        !text.contains("Active rationale."),
        "active text must not leak through; got: {text}"
    );
    assert!(
        !logs_contain("proposal read from active path"),
        "no WARN expected when archive path wins"
    );
}

#[test]
fn extract_stdout_section_returns_body_between_markers() {
    let raw = "=== STDOUT (10) ===\nhello world\n=== STDERR (0) ===\nignored\n";
    assert_eq!(extract_stdout_section(raw), "hello world\n");
}

#[test]
fn extract_stdout_section_returns_empty_when_no_stdout_marker() {
    let raw = "no markers anywhere\n=== STDERR (0) ===\n";
    assert_eq!(extract_stdout_section(raw), "");
}

#[test]
fn extract_stdout_section_returns_empty_when_header_has_no_newline() {
    let raw = "=== STDOUT (10) ===";
    assert_eq!(extract_stdout_section(raw), "");
}

#[test]
fn extract_stdout_section_returns_to_eof_when_no_stderr_marker() {
    let raw = "=== STDOUT (5) ===\nbody only\n";
    assert_eq!(extract_stdout_section(raw), "body only\n");
}

#[test]
fn filter_alert_state_lines_passes_through_when_no_alert_state() {
    let porcelain = " M src/foo.rs\n?? new.txt\n";
    let out = filter_alert_state_lines(porcelain);
    // `.lines()` strips the trailing newline; `join("\n")` re-joins
    // without one, so we compare against the same shape.
    assert_eq!(out, " M src/foo.rs\n?? new.txt");
}

#[test]
fn filter_alert_state_lines_strips_only_alert_state_entry() {
    let porcelain = "?? .alert-state.json\n";
    let out = filter_alert_state_lines(porcelain);
    assert!(
        out.trim().is_empty(),
        "expected empty/whitespace-only output, got {out:?}"
    );
}

#[test]
fn filter_alert_state_lines_keeps_real_files_and_strips_alert_state() {
    let porcelain = " M src/foo.rs\n?? .alert-state.json\n M src/bar.rs\n";
    let out = filter_alert_state_lines(porcelain);
    assert!(out.contains(" M src/foo.rs"), "missing foo.rs: {out:?}");
    assert!(out.contains(" M src/bar.rs"), "missing bar.rs: {out:?}");
    assert!(
        !out.contains(".alert-state.json"),
        "alert-state line leaked: {out:?}"
    );
}

#[test]
fn filter_alert_state_lines_does_not_match_subpath_or_similar_name() {
    let porcelain = " M subdir/.alert-state.json\n?? prefix.alert-state.json\n";
    let out = filter_alert_state_lines(porcelain);
    assert!(
        out.contains("subdir/.alert-state.json"),
        "subdir variant must survive: {out:?}"
    );
    assert!(
        out.contains("prefix.alert-state.json"),
        "prefix variant must survive: {out:?}"
    );
}
