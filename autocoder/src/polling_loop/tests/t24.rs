use super::*;

/// Commit a change carrying a custom `tasks.md` body (the helpers in
/// `support0` hard-code `- [ ] do thing`; the canon-editing pre-flight is a
/// `tasks.md`-content check, so these tests need to control that content).
fn add_committed_change_with_tasks(workspace: &Path, name: &str, tasks_body: &str) {
    let dir = workspace.join("openspec/changes").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("proposal.md"), format!("## Why\nfixture {name}\n")).unwrap();
    std::fs::write(dir.join("tasks.md"), tasks_body).unwrap();
    for args in [
        vec!["add", "-A"],
        vec!["commit", "-q", "-m", "scaffold"],
    ] {
        let st = std::process::Command::new("git")
            .args(&args)
            .current_dir(workspace)
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?} failed");
    }
}

/// A counting executor that records a Failed outcome — used to assert whether
/// the executor was reached without triggering the archive/PR machinery.
struct Counter(std::sync::Arc<std::sync::atomic::AtomicUsize>);
#[async_trait::async_trait]
impl Executor for Counter {
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

/// Task 3.1: a `tasks.md` with an "apply … to openspec/specs/…" task is flagged
/// by the pre-flight — the marker is written with `canon_editing_tasks`, the
/// executor is NOT invoked, and (because the reject precedes them) the
/// `[in]`/`[canon]` gates do not run. Asserts behaviour/state, not wording.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preflight_rejects_canon_editing_task() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change_with_tasks(
        &ws,
        "01-canon-editor",
        "## 1. Spec update\n- [ ] 1.1 Apply the ADDED Requirements block from specs/cap/spec.md to openspec/specs/cap/spec.md\n",
    );
    // A clean trailing change that would run if the queue didn't halt.
    add_committed_change(&ws, "02-clean", "fixture");

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executor = Counter(invocations.clone());
    let _ = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX).await;

    // Executor never invoked: the reject precedes both the executor and the
    // verifier gates.
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "pre-flight must reject before any executor invocation"
    );

    // The marker is the canon-editing one: `canon_editing_tasks` names the
    // offending task; the gate/contradiction/archivability populations are empty.
    let marker_path = ws.join("openspec/changes/01-canon-editor/.needs-spec-revision.json");
    assert!(marker_path.exists(), "marker must be written");
    let parsed: crate::spec_revision::SpecNeedsRevisionMarker =
        serde_json::from_str(&std::fs::read_to_string(&marker_path).unwrap()).unwrap();
    assert_eq!(parsed.canon_editing_tasks.len(), 1);
    assert!(parsed.canon_editing_tasks[0].contains("openspec/specs/cap/spec.md"));
    assert!(parsed.unarchivable_deltas.is_empty());
    assert!(parsed.unimplementable_tasks.is_empty());
    assert!(parsed.contradictions.is_empty());
    assert!(parsed.gate_error.is_none());

    // Same-repo blocking: the clean trailing change is not processed this pass.
    assert!(
        ws.join("openspec/changes/02-clean").exists(),
        "the clean trailing change must remain in pending"
    );
}

/// Task 3.2: a `tasks.md` that references the change's OWN delta
/// (`openspec/changes/<slug>/specs/…`), or mentions canon read-only, is NOT
/// flagged — the change proceeds to the executor.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preflight_allows_own_delta_and_readonly_canon() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change_with_tasks(
        &ws,
        "01-own-delta",
        "## 1. Author delta + tests\n- [ ] 1.1 Add a scenario to openspec/changes/01-own-delta/specs/cap/spec.md\n- [ ] 1.2 Ensure the code matches the existing contract in openspec/specs/cap/spec.md\n",
    );

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executor = Counter(invocations.clone());
    let _ = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX).await;

    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "own-delta + read-only canon reference must reach the executor"
    );
    assert!(
        !ws.join("openspec/changes/01-own-delta/.needs-spec-revision.json")
            .exists(),
        "no canon-editing marker for a non-flagged change"
    );
}

/// Task 3.3: a clean code-and-tests `tasks.md` is NOT flagged — the change
/// reaches the executor exactly as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preflight_allows_clean_code_and_tests_tasks() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change_with_tasks(
        &ws,
        "01-clean",
        "## 1. Fix the bug\n- [ ] 1.1 In `src/handlers/upload.rs::receive_file`, reject `..` paths\n- [ ] 1.2 Add unit test `receive_file_rejects_path_traversal`\n",
    );

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executor = Counter(invocations.clone());
    let _ = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX).await;

    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "clean code-and-tests change must reach the executor"
    );
    assert!(
        !ws.join("openspec/changes/01-clean/.needs-spec-revision.json")
            .exists(),
        "no marker for a clean change"
    );
}
