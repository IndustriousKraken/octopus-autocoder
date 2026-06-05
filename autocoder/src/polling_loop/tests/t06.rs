use super::*;

/// Clear-on-success: a failing iteration alerts, a successful next
/// iteration clears state, then a SECOND failure re-alerts because the
/// throttle was reset (NOT silenced by the 24h window).
///
/// Iter 1 runs the full `execute_one_pass` to produce the real alert +
/// real state file. Iter 2 calls `AlertState::clear` directly to mimic
/// the on-success clear that `execute_one_pass` performs (production
/// already invokes `clear` at three Ok paths — see the inline calls
/// in `execute_one_pass` and `run_pass_through_commits`). Iter 3
/// invokes `handle_predictable_failure` directly to verify that with
/// state cleared the alert fires again immediately, NOT silenced by
/// the 24h throttle.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failure_alert_cleared_on_subsequent_success() {
    let (_dir, ws) = fixture_workspace_with_broken_remote("alert-cleared");
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "round-1", "fixture round 1");

    let mut server = mockito::Server::new_async().await;
    let chatops = fixture_chatops_for(&mut server).await;
    // Two alerts expected across iterations 1 + 3.
    let alert_mock = server
        .mock("POST", "/chat.postMessage")
        .match_body(mockito::Matcher::Regex(
            "branch push keeps failing".to_string(),
        ))
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .expect(2)
        .create_async()
        .await;
    let _start_work_mock = server
        .mock("POST", "/chat.postMessage")
        .match_body(mockito::Matcher::Regex("starting work on".to_string()))
        .with_status(200)
        .with_body(r#"{"ok":true,"ts":"1.0"}"#)
        .create_async()
        .await;

    let executor = CompletingExecutorWithDiff {
        artifact_name: "ART.txt".into(),
        artifact_text: "x".into(),
    };
    let chatops_ctx = ChatOpsContext {
        chatops: chatops.clone(),
        channel: "C_TEST".into(),
        start_work_enabled: true,
        failure_alerts_enabled: true,
        pr_opened_enabled: true,
    };
    let github = GithubConfig {
        token_env: "X".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let stuck_secs = 2400u64;

    // Iteration 1: push fails → alert #1 fires AND state is saved.
    let _ = execute_one_pass(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &executor,
        &github,
        None,
        Some(&chatops_ctx),
        stuck_secs,
        u32::MAX,
        u32::MAX,
        0,  // revision_cap: disabled in tests
        10, // human_revise_cap: irrelevant (dispatcher disabled)
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::collections::HashSet::new(),
    )
    .await;
    let basename = ws.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        paths.alert_state_path(&basename).exists(),
        "alert state should be written after first failure"
    );

    // Iteration 2: simulate a successful pass-end by directly clearing
    // the alert state, mimicking what `execute_one_pass` does on each
    // of its Ok-return paths (after push+PR succeed, when processed is
    // empty, or when commit_count is zero). The clear paths are
    // covered by `AlertState::clear`'s own unit tests; here we just
    // need the on-disk state to be gone so iter 3 can re-alert.
    crate::alert_state::AlertState::clear(&paths, &ws).unwrap();
    assert!(
        !paths.alert_state_path(&basename).exists(),
        "alert state must be gone after clear"
    );

    // Iteration 3: simulate another push failure via the helper. State
    // file is gone (cleared in iter 2), so this re-alerts even though
    // less than 24h has elapsed since iter 1's alert.
    crate::alerts::handle_predictable_failure(
        &paths,
        &ws,
        &fixture_repo(&ws).url,
        Some(&chatops_ctx),
        true,
        crate::alert_state::AlertCategory::BranchPushFailure,
        &anyhow!("second push failure after recovery"),
    )
    .await;

    alert_mock.assert_async().await;
}

#[test]
fn build_implementer_summary_extracts_stdout_only() {
    let dir = unique_workspace("extract-stdout");
    let ws = dir.path();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    write_fixture_run_log(
        &paths,
        ws,
        "alpha",
        "PROMPT_BODY_SECRET",
        "STDOUT_NARRATIVE_VISIBLE",
        "STDERR_LOG_NOISE",
    );
    let out = build_implementer_summary(&paths, ws, &["alpha".to_string()]);
    assert!(out.contains("## Agent implementation notes"));
    assert!(out.contains("### alpha"));
    assert!(out.contains("STDOUT_NARRATIVE_VISIBLE"));
    assert!(!out.contains("PROMPT_BODY_SECRET"));
    assert!(!out.contains("STDERR_LOG_NOISE"));
    assert!(!out.contains("=== PROMPT"));
    assert!(!out.contains("=== STDERR"));
}

