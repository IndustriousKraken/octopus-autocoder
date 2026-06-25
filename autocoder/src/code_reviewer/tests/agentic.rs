//! Tests for the agentic reviewer transport (a58), extracted from the
//! `code_reviewer` inline test module alongside the production move.
//! Shared fixtures (`CannedRunner`, `brief`, `valid_review_payload`,
//! `stub_with_capture`) live in the parent test module and are pulled in
//! via `use super::...`.

use super::{CannedRunner, brief, stub_with_capture, valid_review_payload};
use crate::code_reviewer::agentic::{
    AGENTIC_REVIEW_ALLOWED_TOOLS, agentic_review_allowed_tools, run_agentic_review_with_runner,
};
use crate::code_reviewer::{
    AgenticReviewOutcome, ChangedFile, CodeReviewer, REVIEWER_ROLE, ReviewContext, Verdict,
    payload_to_review_result, register_reviewer_submission_schema, render_agentic_review_prompt,
    resolve_reviewer_strategy, review_diff_artifact_rel, synthesize_agentic_per_change,
};
use crate::config::LlmProvider;
use serde_json::json;

/// 4.2: the agentic sandbox advertises Read/Glob/Grep + `submit_review`
/// AND does NOT advertise Bash/Write/Edit.
#[test]
fn agentic_sandbox_advertises_readonly_tools_plus_submit_review() {
    let tools = agentic_review_allowed_tools();
    for required in ["Read", "Glob", "Grep"] {
        assert!(
            tools.iter().any(|t| t == required),
            "must advertise {required}: {tools:?}"
        );
    }
    assert!(
        tools.iter().any(|t| t.contains("submit_review")),
        "must advertise submit_review: {tools:?}"
    );
    for forbidden in ["Bash", "Write", "Edit"] {
        assert!(
            !tools.iter().any(|t| t == forbidden),
            "must NOT advertise {forbidden}: {tools:?}"
        );
    }
}

/// 4.2 (defense in depth): the agentic sandbox settings file denies
/// `Write`/`Edit` (the read-only `deny_writes` backstop).
#[test]
fn agentic_sandbox_settings_deny_writes() {
    let sandbox = crate::config::ResolvedSandbox {
        allowed_tools: AGENTIC_REVIEW_ALLOWED_TOOLS
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
        disallowed_bash_patterns: Vec::new(),
        disallowed_read_paths: Vec::new(),
    };
    let dir = tempfile::TempDir::new().unwrap();
    let (path, _guard) =
        crate::audits::write_sandbox_settings(&sandbox, Some(dir.path()), true).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("Write(*)"), "deny list must contain Write(*): {raw}");
    assert!(raw.contains("Edit(*)"), "deny list must contain Edit(*): {raw}");
}

/// The agentic prompt lists changed-file PATHS (not their contents) AND
/// references the diff artifact (NOT the inlined diff body), AND produces
/// no budget-exhaustion footer — the agent reads files AND the diff on
/// demand, so the prompt stays bounded and `prompt_budget_chars` does not
/// apply.
#[test]
fn agentic_prompt_lists_paths_and_references_diff_artifact() {
    let artifact_rel = review_diff_artifact_rel("");
    let ctx = ReviewContext {
        archived_changes: vec![brief("demo")],
        changed_files: vec![ChangedFile {
            path: "src/big.rs".into(),
            contents: "SECRET_FILE_BODY".repeat(1000),
        }],
        diff: "DIFFBODY".into(),
        target: None,
    };
    let prompt = render_agentic_review_prompt(&ctx, "", &artifact_rel);
    assert!(prompt.contains("src/big.rs"), "path must be listed");
    assert!(
        !prompt.contains("SECRET_FILE_BODY"),
        "full file contents must NOT be inlined (read on demand)"
    );
    assert!(
        !prompt.contains("DIFFBODY"),
        "the diff body must NOT be inlined — it is referenced as an artifact"
    );
    assert!(
        prompt.contains(&artifact_rel),
        "the prompt must reference the diff artifact path"
    );
    assert!(
        !prompt.contains("Skipped (budget exhausted)"),
        "no budget-exhaustion footer in the agentic prompt"
    );
    assert!(prompt.contains("submit_review"), "must instruct submit_review");
}

