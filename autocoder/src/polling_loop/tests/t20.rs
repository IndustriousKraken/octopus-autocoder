use super::*;

// verifier-gates-fail-closed §5–§6: the default-deny gate-verdict ledger AND
// its PR-rendered verdicts. The ledger-type unit tests (blocking_ok truth table,
// render, persistence) live in `crate::gate_ledger`; these are the caller-level
// tests that the no-skip dispatch records a verdict for every gate slot AND the
// PR body renders the ledger.

/// Executor that records invocation count AND returns Completed-with-diff so the
/// change archives (the pass reaches the post-executor / PR-assembly path).
struct CountingCompletingExecutor(std::sync::Arc<std::sync::atomic::AtomicUsize>);

#[async_trait::async_trait]
impl Executor for CountingCompletingExecutor {
    async fn run(&self, workspace: &Path, change: &str) -> Result<ExecutorOutcome> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Write a per-change artifact so the diff is non-empty.
        std::fs::write(
            workspace.join(format!("artifact-{change}.txt")),
            format!("impl for {change}\n"),
        )?;
        Ok(ExecutorOutcome::Completed { final_answer: None })
    }
    async fn resume(&self, _h: crate::executor::ResumeHandle, _a: &str) -> Result<ExecutorOutcome> {
        unreachable!()
    }
}

/// §5.4: every blocking gate disabled → the no-skip dispatch records `Disabled`
/// (NOT an absence) for `[in]` AND `[canon]`, the executor proceeds (disabled is
/// non-blocking), AND the per-change ledger is persisted under `.git/` (NOT the
/// working tree).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_blocking_gates_record_disabled_and_proceed() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "plain", "fixture");

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executor = CountingCompletingExecutor(invocations.clone());
    // No `change_contradiction::scope` / `canon_contradiction::scope` wrappers →
    // both gates are disabled.
    run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX)
        .await
        .expect("pass succeeds");
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "disabled blocking gates are non-blocking — the executor must proceed"
    );

    let ledger = crate::gate_ledger::read_ledger(&ws, "plain")
        .expect("the dispatch must persist a ledger under .git/ even when gates are disabled");
    assert_eq!(
        ledger.r#in.verdict,
        crate::gate_ledger::GateVerdict::Disabled,
        "[in] slot records Disabled via a stub, not an absence"
    );
    assert_eq!(
        ledger.canon.verdict,
        crate::gate_ledger::GateVerdict::Disabled,
        "[canon] slot records Disabled via a stub, not an absence"
    );
    // global-rules-gate task 6.1: default-disabled → no `[rules]` session; the
    // stub records `Disabled` (not an absence) AND the executor proceeds.
    assert_eq!(
        ledger.rules.verdict,
        crate::gate_ledger::GateVerdict::Disabled,
        "[rules] slot records Disabled via a stub, not an absence"
    );
    assert!(ledger.blocking_ok(), "all Disabled blocking gates are blocking-ok");

    // a16: the ledger lives under .git/, never the managed working tree.
    assert!(
        ws.join(".git/autocoder-gate-ledger/plain.json").exists(),
        "ledger persists under .git/"
    );
    assert!(
        !ws.join("openspec/changes/plain/.gate-ledger.json").exists(),
        "ledger must NOT be written into the change's working-tree directory"
    );
}

/// §5.4: an enabled `[in]` gate that returns a CLEAN result records `Pass` (with
/// the model that ran it) in the persisted ledger AND the executor proceeds —
/// the executor runs ONLY when every blocking gate is Pass/Disabled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clean_in_gate_records_pass_with_model_and_proceeds() {
    // `Some({contradictions: []})` = the agentic session submitted an empty
    // array → a clean PASS.
    let ctx = cc_test_ctx(
        Some(serde_json::json!({ "contradictions": [] })),
        Some("anthropic/claude-opus-4-8".into()),
    );
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change_with_spec(
        &ws,
        "clean",
        "newcap",
        "## ADDED Requirements\n\n### Requirement: A\nThe system SHALL a.\n",
    );

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executor = CountingCompletingExecutor(invocations.clone());
    let fut = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX);
    crate::preflight::change_contradiction::scope(Some(std::sync::Arc::new(ctx)), fut)
        .await
        .expect("pass succeeds");
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a clean [in] gate (Pass) lets the executor proceed"
    );

    let ledger = crate::gate_ledger::read_ledger(&ws, "clean").expect("ledger persisted");
    assert_eq!(
        ledger.r#in.verdict,
        crate::gate_ledger::GateVerdict::Pass,
        "a clean session records Pass"
    );
    assert_eq!(
        ledger.r#in.model.as_deref(),
        Some("claude-test"),
        "the [in] row records the model that ran the gate"
    );
    // [canon] is disabled in this test → Disabled.
    assert_eq!(
        ledger.canon.verdict,
        crate::gate_ledger::GateVerdict::Disabled
    );
    assert!(ledger.blocking_ok());
}