/// a49 task 3.4: the executor's `## Agent implementation notes` section
/// is OUT of scope for model attribution — the executor wraps the
/// Claude CLI and has no daemon-known `(provider, model)` in this
/// change. The composed notes must carry NO attribution line.
#[test]
fn build_implementer_summary_has_no_attribution_line() {
    let dir = unique_workspace("no-attribution");
    let ws = dir.path();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    write_fixture_run_log(
        &paths,
        ws,
        "alpha",
        "PROMPT",
        "implementer narrative output",
        "",
    );
    let out = build_implementer_summary(&paths, ws, &["alpha".to_string()]);
    assert!(out.contains("## Agent implementation notes"));
    // No reviewer/auditor/contradiction-check attribution, and no
    // generic italic `*<Role>: <provider>/<model>*` line.
    assert!(
        !out.contains("*Reviewer:"),
        "no reviewer attribution: {out}"
    );
    assert!(!out.contains("*Auditor"), "no auditor attribution: {out}");
    assert!(
        !out.contains("*Contradiction-check:"),
        "no contradiction-check attribution: {out}"
    );
}

#[test]
fn build_implementer_summary_skips_missing_log() {
    let dir = unique_workspace("skip-missing");
    let ws = dir.path();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    write_fixture_run_log(&paths, ws, "present", "p", "PRESENT_STDOUT", "");
    // "absent" has no log file written.
    let out = build_implementer_summary(&paths, ws, &["present".to_string(), "absent".to_string()]);
    assert!(out.contains("PRESENT_STDOUT"));
    assert!(out.contains("### present"));
    assert!(!out.contains("### absent"));
}

#[test]
fn build_implementer_summary_returns_empty_when_all_missing() {
    let dir = unique_workspace("all-missing");
    let ws = dir.path();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let out = build_implementer_summary(&paths, ws, &["nope-1".to_string(), "nope-2".to_string()]);
    assert!(out.is_empty(), "expected empty string, got: {out:?}");
}

#[test]
fn build_implementer_summary_uses_placeholder_for_empty_stdout() {
    let dir = unique_workspace("empty-stdout");
    let ws = dir.path();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    write_fixture_run_log(&paths, ws, "silent", "p", "", "");
    let out = build_implementer_summary(&paths, ws, &["silent".to_string()]);
    assert!(out.contains("### silent"));
    assert!(out.contains("_(no implementer output captured)_"));
}

#[test]
fn build_implementer_summary_reads_final_answer_from_json_log() {
    let dir = unique_workspace("final-answer");
    let ws = dir.path();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    write_fixture_json_run_log(
        &paths,
        ws,
        "alpha",
        "PROMPT_BODY",
        &[
            "[tool_use] Read foo.rs",
            "[tool_result] (123 bytes returned)",
            "[assistant] looking at the code",
        ],
        "FINAL_SUMMARY_TEXT",
        "",
    );
    let out = build_implementer_summary(&paths, ws, &["alpha".to_string()]);
    assert!(out.contains("FINAL_SUMMARY_TEXT"));
    // Action stream MUST NOT leak into the PR comment.
    assert!(!out.contains("[tool_use]"));
    assert!(!out.contains("[tool_result]"));
    assert!(!out.contains("[assistant]"));
    assert!(!out.contains("Read foo.rs"));
}

#[test]
fn build_implementer_summary_falls_back_to_timeout_placeholder() {
    let dir = unique_workspace("timeout-fallback");
    let ws = dir.path();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    write_fixture_json_run_log(
        &paths,
        ws,
        "alpha",
        "p",
        &["[tool_use] Read a"],
        "", // empty FINAL ANSWER → timeout case
        "",
    );
    let out = build_implementer_summary(&paths, ws, &["alpha".to_string()]);
    assert!(
        out.contains("(executor timed out before final summary; see daemon log for action stream)"),
        "expected timeout fallback in: {out}"
    );
}

#[test]
fn build_implementer_summary_legacy_text_mode_log_still_works() {
    // Operators with `output_format: text` produce the legacy
    // STDOUT/STDERR log shape; the PR comment must still surface
    // the raw stdout (today's behavior preserved).
    let dir = unique_workspace("legacy-shape");
    let ws = dir.path();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    write_fixture_run_log(&paths, ws, "alpha", "p", "LEGACY_STDOUT_CONTENT", "");
    let out = build_implementer_summary(&paths, ws, &["alpha".to_string()]);
    assert!(out.contains("LEGACY_STDOUT_CONTENT"));
}