/// The agentic prompt's size does not grow with the diff: a tiny diff and
/// a huge diff produce prompts of (nearly) the same length, because the
/// diff is referenced as an artifact rather than inlined.
#[test]
fn agentic_prompt_is_bounded_regardless_of_diff_size() {
    let artifact_rel = review_diff_artifact_rel("");
    let mk = |diff: String| ReviewContext {
        archived_changes: vec![brief("demo")],
        changed_files: vec![ChangedFile {
            path: "src/big.rs".into(),
            contents: String::new(),
        }],
        diff,
        target: None,
    };
    let small = render_agentic_review_prompt(&mk("a".into()), "", &artifact_rel);
    let huge = render_agentic_review_prompt(&mk("x".repeat(500_000)), "", &artifact_rel);
    assert_eq!(
        small.len(),
        huge.len(),
        "prompt length must not depend on diff size (diff is referenced, not inlined)"
    );
}

/// 4.3: a schema-valid `submit_review` payload round-trips
/// `record_submission` → `consume_submission` → the expected
/// `ReviewResult` (verdict + concerns + raw_output).
#[test]
fn submit_review_payload_round_trips_to_review_result() {
    use crate::submission_store::SubmissionStore;
    let store = SubmissionStore::new();
    register_reviewer_submission_schema(&store);
    let payload = json!({
        "verdict": "Block",
        "summary": "found a real issue",
        "concerns": [{
            "title": "sql injection",
            "detail": "user input is concatenated into the query",
            "anchor": "src/db.rs:42",
            "should_request_revision": true,
            "actionable_request": "use parameterized queries"
        }]
    });
    store
        .record("repo".into(), REVIEWER_ROLE.into(), REVIEWER_ROLE, payload)
        .expect("valid payload records");
    let consumed = store.consume("repo", REVIEWER_ROLE).expect("entry present");
    let result = payload_to_review_result(&consumed).expect("maps to ReviewResult");
    assert_eq!(result.verdict, Verdict::Block);
    assert_eq!(result.concerns.len(), 1);
    assert!(result.concerns[0].should_request_revision);
    assert_eq!(
        result.concerns[0].actionable_request.as_deref(),
        Some("use parameterized queries")
    );
    assert_eq!(result.per_concern.len(), 1);
    assert!(result.raw_output.contains("found a real issue"));
    assert!(result.raw_output.contains("sql injection"));
    // Drained: a second consume returns nothing.
    assert!(store.consume("repo", REVIEWER_ROLE).is_none());
}

/// a004 (agentic path, tasks 3.1/3.2): a `submit_review` payload that
/// flags a finding `security_critical: true` but returns `Approve` is
/// escalated to `Block` by `payload_to_review_result`, keyed on the
/// structured signal.
#[test]
fn agentic_security_finding_escalates_approve_to_block() {
    let payload = json!({
        "verdict": "Approve",
        "summary": "mostly fine",
        "concerns": [{
            "title": "api key written to opencode.json",
            "detail": "the key lands in a committable workspace file",
            "anchor": "src/config.rs:10",
            "should_request_revision": true,
            "actionable_request": "read the key from an env var at runtime",
            "security_critical": true
        }]
    });
    let result = payload_to_review_result(&payload).expect("maps to ReviewResult");
    assert_eq!(
        result.verdict,
        Verdict::Block,
        "a security_critical concern must escalate Approve to Block"
    );
    assert!(result.concerns[0].security_critical);
}

/// a004 (agentic path, task 3.3): a payload with only non-security
/// concerns (`security_critical` omitted → false) keeps its `Approve`
/// verdict — no escalation.
#[test]
fn agentic_non_security_concern_is_not_escalated() {
    let payload = json!({
        "verdict": "Approve",
        "summary": "minor nits",
        "concerns": [{
            "title": "rename tmp",
            "detail": "unclear name",
            "anchor": "src/x.rs:3",
            "should_request_revision": false
        }]
    });
    let result = payload_to_review_result(&payload).expect("maps to ReviewResult");
    assert_eq!(result.verdict, Verdict::Approve);
    assert!(!result.concerns[0].security_critical);
}

