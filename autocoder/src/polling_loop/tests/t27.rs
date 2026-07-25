use super::*;

// iteration-sequence-gates-once: pre-executor gate evaluation is SEQUENCE-scoped.
// A continuation pickup (iteration-pending marker present) whose gate inputs are
// byte-identical to the pass that recorded a gate-pass carries the recorded
// verdicts forward instead of re-spawning the gate sessions. Any doubt runs the
// gates in full (fail toward RUNNING). These caller-level tests drive
// `process_one_pending_change` through a full pass; the record + hash unit tests
// live in `crate::gate_pass_record`.

use crate::gate_ledger::{GateVerdict, read_ledger, write_ledger, GateLedger};
use crate::gate_pass_record::{compute_inputs_hash, read_record, write_record};

/// Completing executor that counts invocations AND writes a per-change artifact
/// so the diff is non-empty (the change archives). Used to prove the executor
/// was REACHED (i.e. the gates did not hold the change).
struct CountingCompletingExecutor(std::sync::Arc<std::sync::atomic::AtomicUsize>);

#[async_trait::async_trait]
impl Executor for CountingCompletingExecutor {
    async fn run(&self, workspace: &Path, change: &str) -> Result<ExecutorOutcome> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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

/// Executor that returns `SpecNeedsRevision` (a sequence-terminating outcome).
struct SpecRevisionExec;

#[async_trait::async_trait]
impl Executor for SpecRevisionExec {
    async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
        Ok(ExecutorOutcome::SpecNeedsRevision {
            unimplementable_tasks: fixture_unimpl_tasks(),
            revision_suggestion: "revise it".into(),
        })
    }
    async fn resume(&self, _h: crate::executor::ResumeHandle, _a: &str) -> Result<ExecutorOutcome> {
        unreachable!()
    }
}

fn basename(ws: &Path) -> &str {
    ws.file_name().and_then(|s| s.to_str()).unwrap()
}

fn change_dir(ws: &Path, change: &str) -> std::path::PathBuf {
    ws.join("openspec/changes").join(change)
}

/// Make `change` look like a continuation whose gates already passed:
/// - an iteration-pending marker (so the pickup is a continuation),
/// - a persisted `.git/` gate ledger carrying the recorded verdicts,
/// - a gate-pass record whose hash matches the change dir's current bytes.
fn seed_passed_continuation(paths: &DaemonPaths, ws: &Path, change: &str) {
    let bn = basename(ws);
    crate::iteration_pending::write_marker(
        paths,
        bn,
        change,
        &crate::iteration_pending::IterationPendingMarker {
            completed_tasks: vec![],
            remaining_tasks: vec!["do thing".into()],
            reason: "wip".into(),
            iteration_number: 2,
        },
    )
    .unwrap();

    let mut prior = GateLedger::new();
    prior.set_in(GateVerdict::Pass, Some("m-in".into()), None);
    prior.set_canon(GateVerdict::Disabled, None, None);
    prior.set_rules(GateVerdict::Disabled, None, None);
    write_ledger(ws, change, &prior).unwrap();

    let h = compute_inputs_hash(&change_dir(ws, change)).unwrap();
    write_record(paths, bn, change, &h).unwrap();
}

/// An `[in]` gate context whose canned submission reports a contradiction — if
/// the gate is ever SPAWNED it FAILs and holds the change. Skipping it is what
/// lets the executor run.
fn in_gate_would_fail() -> crate::preflight::change_contradiction::ContradictionCheckCtx {
    cc_test_ctx(
        Some(serde_json::json!({
            "contradictions": [
                { "requirement_a": "A", "requirement_b": "B", "summary": "A and B conflict" }
            ]
        })),
        Some("anthropic/claude-in".into()),
    )
}