#[test]
fn truncate_to_fit_appends_marker_when_exceeded() {
    let body = "x".repeat(100_000);
    let out = truncate_to_fit(body, 60_000);
    let marker = "_[summary truncated to fit GitHub comment limit;";
    assert!(out.ends_with("/<change>.log]_"));
    assert!(out.contains(marker), "missing truncation marker");
    // Total length is bounded by max + marker length.
    assert!(
        out.len() <= 60_000 + 200,
        "unexpected length: {}",
        out.len()
    );
}

#[test]
fn truncate_to_fit_passthrough_when_under_budget() {
    let body = "small body".to_string();
    let out = truncate_to_fit(body.clone(), 60_000);
    assert_eq!(out, body);
}

#[test]
fn truncate_to_fit_respects_char_boundary() {
    // Three-byte char "界" repeated. With max=10 the byte cut would
    // land mid-codepoint; the function must walk back to the prior
    // boundary.
    let body = "界".repeat(20); // 60 bytes
    let out = truncate_to_fit(body, 10);
    // Did not panic. The truncated prefix must be valid UTF-8 and end
    // on a char boundary.
    let prefix_end = out.find("\n\n_[").unwrap();
    let prefix = &out[..prefix_end];
    assert!(prefix.is_char_boundary(prefix.len()));
    assert!(prefix.chars().all(|c| c == '界'));
    // At max=10, three-byte char fits 3 times (9 bytes) — boundary
    // walks down from 10 to 9.
    assert_eq!(prefix.chars().count(), 3);
}