/// a004 (agentic path, task 3.4): the escalation keys on the structured
/// signal, not the wording. A credential-leak-worded but unflagged
/// concern stays `Approve`; an innocuous-worded but flagged concern
/// escalates to `Block`.
#[test]
fn agentic_escalation_keys_on_signal_not_wording() {
    let worded_but_unflagged = json!({
        "verdict": "Approve",
        "summary": "s",
        "concerns": [{
            "title": "possible credential leak / secret / api key exposure",
            "detail": "d",
            "anchor": "a",
            "should_request_revision": false
        }]
    });
    let r = payload_to_review_result(&worded_but_unflagged).expect("maps");
    assert_eq!(
        r.verdict,
        Verdict::Approve,
        "wording alone must not escalate the agentic verdict"
    );

    let flagged_but_innocuous = json!({
        "verdict": "Approve",
        "summary": "s",
        "concerns": [{
            "title": "tidy up helper foo",
            "detail": "d",
            "anchor": "a",
            "should_request_revision": false,
            "security_critical": true
        }]
    });
    let r = payload_to_review_result(&flagged_but_innocuous).expect("maps");
    assert_eq!(r.verdict, Verdict::Block);
}

/// 4.4: a non-enum verdict AND a `should_request_revision` concern with
/// an empty `actionable_request` are each rejected as a correctable
/// error; a subsequent valid submission in the same execution succeeds.
#[test]
fn submit_review_rejects_bad_verdict_and_missing_request() {
    let bad_verdict = json!({ "verdict": "LookGoodToMe", "summary": "s", "concerns": [] });
    let e = payload_to_review_result(&bad_verdict).expect_err("non-enum verdict rejected");
    assert!(e.contains("verdict"), "reason names the verdict: {e}");

    let bad_concern = json!({
        "verdict": "Block",
        "summary": "s",
        "concerns": [{
            "title": "t", "detail": "d", "anchor": "a",
            "should_request_revision": true,
            "actionable_request": ""
        }]
    });
    let e2 = payload_to_review_result(&bad_concern)
        .expect_err("should_request_revision without actionable_request rejected");
    assert!(e2.contains("actionable_request"), "reason names the field: {e2}");

    // A subsequent valid submission succeeds.
    let good = json!({ "verdict": "Approve", "summary": "s", "concerns": [] });
    assert!(payload_to_review_result(&good).is_ok());
}

/// 4.4 (store-level): a rejected `submit_review` payload stores nothing,
/// AND a subsequent valid submission for the same execution is accepted.
#[test]
fn submit_review_rejection_does_not_store_then_valid_accepted() {
    use crate::submission_store::SubmissionStore;
    let store = SubmissionStore::new();
    register_reviewer_submission_schema(&store);
    let bad = json!({ "verdict": "Maybe", "summary": "s", "concerns": [] });
    assert!(
        store
            .record("r".into(), REVIEWER_ROLE.into(), REVIEWER_ROLE, bad)
            .is_err(),
        "schema-invalid payload is rejected"
    );
    assert!(store.consume("r", REVIEWER_ROLE).is_none(), "nothing stored");
    store
        .record(
            "r".into(),
            REVIEWER_ROLE.into(),
            REVIEWER_ROLE,
            valid_review_payload("Approve"),
        )
        .expect("subsequent valid payload accepted");
    assert!(store.consume("r", REVIEWER_ROLE).is_some());
}

/// 4.5: an agentic session that ends with no valid submission discards
/// the review (no verdict written, no auto-approve).
#[tokio::test]
async fn agentic_no_submission_discards_review() {
    let (client, _) = stub_with_capture("");
    let reviewer = CodeReviewer::new(client, "t".to_string());
    let runner = CannedRunner::new(vec![None]);
    let outcome = run_agentic_review_with_runner(&reviewer, &ReviewContext::default(), &runner)
        .await
        .unwrap();
    match outcome {
        AgenticReviewOutcome::Discarded { reason } => {
            assert!(reason.contains("no valid submit_review"), "reason: {reason}");
        }
        AgenticReviewOutcome::Reviewed(_) => {
            panic!("a missing submission must discard, never produce a verdict")
        }
    }
    assert_eq!(runner.session_count(), 1);
}