/// 3.1: a continuation pickup with an UNCHANGED hash spawns no pre-executor gate
/// sessions (the executor runs even though an installed `[in]` gate WOULD fail)
/// AND the ledger renders the carried-forward verdicts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continuation_unchanged_hash_carries_forward_and_skips_sessions() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "cont", "fixture");
    seed_passed_continuation(&paths, &ws, "cont");

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executor = CountingCompletingExecutor(invocations.clone());
    // The `[in]` gate is ENABLED and WOULD fail — but the carry path skips it.
    let fut = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX);
    crate::preflight::change_contradiction::scope(Some(std::sync::Arc::new(in_gate_would_fail())), fut)
        .await
        .expect("pass succeeds");

    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the executor must run: a skipped [in] gate cannot hold the change (if it had spawned, the FAIL submission would have held it)"
    );
    // The ledger carries the recorded [in] Pass forward, annotated.
    let ledger = read_ledger(&ws, "cont").expect("ledger persisted");
    assert_eq!(ledger.r#in.verdict, GateVerdict::Pass);
    assert!(
        ledger.r#in.carried_forward,
        "the carried [in] verdict must be annotated as carried forward"
    );
    let section = ledger.render_pr_section();
    assert!(
        section.contains("carried forward (sequence)"),
        "the PR ledger must render the carried-forward annotation: {section}"
    );
}

/// 3.2: a mid-sequence edit to any file in the change's directory produces a hash
/// mismatch, so the gates run in FULL — the enabled `[in]` gate then FAILs and
/// holds the change (the executor never runs).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mid_sequence_edit_re_gates_in_full() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "edited", "fixture");
    seed_passed_continuation(&paths, &ws, "edited");

    // Mid-sequence edit: append to tasks.md AND commit so it survives any
    // workspace hygiene. The recorded hash was taken before this edit.
    let tasks = change_dir(&ws, "edited").join("tasks.md");
    let mut body = std::fs::read_to_string(&tasks).unwrap();
    body.push_str("- [ ] a newly added task\n");
    std::fs::write(&tasks, body).unwrap();
    for args in [&["add", "-A"][..], &["commit", "-q", "-m", "edit tasks"][..]] {
        let st = std::process::Command::new("git")
            .args(args)
            .current_dir(&ws)
            .status()
            .unwrap();
        assert!(st.success());
    }

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executor = CountingCompletingExecutor(invocations.clone());
    let fut = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX);
    let _ = crate::preflight::change_contradiction::scope(
        Some(std::sync::Arc::new(in_gate_would_fail())),
        fut,
    )
    .await;

    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a hash mismatch must re-run the gates in full; the enabled [in] gate FAILs and holds the change (executor never runs)"
    );
    assert!(
        ws.join("openspec/changes/edited/.needs-spec-revision.json").exists(),
        "the re-run [in] gate must write the hold marker"
    );
    let ledger = read_ledger(&ws, "edited").expect("ledger persisted");
    assert_eq!(ledger.r#in.verdict, GateVerdict::Fail);
    assert!(!ledger.r#in.carried_forward, "a freshly-run gate is not carried forward");
}

/// 3.3: a missing gate-pass record on a continuation runs the gates in FULL (the
/// skip fails toward running). With the enabled `[in]` gate failing, the change
/// is held.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_record_re_gates_in_full() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "norec", "fixture");
    // Continuation marker + prior ledger present, but NO gate-pass record.
    let bn = basename(&ws);
    crate::iteration_pending::write_marker(
        &paths,
        bn,
        "norec",
        &crate::iteration_pending::IterationPendingMarker {
            completed_tasks: vec![],
            remaining_tasks: vec!["do thing".into()],
            reason: "wip".into(),
            iteration_number: 2,
        },
    )
    .unwrap();
    let mut prior = GateLedger::new();
    prior.set_in(GateVerdict::Pass, Some("m".into()), None);
    write_ledger(&ws, "norec", &prior).unwrap();

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executor = CountingCompletingExecutor(invocations.clone());
    let fut = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX);
    let _ = crate::preflight::change_contradiction::scope(
        Some(std::sync::Arc::new(in_gate_would_fail())),
        fut,
    )
    .await;

    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a missing record must re-run the gates in full (fail toward running); the [in] gate FAILs and holds"
    );
    assert!(
        ws.join("openspec/changes/norec/.needs-spec-revision.json").exists(),
        "the re-run [in] gate must write the hold marker"
    );
}