/// Integration: `post_implementer_summary_comment` against a mockito
/// server. Asserts the POST hits the expected endpoint AND the body
/// contains the per-change stdout sentinel pulled from the fixture
/// run-log.
#[tokio::test]
async fn post_implementer_summary_comment_hits_endpoint_with_stdout_body() {
    let dir = unique_workspace("integration-comment");
    let ws = dir.path();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    write_fixture_run_log(
        &paths,
        ws,
        "the-change",
        "p",
        "INTEGRATION_STDOUT_SENTINEL",
        "",
    );

    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/repos/upstream-org/the-repo/issues/77/comments")
        .match_header("authorization", "Bearer testtoken")
        .match_body(mockito::Matcher::Regex(
            "INTEGRATION_STDOUT_SENTINEL".to_string(),
        ))
        .with_status(201)
        .with_body(r#"{"id":1}"#)
        .expect(1)
        .create_async()
        .await;

    post_implementer_summary_comment(
        &paths,
        &server.url(),
        ws,
        "upstream-org",
        "the-repo",
        77,
        &["the-change".to_string()],
        "testtoken",
    )
    .await;

    mock.assert_async().await;
}

/// When all logs are absent the comment is NOT posted.
#[tokio::test]
async fn post_implementer_summary_comment_skips_when_summary_empty() {
    let dir = unique_workspace("integration-skip");
    let ws = dir.path();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", mockito::Matcher::Any)
        .expect(0)
        .create_async()
        .await;

    post_implementer_summary_comment(
        &paths,
        &server.url(),
        ws,
        "owner",
        "repo",
        1,
        &["no-such-change".to_string()],
        "testtoken",
    )
    .await;

    mock.assert_async().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_increments_failure_counter() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "stuck-change", "fixture reason");
    let executor = AlwaysFailingExecutor;
    // Use a high threshold so a single failure does NOT yet mark
    // perma-stuck; we are asserting only the counter side-effect here.
    let _ = run_one_pass_with_threshold(&paths, &ws, &executor, 10).await;
    let state = failure_state::load(&paths, &ws).unwrap();
    let entry = state.entries.get("stuck-change").expect("entry present");
    assert_eq!(entry.count, 1);
    assert!(
        entry.last_reason.contains("fixture failure"),
        "last_reason should capture the executor's Failed reason: {}",
        entry.last_reason
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archived_clears_failure_counter() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "recovered", "fixture");
    // Pre-populate the failure-state file with a count for this change.
    let _ = failure_state::record_failure(&paths, &ws, "recovered", "earlier fail").unwrap();
    assert!(
        failure_state::load(&paths, &ws)
            .unwrap()
            .entries
            .contains_key("recovered"),
        "fixture must have a counter entry before the pass"
    );
    let executor = CompletingExecutorWithDiff {
        artifact_name: "RECOVERED.txt".into(),
        artifact_text: "x".into(),
    };
    let processed = run_one_pass_with_threshold(&paths, &ws, &executor, 10)
        .await
        .expect("pass succeeds");
    assert_eq!(processed, vec!["recovered".to_string()]);
    let state = failure_state::load(&paths, &ws).unwrap();
    assert!(
        !state.entries.contains_key("recovered"),
        "archive must clear the failure-state entry"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn threshold_reached_writes_marker_and_excludes_change() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "doomed", "fixture");
    let executor = AlwaysFailingExecutor;

    // Pass 1: count 1, no marker.
    let _ = run_one_pass_with_threshold(&paths, &ws, &executor, 2).await;
    assert!(
        !ws.join("openspec/changes/doomed/.perma-stuck.json")
            .exists(),
        "no marker after first failure"
    );
    assert_eq!(
        queue::list_pending(&paths, &ws).unwrap(),
        vec!["doomed".to_string()],
        "change still pending after one failure"
    );

    // Pass 2: count 2 = threshold → marker written, change excluded.
    let _ = run_one_pass_with_threshold(&paths, &ws, &executor, 2).await;
    assert!(
        ws.join("openspec/changes/doomed/.perma-stuck.json")
            .exists(),
        "marker must be written when threshold is reached"
    );
    assert!(
        queue::list_pending(&paths, &ws).unwrap().is_empty(),
        "perma-stuck change must be excluded from pending"
    );
    // Marker file schema: confirm it contains the change name and count.
    let raw =
        std::fs::read_to_string(ws.join("openspec/changes/doomed/.perma-stuck.json")).unwrap();
    assert!(raw.contains("\"change\": \"doomed\""));
    assert!(raw.contains("\"consecutive_failures\": 2"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_marker_re_enables_change() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "recoverable", "fixture");
    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    struct CountingFailing(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    #[async_trait::async_trait]
    impl Executor for CountingFailing {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(ExecutorOutcome::Failed {
                reason: "fixture".into(),
            })
        }
        async fn resume(
            &self,
            _h: crate::executor::ResumeHandle,
            _a: &str,
        ) -> Result<ExecutorOutcome> {
            unreachable!()
        }
    }
    let executor = CountingFailing(invocations.clone());

    let _ = run_one_pass_with_threshold(&paths, &ws, &executor, 2).await;
    let _ = run_one_pass_with_threshold(&paths, &ws, &executor, 2).await;
    // 2 invocations so far; marker should now exist.
    assert_eq!(invocations.load(std::sync::atomic::Ordering::SeqCst), 2);
    let marker = ws.join("openspec/changes/recoverable/.perma-stuck.json");
    assert!(marker.exists(), "marker must be written by pass 2");

    // Pass 3: marker present → excluded → executor NOT invoked.
    let _ = run_one_pass_with_threshold(&paths, &ws, &executor, 2).await;
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "executor must not run while marker is present"
    );

    // Operator removes the marker.
    std::fs::remove_file(&marker).unwrap();

    // Pass 4: change is back in pending, executor runs again.
    let _ = run_one_pass_with_threshold(&paths, &ws, &executor, 2).await;
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "executor must run after the operator clears the marker"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transient_error_does_not_increment_counter() {
    // Workspace with no .git directory → workspace::ensure_initialized
    // errors out before the executor is ever invoked. The
    // failure-state file must remain absent.
    let dir = tempfile::TempDir::new().unwrap();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    let ws = dir.path().join("not-a-repo");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("placeholder.txt"), "x").unwrap();

    let repo = RepositoryConfig { forge: None,
        url: "git@github.com:owner/missing.git".into(),
        local_path: Some(ws.clone()),
        base_branch: "main".into(),
        agent_branch: "agent-q".into(),
        poll_interval_sec: 60,
        chatops_channel_id: None,
        max_changes_per_pr: None,
        audits: None,
        spec_storage: None,
        upstream: None,
        auto_submit_pr: true,
        sandbox: None,
    };
    let github_cfg = GithubConfig {
        token_env: "X".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let executor = AlwaysFailingExecutor;

    let result = run_pass_through_commits(
        &paths,
        &ws,
        &repo,
        &github_cfg,
        &executor,
        None,
        1,
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::collections::HashSet::new(),
    )
    .await;
    assert!(result.is_err(), "pre-executor failure must propagate");
    // The per-repo failure-state must remain empty — a transient
    // pre-executor error must not bump the counter.
    let state = failure_state::load(&paths, &ws).unwrap();
    assert!(
        state.entries.is_empty(),
        "transient pre-executor errors must not bump the counter; got: {:?}",
        state.entries
    );
}