/// A schema-valid submission drives a bundled `Reviewed` outcome whose
/// verdict + concerns come from the payload, AND the reviewer's
/// attribution is stamped onto the result.
#[tokio::test]
async fn agentic_valid_submission_produces_reviewed_outcome() {
    let (client, _) = stub_with_capture("");
    let reviewer = CodeReviewer::new(client, "t".to_string())
        .with_attribution(Some("anthropic/claude-opus-4-8".to_string()));
    let payload = json!({
        "verdict": "Approve",
        "summary": "all good",
        "concerns": []
    });
    let runner = CannedRunner::new(vec![Some(payload)]);
    let outcome = run_agentic_review_with_runner(&reviewer, &ReviewContext::default(), &runner)
        .await
        .unwrap();
    match outcome {
        AgenticReviewOutcome::Reviewed(r) => {
            assert_eq!(r.verdict, Verdict::Approve);
            assert!(r.per_change_sections.is_empty(), "bundled has no per-change sections");
            assert_eq!(r.attribution.as_deref(), Some("anthropic/claude-opus-4-8"));
        }
        AgenticReviewOutcome::Discarded { .. } => panic!("expected a reviewed outcome"),
    }
}

/// 4.7: `reviewer.mode: per_change` dispatches one agentic session per
/// change; the per-change results synthesize into one `ReviewResult`
/// with one section per change AND the worst-of verdict (any Block →
/// Block), feeding the same disposition the one-shot path produces.
#[tokio::test]
async fn agentic_per_change_runs_one_session_per_change() {
    let (client, _) = stub_with_capture("");
    let reviewer = CodeReviewer::new(client, "t".to_string())
        .with_mode(crate::config::ReviewerMode::PerChange);
    let ctx = ReviewContext {
        archived_changes: vec![brief("a-one"), brief("b-two"), brief("c-three")],
        changed_files: Vec::new(),
        diff: "d".into(),
        target: None,
    };
    let runner = CannedRunner::new(vec![
        Some(valid_review_payload("Approve")),
        Some(valid_review_payload("Block")),
        Some(valid_review_payload("Approve")),
    ]);
    let outcome = run_agentic_review_with_runner(&reviewer, &ctx, &runner)
        .await
        .unwrap();
    assert_eq!(runner.session_count(), 3, "one session per change");
    match outcome {
        AgenticReviewOutcome::Reviewed(r) => {
            assert_eq!(r.per_change_sections.len(), 3);
            assert_eq!(r.verdict, Verdict::Block, "any Block change blocks the PR");
            let slugs: Vec<&str> = r
                .per_change_sections
                .iter()
                .map(|s| s.change_slug.as_str())
                .collect();
            assert_eq!(slugs, vec!["a-one", "b-two", "c-three"]);
        }
        AgenticReviewOutcome::Discarded { .. } => panic!("expected a reviewed outcome"),
    }
}

/// 4.7 (per-change discard): if ANY per-change session records no valid
/// submission, the whole review is discarded (never partially approved).
#[tokio::test]
async fn agentic_per_change_one_missing_submission_discards_all() {
    let (client, _) = stub_with_capture("");
    let reviewer = CodeReviewer::new(client, "t".to_string())
        .with_mode(crate::config::ReviewerMode::PerChange);
    let ctx = ReviewContext {
        archived_changes: vec![brief("a-one"), brief("b-two")],
        changed_files: Vec::new(),
        diff: "d".into(),
        target: None,
    };
    let runner = CannedRunner::new(vec![Some(valid_review_payload("Approve")), None]);
    let outcome = run_agentic_review_with_runner(&reviewer, &ctx, &runner)
        .await
        .unwrap();
    assert!(matches!(outcome, AgenticReviewOutcome::Discarded { .. }));
}