/// 3.4: a FRESH pickup (no iteration-pending marker) always runs the gates, even
/// when a stale record exists, AND the record is replaced on pass. Uses a Failed
/// executor so the freshly-written record survives (Failed does not terminate the
/// sequence) and can be inspected.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_pickup_ignores_stale_record_and_replaces_on_pass() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "fresh", "fixture");
    let bn = basename(&ws);
    // A STALE record whose hash cannot match the change dir. No marker → fresh.
    write_record(&paths, bn, "fresh", "0000stale0000").unwrap();

    // Clean `[in]` gate (empty contradictions → Pass) so the gates pass and the
    // record is (re)written.
    let ctx = cc_test_ctx(Some(serde_json::json!({ "contradictions": [] })), Some("m".into()));
    let executor = AlwaysFailingExecutor;
    let fut = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX);
    let _ = crate::preflight::change_contradiction::scope(Some(std::sync::Arc::new(ctx)), fut).await;

    // Gates RAN (not carried forward): [in] recorded a fresh Pass.
    let ledger = read_ledger(&ws, "fresh").expect("ledger persisted");
    assert_eq!(ledger.r#in.verdict, GateVerdict::Pass);
    assert!(
        !ledger.r#in.carried_forward,
        "a fresh pickup runs the gates; the verdict is not carried forward"
    );
    // The stale record was REPLACED with the change dir's current hash (Failed
    // does not remove it).
    let rec = read_record(&paths, bn, "fresh").expect("record replaced, not removed on Failed");
    assert_ne!(rec.inputs_hash, "0000stale0000", "the stale record must be replaced");
    assert_eq!(
        rec.inputs_hash,
        compute_inputs_hash(&change_dir(&ws, "fresh")).unwrap(),
        "the replaced record carries the change dir's current gate-inputs hash"
    );
}

/// 3.5: the gate-pass record is removed on each sequence-terminating outcome
/// (`Completed`, `SpecNeedsRevision`) — the paths where the iteration-pending
/// marker is dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn record_removed_on_terminating_outcomes() {
    // --- Completed ---
    {
        let (_dir, ws) = fixture_workspace_with_remote();
        let (_td, paths) = crate::testing::test_daemon_paths();
        add_committed_change(&ws, "done", "fixture");
        let bn = basename(&ws);
        write_record(&paths, bn, "done", "seeded-hash").unwrap();

        // Gates disabled (no scope) → pass as Disabled → executor → Completed.
        let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let executor = CountingCompletingExecutor(invocations.clone());
        run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX)
            .await
            .expect("pass succeeds");
        assert_eq!(invocations.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            read_record(&paths, bn, "done").is_none(),
            "Completed terminates the sequence: the gate-pass record must be removed"
        );
    }
    // --- SpecNeedsRevision ---
    {
        let (_dir, ws) = fixture_workspace_with_remote();
        let (_td, paths) = crate::testing::test_daemon_paths();
        add_committed_change(&ws, "revise", "fixture");
        let bn = basename(&ws);
        write_record(&paths, bn, "revise", "seeded-hash").unwrap();

        let executor = SpecRevisionExec;
        run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX)
            .await
            .expect("pass succeeds");
        assert!(
            ws.join("openspec/changes/revise/.needs-spec-revision.json").exists(),
            "SpecNeedsRevision writes the marker"
        );
        assert!(
            read_record(&paths, bn, "revise").is_none(),
            "SpecNeedsRevision terminates the sequence: the gate-pass record must be removed"
        );
    }
}
