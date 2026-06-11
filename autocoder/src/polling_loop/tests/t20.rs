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
    assert!(ledger.blocking_ok(), "two Disabled blocking gates are blocking-ok");

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