/// a015 (agentic path): `per_change` mode whose split yields ZERO
/// sub-contexts (empty `archived_changes`) but a real diff falls back to
/// a single BUNDLED session — exactly one reviewer session runs AND the
/// verdict is the one that session returned, NOT a defaulted
/// `Approve`/`Reviewed` synthesized from zero reviews. The canned
/// submission returns `Block` precisely because `Block` is the only
/// verdict that does not map to an approval: if the pre-fix bug were
/// present (empty split → zero sessions → `synthesize_agentic_per_change`
/// defaulting to `Approve`), this assertion would fail.
#[tokio::test]
async fn agentic_per_change_empty_split_falls_back_to_bundled_with_real_verdict() {
    let (client, _) = stub_with_capture("");
    let reviewer = CodeReviewer::new(client, "t".to_string())
        .with_mode(crate::config::ReviewerMode::PerChange);
    // Empty archived_changes → split yields zero sub-contexts, but the
    // PR still has a real diff and changed files to review.
    let ctx = ReviewContext {
        archived_changes: Vec::new(),
        changed_files: vec![ChangedFile {
            path: "src/x.rs".into(),
            contents: "fn x() {}".into(),
        }],
        diff: "the union diff".into(),
        target: None,
    };
    let runner = CannedRunner::new(vec![Some(valid_review_payload("Block"))]);
    let outcome = run_agentic_review_with_runner(&reviewer, &ctx, &runner)
        .await
        .unwrap();
    assert_eq!(
        runner.session_count(),
        1,
        "empty split falls back to exactly one bundled reviewer session"
    );
    assert_eq!(
        runner.slugs.lock().unwrap().as_slice(),
        [String::new()],
        "the fallback session is bundled (empty slug), not per-change"
    );
    match outcome {
        AgenticReviewOutcome::Reviewed(r) => {
            assert_eq!(
                r.verdict,
                Verdict::Block,
                "verdict comes from the bundled review, not a defaulted Approve"
            );
            assert!(
                r.per_change_sections.is_empty(),
                "the fallback is a bundled review — no per-change sections"
            );
        }
        AgenticReviewOutcome::Discarded { .. } => {
            panic!("a valid bundled submission must produce a reviewed outcome")
        }
    }
}

/// a015 (agentic path): the fallback bundled session is handed the
/// context's diff and changed files (asserting on what the stub runner
/// received, not on any log/message wording). Proves the reviewer builds
/// its session over the real context rather than skipping the call. Since
/// the diff is no longer inlined into the prompt (it is written to the
/// artifact the runner receives via `diff`), assert the diff reaches the
/// session AND the changed-file path is listed in the prompt.
#[tokio::test]
async fn agentic_per_change_empty_split_fallback_passes_diff_and_files() {
    let (client, _) = stub_with_capture("");
    let reviewer = CodeReviewer::new(client, "t".to_string())
        .with_mode(crate::config::ReviewerMode::PerChange);
    let ctx = ReviewContext {
        archived_changes: Vec::new(),
        changed_files: vec![ChangedFile {
            path: "src/TOUCHED_SENTINEL_a015.rs".into(),
            contents: "fn x() {}".into(),
        }],
        diff: "DIFF_SENTINEL_a015".into(),
        target: None,
    };
    let runner = CannedRunner::new(vec![Some(valid_review_payload("Approve"))]);
    let _ = run_agentic_review_with_runner(&reviewer, &ctx, &runner)
        .await
        .unwrap();
    let prompts = runner.prompts.lock().unwrap();
    let diffs = runner.diffs.lock().unwrap();
    assert_eq!(prompts.len(), 1, "exactly one bundled session prompt");
    assert!(
        diffs[0].contains("DIFF_SENTINEL_a015"),
        "the fallback review's session receives the context's diff (via the artifact)"
    );
    assert!(
        prompts[0].contains("src/TOUCHED_SENTINEL_a015.rs"),
        "the fallback review receives the context's changed files"
    );
}