/// §5 (defensive proceed-gate / structural fail-closed): a blocking gate left
/// `Pending` — modeling a runner that returned without recording — holds the
/// change without any code anticipating the specific failure. This is the
/// structural guarantee the dispatch's `if !ledger.blocking_ok() { hold }` relies
/// on: `Pending` is non-passing by construction.
#[test]
fn pending_blocking_gate_is_not_blocking_ok() {
    let mut ledger = crate::gate_ledger::GateLedger::new();
    // [in] never recorded → Pending (the default). [canon] is a clean Pass.
    ledger.set_canon(crate::gate_ledger::GateVerdict::Pass, Some("m".into()), None);
    assert_eq!(ledger.r#in.verdict, crate::gate_ledger::GateVerdict::Pending);
    assert!(
        !ledger.blocking_ok(),
        "a Pending blocking gate must hold the change (default-deny)"
    );
}

/// §6.1 / §6.3: the PR body's `## Gate verdicts` section names each verifier gate
/// (`[verifier:in]`/`[verifier:canon]`/`[verifier:out]`), the model that ran it,
/// AND its verdict — AND folds in the agentic reviewer's verdict. A `PASS` is
/// VISIBLE there, not inferred from the silent absence of an alert.
#[test]
fn pr_section_renders_each_gate_model_verdict_and_reviewer() {
    use crate::code_reviewer::{ReviewReport, ReviewVerdict};
    use crate::gate_ledger::{GateLedger, GateVerdict};

    let mut ledger = GateLedger::new();
    ledger.set_in(GateVerdict::Pass, Some("anthropic/claude-in".into()), None);
    ledger.set_canon(GateVerdict::Disabled, None, None);
    ledger.set_out(
        GateVerdict::Fail,
        Some("anthropic/claude-out".into()),
        Some("1 gap(s) found".into()),
    );
    let report = ReviewReport {
        verdict: ReviewVerdict::Concerns,
        markdown: "## Code Review\n...".into(),
        concerns: Vec::new(),
        per_change_sections: Vec::new(),
        attribution: Some("xai/grok".into()),
    };
    let section = render_gate_verdicts_with_reviewer(&ledger, Some(&report));

    assert!(section.starts_with("## Gate verdicts"), "{section}");
    // Each gate identifier appears.
    assert!(section.contains("[verifier:in]"), "{section}");
    assert!(section.contains("[verifier:canon]"), "{section}");
    assert!(section.contains("[verifier:out]"), "{section}");
    // Verdicts — a PASS is visible.
    assert!(section.contains("PASS"), "{section}");
    assert!(section.contains("DISABLED"), "{section}");
    assert!(section.contains("FAIL"), "{section}");
    // Models (for the gates that ran with a known model).
    assert!(section.contains("anthropic/claude-in"), "{section}");
    assert!(section.contains("anthropic/claude-out"), "{section}");
    // The [out] FAIL row carries its one-line summary.
    assert!(section.contains("1 gap(s) found"), "{section}");
    // The agentic reviewer's verdict + model are folded in.
    assert!(section.contains("reviewer CONCERNS"), "{section}");
    assert!(section.contains("xai/grok"), "{section}");
}

/// reviewer-failure-visible-in-pr: a discarded/errored agentic review renders a
/// VISIBLE `## Code Review: FAILED TO RUN` report (the PR-body section), naming
/// the cause — NOT a silent omission AND NOT an approval.
#[test]
fn reviewer_failed_to_run_report_is_visible_and_not_an_approval() {
    use crate::code_reviewer::ReviewVerdict;
    use crate::polling_loop::pass::reviewer_failed_to_run_report;

    let report = reviewer_failed_to_run_report(
        "agentic reviewer session recorded no valid submit_review submission",
    );
    // Visible failed-to-run section in the PR body.
    assert!(
        report.markdown.contains("## Code Review: FAILED TO RUN"),
        "markdown: {}",
        report.markdown
    );
    // Names the cause.
    assert!(
        report.markdown.contains("no valid submit_review"),
        "markdown: {}",
        report.markdown
    );
    // A could-not-run state — NOT an approval/pass AND not a Block (advisory).
    assert_eq!(report.verdict, ReviewVerdict::FailedToRun);
    assert_ne!(report.verdict, ReviewVerdict::Pass);
}

/// reviewer-failure-visible-in-pr: the gate-verdict ledger records the reviewer
/// as FAILED TO RUN on a discarded/errored review — NOT passed/approved AND NOT
/// absent (so "could not run" is distinguishable from "ran and approved").
#[test]
fn ledger_records_reviewer_failed_to_run() {
    use crate::code_reviewer::ReviewVerdict;
    use crate::gate_ledger::{GateLedger, GateVerdict};
    use crate::polling_loop::pass::reviewer_failed_to_run_report;

    let mut ledger = GateLedger::new();
    ledger.set_out(GateVerdict::Pass, Some("anthropic/claude-out".into()), None);
    let report = reviewer_failed_to_run_report("agentic reviewer failed: spawn error");
    assert_eq!(report.verdict, ReviewVerdict::FailedToRun);

    let section = render_gate_verdicts_with_reviewer(&ledger, Some(&report));
    // The reviewer line is present AND says FAILED TO RUN — not absent, not PASS.
    assert!(
        section.contains("reviewer FAILED TO RUN"),
        "ledger must record the reviewer as failed-to-run: {section}"
    );
    assert!(
        !section.contains("reviewer PASS"),
        "a failed-to-run reviewer must NOT read as PASS: {section}"
    );
}

/// §6.2: `seed_ledger_from_processed` reads the pre-executor verdicts persisted
/// per change during the queue walk, so the PR-level ledger carries the EXACT
/// `[in]`/`[canon]` verdicts (not a re-derivation). A processed change with no
/// persisted ledger seeds a fresh (all-`Pending`) ledger.
#[test]
fn seed_ledger_reads_persisted_pre_executor_verdicts() {
    use crate::gate_ledger::{GateLedger, GateVerdict};
    let dir = tempfile::TempDir::new().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(ws.join(".git")).unwrap();

    let mut persisted = GateLedger::new();
    persisted.set_in(GateVerdict::Pass, Some("m-in".into()), None);
    persisted.set_canon(GateVerdict::Disabled, None, None);
    crate::gate_ledger::write_ledger(ws, "c1", &persisted).unwrap();

    let seeded = seed_ledger_from_processed(ws, &["c1".to_string()]);
    assert_eq!(seeded.r#in.verdict, GateVerdict::Pass);
    assert_eq!(seeded.r#in.model.as_deref(), Some("m-in"));
    assert_eq!(seeded.canon.verdict, GateVerdict::Disabled);

    // No persisted ledger → fresh all-Pending ledger.
    let fresh = seed_ledger_from_processed(ws, &["never-ran".to_string()]);
    assert_eq!(fresh.r#in.verdict, GateVerdict::Pending);
    assert_eq!(fresh.canon.verdict, GateVerdict::Pending);
}

/// Seed a tiny global rule corpus in a standalone tempdir (outside the managed
/// workspace) AND return it; the caller keeps the `TempDir` alive.
fn seeded_rule_corpus() -> tempfile::TempDir {
    let corpus = tempfile::TempDir::new().unwrap();
    std::fs::write(
        corpus.path().join("no-secrets.md"),
        "Secrets are never committed to the repository.",
    )
    .unwrap();
    corpus
}

/// global-rules-gate task 6.2: enabled `[rules]` gate + a change that violates a
/// seeded rule → the agent submits `submit_rule_violations` naming the rule id,
/// the marker is written naming the rule, the executor is NOT invoked, AND the
/// queue walk halts. (Behavior + ledger state asserted.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rules_violation_writes_marker_and_halts() {
    let corpus = seeded_rule_corpus();
    let submission = serde_json::json!({
        "violations": [
            { "rule_id": "no-secrets", "summary": "the change stores an API key in a tracked config file" }
        ]
    });
    let ctx = gr_test_ctx(
        Some(submission),
        Some("anthropic/claude-opus-4-8".into()),
        corpus.path().to_path_buf(),
    );
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change_with_spec(
        &ws,
        "leaks-secret",
        "security",
        "## ADDED Requirements\n\n### Requirement: Store key\nThe system SHALL store the API key in config.yaml.\n",
    );
    // A clean sibling that sorts AFTER; it must NOT run (the walk halts).
    add_committed_change(&ws, "z-runs-if-not-halted", "fixture");

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executor = CountingCompletingExecutor(invocations.clone());
    let fut = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX);
    let _ = crate::preflight::global_rules::scope(Some(std::sync::Arc::new(ctx)), fut).await;

    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "executor must NOT be invoked when a global-rule violation is found (queue walk halts)"
    );
    let marker_path = ws.join("openspec/changes/leaks-secret/.needs-spec-revision.json");
    assert!(marker_path.exists(), "marker must be written");
    let raw = std::fs::read_to_string(&marker_path).unwrap();
    assert!(
        raw.contains("no-secrets"),
        "revision_suggestion must name the violated rule by id; got: {raw}"
    );
    let parsed: crate::spec_revision::SpecNeedsRevisionMarker = serde_json::from_str(&raw).unwrap();
    assert!(
        parsed.unimplementable_tasks.is_empty() && parsed.unarchivable_deltas.is_empty(),
        "semantic-finding shape: unimplementable_tasks AND unarchivable_deltas empty"
    );
    assert!(
        parsed.gate_error.is_none(),
        "a findings marker carries NO gate_error (that is the could-not-run shape)"
    );

    let ledger = crate::gate_ledger::read_ledger(&ws, "leaks-secret").expect("ledger persisted");
    assert_eq!(ledger.rules.verdict, crate::gate_ledger::GateVerdict::Fail);
    assert!(!ledger.blocking_ok(), "a Fail [rules] verdict holds the change");
    // The PR-body ledger summary for a `[rules]` Fail names "rule violations",
    // NOT the `[in]`/`[canon]` gates' "contradiction findings" noun.
    let summary = ledger
        .rules
        .summary
        .as_deref()
        .expect("a [rules] Fail row carries a one-line summary");
    assert!(
        summary.contains("rule violations"),
        "the [rules] Fail summary must read 'rule violations', not 'contradiction findings'; got: {summary}"
    );
}

/// global-rules-gate task 6.3: enabled `[rules]` gate + a clean change (empty
/// `violations`) → proceeds to the executor, no marker, `[rules]` records Pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rules_clean_change_proceeds_no_marker() {
    let corpus = seeded_rule_corpus();
    let ctx = gr_test_ctx(
        Some(serde_json::json!({ "violations": [] })),
        Some("anthropic/claude-opus-4-8".into()),
        corpus.path().to_path_buf(),
    );
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change_with_spec(
        &ws,
        "clean-rules",
        "newcap",
        "## ADDED Requirements\n\n### Requirement: A\nThe system SHALL a.\n",
    );

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executor = CountingCompletingExecutor(invocations.clone());
    let fut = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX);
    crate::preflight::global_rules::scope(Some(std::sync::Arc::new(ctx)), fut)
        .await
        .expect("pass succeeds");

    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a clean [rules] gate (empty violations → Pass) lets the executor proceed"
    );
    assert!(
        !ws.join("openspec/changes/clean-rules/.needs-spec-revision.json").exists(),
        "no marker is written on a clean global-rules result"
    );
    let ledger = crate::gate_ledger::read_ledger(&ws, "clean-rules").expect("ledger persisted");
    assert_eq!(ledger.rules.verdict, crate::gate_ledger::GateVerdict::Pass);
    assert_eq!(
        ledger.rules.model.as_deref(),
        Some("claude-test"),
        "the [rules] row records the model that ran the gate"
    );
}