/// executor-outcome-legibility-and-retry §7.3: a reviewer session that records
/// no submission BUT carried captured output yields a `Discarded` whose reason
/// INCLUDES that output (surfaced raw), rather than only the bare "recorded no
/// valid submit_review submission". Driven via the existing test-runner seam;
/// asserts the captured text appears, not exact wording.
#[tokio::test]
async fn agentic_no_submission_discard_surfaces_captured_output() {
    let (client, _) = stub_with_capture("");
    let reviewer = CodeReviewer::new(client, "t".to_string());
    // The runner's session carries no submission AND a captured-output
    // diagnostic (as the production runner would assemble via `failure_reason`).
    let runner = CannedRunner::new_with_diagnostics(vec![(
        None,
        "stderr: 529 Overloaded | exit status: 1".to_string(),
    )]);
    let outcome = run_agentic_review_with_runner(&reviewer, &ReviewContext::default(), &runner)
        .await
        .unwrap();
    match outcome {
        AgenticReviewOutcome::Discarded { reason } => {
            assert!(
                reason.contains("529 Overloaded"),
                "the discard reason surfaces the captured session output: {reason}"
            );
            assert!(
                reason.contains("no valid submit_review"),
                "the discard still names the no-submission disposition: {reason}"
            );
        }
        AgenticReviewOutcome::Reviewed(_) => {
            panic!("a missing submission must discard, never produce a verdict")
        }
    }
}

/// executor-outcome-legibility-and-retry §7.3: a reviewer session persists its
/// captured output to a discoverable per-session log under `reviews/`,
/// mirroring the audit-log file pattern — so an operator can open it without
/// re-running the review. Tests the shared persist helper the production runner
/// calls (the production runner spawns a CLI, which cannot run in-test).
#[test]
fn reviewer_session_writes_discoverable_log() {
    let (_td, paths) = crate::testing::test_daemon_paths();
    let ws = tempfile::TempDir::new().unwrap();
    let outcome = crate::agentic_run::AgenticRunOutcome {
        stdout: "prose instead of a tool call".into(),
        stderr: "529 Overloaded".into(),
        ..Default::default()
    };
    let path = crate::audits::persist_reviewer_session_log(&paths, ws.path(), "bundled", &outcome)
        .expect("the reviewer session log is written");
    assert!(path.exists(), "the per-session log file exists at {}", path.display());
    let basename = ws.path().file_name().and_then(|n| n.to_str()).unwrap();
    assert!(
        path.starts_with(paths.reviewer_logs_dir(basename)),
        "the log lives under the workspace's reviews/ dir: {}",
        path.display()
    );
    assert!(
        path.extension().and_then(|e| e.to_str()) == Some("log"),
        "the file uses the .log extension: {}",
        path.display()
    );
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(body.contains("529 Overloaded"), "the captured stderr is persisted: {body}");
}

/// a015 (agentic path): the empty-input guard on
/// `synthesize_agentic_per_change` makes the "never a defaulted Approve"
/// invariant explicit. Called with an empty `reviews` vec it returns
/// `Block` (not `Approve`), so a synthesis from zero reviews can never
/// become a silent approval even if a future caller reaches it directly.
#[test]
fn synthesize_agentic_per_change_empty_input_is_block() {
    let result = synthesize_agentic_per_change(Vec::new(), Some("p/m".to_string()));
    assert_eq!(
        result.verdict,
        Verdict::Block,
        "an empty per-change synthesis must never default to Approve"
    );
    assert!(result.per_change_sections.is_empty());
    assert!(result.concerns.is_empty());
    assert_eq!(
        result.attribution.as_deref(),
        Some("p/m"),
        "attribution is preserved through the guard"
    );
}

/// A reviewer whose provider resolves (via the a55 provider→CLI rule) to
/// the `opencode` CLI now resolves to a working strategy (a60 registered
/// it); an Anthropic reviewer resolves the `claude` strategy. Neither
/// spawns a subprocess at resolution time.
#[test]
fn agentic_strategy_resolution_resolves_registered_clis() {
    let (c1, _) = stub_with_capture("");
    let opencode_reviewer = CodeReviewer::new(c1, "t".to_string())
        .with_provider(LlmProvider::OpenAiCompatible)
        .with_command("opencode".to_string());
    assert!(
        resolve_reviewer_strategy(&opencode_reviewer).is_ok(),
        "openai_compatible reviewer resolves the opencode strategy (a60)"
    );

    let (c2, _) = stub_with_capture("");
    let claude_reviewer = CodeReviewer::new(c2, "t".to_string());
    assert!(
        resolve_reviewer_strategy(&claude_reviewer).is_ok(),
        "Anthropic reviewer resolves the claude strategy"
    );
}