/// global-rules-gate task 6.4: enabled `[rules]` gate + a session that records NO
/// submission → FAIL CLOSED: the executor is NOT invoked, a held marker with a
/// structured `gate_error` labeled `[verifier:rules]` is written — never "no
/// violations".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rules_no_submission_holds_fail_closed() {
    let corpus = seeded_rule_corpus();
    // `None` = the agentic session ran but recorded no submission.
    let ctx = gr_test_ctx(None, None, corpus.path().to_path_buf());
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change_with_spec(
        &ws,
        "rules-transport-err",
        "newcap",
        "## ADDED Requirements\n\n### Requirement: A\nThe system SHALL a.\n",
    );

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executor = CountingCompletingExecutor(invocations.clone());
    let fut = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX);
    let _ = crate::preflight::global_rules::scope(Some(std::sync::Arc::new(ctx)), fut).await;

    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "fail-closed: the executor must NOT run when the [rules] gate could not evaluate the change"
    );
    let marker_path = ws.join("openspec/changes/rules-transport-err/.needs-spec-revision.json");
    assert!(marker_path.exists(), "fail-closed: a held marker must be written");
    let marker: crate::spec_revision::SpecNeedsRevisionMarker =
        serde_json::from_str(&std::fs::read_to_string(&marker_path).unwrap()).unwrap();
    let ge = marker
        .gate_error
        .expect("the held marker must carry a structured gate_error (not a finding)");
    assert_eq!(ge.gate, "[verifier:rules]", "gate_error names the [rules] gate");
    let ledger = crate::gate_ledger::read_ledger(&ws, "rules-transport-err").expect("ledger");
    assert_eq!(
        ledger.rules.verdict,
        crate::gate_ledger::GateVerdict::FailedToRun
    );
}
