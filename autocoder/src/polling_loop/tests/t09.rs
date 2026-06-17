use super::*;

/// Enabled mode + a session that records NO submission → FAIL CLOSED: the gate
/// could not evaluate the change, so the executor is NOT invoked AND a held
/// marker with a structured `gate_error` is written (gatekeepers-fail-closed).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn contradiction_preflight_no_submission_holds_fail_closed() {
    // `Some(None)` = the agentic session ran but recorded no
    // `submit_contradictions` submission.
    let ctx = cc_test_ctx(None, None);
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change_with_spec(
        &ws,
        "transport-err",
        "newcap",
        "## ADDED Requirements\n\n### Requirement: A\nThe system SHALL a.\n",
    );

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
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
    let executor = Counter(invocations.clone());
    let fut = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX);
    let _ =
        crate::preflight::change_contradiction::scope(Some(std::sync::Arc::new(ctx)), fut).await;
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "fail-closed: the executor must NOT run when the gate could not evaluate the change"
    );
    let marker_path = ws.join("openspec/changes/transport-err/.needs-spec-revision.json");
    assert!(
        marker_path.exists(),
        "fail-closed: a held marker must be written when the gate could not run"
    );
    let marker: crate::spec_revision::SpecNeedsRevisionMarker =
        serde_json::from_str(&std::fs::read_to_string(&marker_path).unwrap()).unwrap();
    let ge = marker
        .gate_error
        .expect("the held marker must carry a structured gate_error (not a finding)");
    assert_eq!(ge.gate, "[verifier:in]", "gate_error names the [in] gate");
    assert!(
        ge.cause.contains("no submit_contradictions"),
        "gate_error cause names the failure: {}",
        ge.cause
    );
}

/// Sanity test for the marker's `revision_suggestion` text shape —
/// uses the public `build_contradiction_revision_suggestion` helper
/// directly.
#[test]
fn revision_suggestion_text_enumerates_findings() {
    let findings = vec![
        crate::preflight::change_contradiction::ContradictionFinding {
            requirement_a: "A1".into(),
            requirement_b: "B1".into(),
            summary: "S1".into(),
        },
        crate::preflight::change_contradiction::ContradictionFinding {
            requirement_a: "A2".into(),
            requirement_b: "B2".into(),
            summary: "S2".into(),
        },
    ];
    let text = build_contradiction_revision_suggestion(&findings);
    assert!(text.contains("Pre-flight contradiction check found 2 issue(s)"));
    assert!(text.contains("1. Requirement A: A1"));
    assert!(text.contains("   Requirement B: B1"));
    assert!(text.contains("   S1"));
    assert!(text.contains("2. Requirement A: A2"));
    assert!(text.contains("   Requirement B: B2"));
    assert!(text.contains("   S2"));
    assert!(text.contains("clear-revision"));
}

/// Task 4.1: disabled mode (no scoped canon context) → no session, executor
/// reached normally.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canon_preflight_disabled_proceeds_to_executor() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change(&ws, "plain", "fixture");

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
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
    let executor = Counter(invocations.clone());
    let fut = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX);
    let _ = crate::preflight::canon_contradiction::scope(None, fut).await;
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "executor must be invoked when the [canon] gate is disabled"
    );
}

/// Task 4.3 (empty): enabled mode + empty submission → executor still
/// reached (the check is a no-op outcome-wise), no marker.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canon_preflight_empty_findings_proceeds_to_executor() {
    let ctx = canon_test_ctx(Some(serde_json::json!({ "contradictions": [] })), None);
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change_with_spec(
        &ws,
        "clean",
        "newcap",
        "## ADDED Requirements\n\n### Requirement: A\nThe system SHALL a.\n",
    );

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
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
    let executor = Counter(invocations.clone());
    let fut = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX);
    let _ = crate::preflight::canon_contradiction::scope(Some(std::sync::Arc::new(ctx)), fut).await;
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "executor must be invoked when the [canon] gate returns empty findings"
    );
    assert!(
        !ws.join("openspec/changes/clean/.needs-spec-revision.json")
            .exists(),
        "no marker on empty findings"
    );
}

/// Task 4.3 (non-empty): enabled mode + findings → marker written with
/// empty structural arrays, executor NOT invoked, queue walk halts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canon_preflight_findings_write_marker_and_skip_executor() {
    let submission = serde_json::json!({
        "contradictions": [
            {
                "change_requirement": "Secrets MAY live in config.yaml",
                "canonical_capability": "security",
                "canonical_requirement": "All secrets in env vars",
                "summary": "the change re-allows what canon forbids"
            },
            {
                "change_requirement": "Cap operations at 5 minutes",
                "canonical_capability": "executor",
                "canonical_requirement": "Operations cap at 60 seconds",
                "summary": "the change exceeds the canonical cap"
            }
        ]
    });
    let ctx = canon_test_ctx(Some(submission), Some("anthropic/claude-opus-4-8".into()));
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_canonical_spec(
        &ws,
        "security",
        "## Requirements\n\n### Requirement: All secrets in env vars\nThe system SHALL store secrets only in env vars.\n",
    );
    add_committed_change_with_spec(
        &ws,
        "a-conflicting",
        "security",
        "## MODIFIED Requirements\n\n### Requirement: All secrets in env vars\nSecrets MAY live in config.yaml.\n",
    );
    // A clean change that sorts AFTER the conflicting one; it would run if
    // the gate didn't halt the queue walk on the first flagged change.
    add_committed_change(&ws, "b-runs-if-not-halted", "fixture");

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    struct Counter(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    #[async_trait::async_trait]
    impl Executor for Counter {
        async fn run(&self, _w: &Path, _c: &str) -> Result<ExecutorOutcome> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(ExecutorOutcome::Completed { final_answer: None })
        }
        async fn resume(
            &self,
            _h: crate::executor::ResumeHandle,
            _a: &str,
        ) -> Result<ExecutorOutcome> {
            unreachable!()
        }
    }
    let executor = Counter(invocations.clone());
    let fut = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX);
    let _ = crate::preflight::canon_contradiction::scope(Some(std::sync::Arc::new(ctx)), fut).await;
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "executor must NOT be invoked when canon contradictions are found (queue walk halts)"
    );

    let marker_path = ws.join("openspec/changes/a-conflicting/.needs-spec-revision.json");
    assert!(marker_path.exists(), "marker must be written");
    let raw = std::fs::read_to_string(&marker_path).unwrap();
    assert!(
        raw.contains("Pre-flight change-vs-canonical check found 2 issue(s)"),
        "revision_suggestion should announce 2 findings; got: {raw}"
    );
    assert!(raw.contains("Secrets MAY live in config.yaml"));
    assert!(raw.contains("All secrets in env vars"));
    assert!(raw.contains("capability: security"));
    assert!(raw.contains("the change re-allows what canon forbids"));

    let parsed: crate::spec_revision::SpecNeedsRevisionMarker = serde_json::from_str(&raw).unwrap();
    assert!(
        parsed.unimplementable_tasks.is_empty(),
        "unimplementable_tasks must be empty (semantic case)"
    );
    assert!(
        parsed.unarchivable_deltas.is_empty(),
        "unarchivable_deltas must be empty (semantic case)"
    );
    assert!(
        !parsed.revision_suggestion.is_empty(),
        "revision_suggestion must carry the narrative"
    );
}

/// Enabled mode + a session that records NO submission → FAIL CLOSED: the gate
/// could not evaluate the change, so the executor is NOT invoked AND a held
/// marker with a structured `gate_error` is written (gatekeepers-fail-closed).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canon_preflight_no_submission_holds_fail_closed() {
    // `None` = the agentic session ran but recorded no
    // `submit_canon_contradictions` submission.
    let ctx = canon_test_ctx(None, None);
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_change_with_spec(
        &ws,
        "transport-err",
        "newcap",
        "## ADDED Requirements\n\n### Requirement: A\nThe system SHALL a.\n",
    );

    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
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
    let executor = Counter(invocations.clone());
    let fut = run_one_pass_with_threshold(&paths, &ws, &executor, u32::MAX);
    let _ = crate::preflight::canon_contradiction::scope(Some(std::sync::Arc::new(ctx)), fut).await;
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "fail-closed: the executor must NOT run when the gate could not evaluate the change"
    );
    let marker_path = ws.join("openspec/changes/transport-err/.needs-spec-revision.json");
    assert!(
        marker_path.exists(),
        "fail-closed: a held marker must be written when the gate could not run"
    );
    let marker: crate::spec_revision::SpecNeedsRevisionMarker =
        serde_json::from_str(&std::fs::read_to_string(&marker_path).unwrap()).unwrap();
    let ge = marker
        .gate_error
        .expect("the held marker must carry a structured gate_error (not a finding)");
    assert_eq!(ge.gate, "[verifier:canon]", "gate_error names the [canon] gate");
    assert!(
        ge.cause.contains("no submit_canon_contradictions"),
        "gate_error cause names the failure: {}",
        ge.cause
    );
}

/// Sanity test for the `[canon]` gate marker's `revision_suggestion` text
/// shape — uses the `build_canon_contradiction_revision_suggestion` helper
/// directly. Each finding names the conflicting canonical requirement.
#[test]
fn canon_revision_suggestion_text_enumerates_findings() {
    let findings = vec![
        crate::preflight::canon_contradiction::CanonContradictionFinding {
            change_requirement: "CR1".into(),
            canonical_capability: "cap1".into(),
            canonical_requirement: "Canon1".into(),
            summary: "S1".into(),
        },
        crate::preflight::canon_contradiction::CanonContradictionFinding {
            change_requirement: "CR2".into(),
            canonical_capability: "cap2".into(),
            canonical_requirement: "Canon2".into(),
            summary: "S2".into(),
        },
    ];
    let text = build_canon_contradiction_revision_suggestion(&findings);
    assert!(text.contains("Pre-flight change-vs-canonical check found 2 issue(s)"));
    assert!(text.contains("1. Change requirement: CR1"));
    assert!(text.contains("Conflicting canonical requirement: Canon1 (capability: cap1)"));
    assert!(text.contains("   S1"));
    assert!(text.contains("2. Change requirement: CR2"));
    assert!(text.contains("Conflicting canonical requirement: Canon2 (capability: cap2)"));
    assert!(text.contains("   S2"));
    assert!(text.contains("clear-revision"));
}

/// `tasks_md_all_complete`: every checkbox is `[x]` → true.
#[test]
fn tasks_md_all_complete_all_checked_returns_true() {
    let dir = tempfile::TempDir::new().unwrap();
    let ws = dir.path();
    let tasks = ws.join("openspec/changes/c/tasks.md");
    std::fs::create_dir_all(tasks.parent().unwrap()).unwrap();
    std::fs::write(
        &tasks,
        "## 1. things\n- [x] 1.1 first\n- [x] 1.2 second\n  - [x] 1.3 nested\n",
    )
    .unwrap();
    let sr = crate::spec_root::SpecRoot::from_parts(ws.to_path_buf(), ws.join("openspec"), false);
    assert!(tasks_md_all_complete(&sr, "c").unwrap());
}

/// `tasks_md_all_complete`: mixed `[x]` and `[ ]` → false.
#[test]
fn tasks_md_all_complete_mixed_returns_false() {
    let dir = tempfile::TempDir::new().unwrap();
    let ws = dir.path();
    let tasks = ws.join("openspec/changes/c/tasks.md");
    std::fs::create_dir_all(tasks.parent().unwrap()).unwrap();
    std::fs::write(&tasks, "- [x] done\n- [ ] still open\n").unwrap();
    let sr = crate::spec_root::SpecRoot::from_parts(ws.to_path_buf(), ws.join("openspec"), false);
    assert!(!tasks_md_all_complete(&sr, "c").unwrap());
}

/// `tasks_md_all_complete`: every checkbox is `[ ]` → false.
#[test]
fn tasks_md_all_complete_all_open_returns_false() {
    let dir = tempfile::TempDir::new().unwrap();
    let ws = dir.path();
    let tasks = ws.join("openspec/changes/c/tasks.md");
    std::fs::create_dir_all(tasks.parent().unwrap()).unwrap();
    std::fs::write(&tasks, "- [ ] a\n- [ ] b\n").unwrap();
    let sr = crate::spec_root::SpecRoot::from_parts(ws.to_path_buf(), ws.join("openspec"), false);
    assert!(!tasks_md_all_complete(&sr, "c").unwrap());
}

/// `tasks_md_all_complete`: no checkbox lines at all → false.
/// "no tasks recorded = not complete" is the conservative path.
#[test]
fn tasks_md_all_complete_empty_returns_false() {
    let dir = tempfile::TempDir::new().unwrap();
    let ws = dir.path();
    let tasks = ws.join("openspec/changes/c/tasks.md");
    std::fs::create_dir_all(tasks.parent().unwrap()).unwrap();
    std::fs::write(&tasks, "## Heading\nNo checkboxes here.\n").unwrap();
    let sr = crate::spec_root::SpecRoot::from_parts(ws.to_path_buf(), ws.join("openspec"), false);
    assert!(!tasks_md_all_complete(&sr, "c").unwrap());
}

/// `tasks_md_all_complete`: missing file → Err.
#[test]
fn tasks_md_all_complete_missing_file_returns_err() {
    let dir = tempfile::TempDir::new().unwrap();
    let ws = dir.path();
    let sr = crate::spec_root::SpecRoot::from_parts(ws.to_path_buf(), ws.join("openspec"), false);
    assert!(tasks_md_all_complete(&sr, "does-not-exist").is_err());
}

/// Self-heal succeeds: change with every task `[x]`, valid spec, and a
/// Completed-with-empty-workspace executor result. autocoder must
/// archive, commit the move, and flag the pass as self-healing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn self_heal_archives_when_preconditions_met() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    add_committed_self_heal_change(&ws, "already-done", true, true);

    let executor = CompletingExecutorNoDiff;
    let repo = fixture_repo(&ws);
    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let (processed, includes_self_heal) = run_pass_through_commits(
        &paths,
        &ws,
        &repo,
        &github_cfg,
        &executor,
        None,
        u32::MAX,
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::sync::Mutex::new(Vec::new()),
    )
    .await
    .expect("self-heal pass succeeds");
    assert_eq!(
        processed,
        vec!["already-done".to_string()],
        "self-healed change must appear in processed list"
    );
    assert!(
        includes_self_heal,
        "pass should report includes_self_heal = true"
    );

    // Active change dir is gone; archive entry exists with date prefix.
    assert!(
        !ws.join("openspec/changes/already-done").exists(),
        "active change dir must be moved into archive"
    );
    let archive = ws.join("openspec/changes/archive");
    let archived_names: Vec<String> = std::fs::read_dir(&archive)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        archived_names.iter().any(|n| n.ends_with("-already-done")),
        "expected archived already-done in {archived_names:?}"
    );

    // Commit subject matches the spec-mandated form.
    let out = std::process::Command::new("git")
        .args(["log", "-1", "--pretty=%s", "agent-q"])
        .current_dir(&ws)
        .output()
        .unwrap();
    assert!(out.status.success());
    let subject = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert_eq!(
        subject, "archive: already-done: implementation already in base",
        "self-heal commit subject must follow the spec-mandated format"
    );

    // PR body for this pass carries the disclaimer paragraph.
    let body = build_pr_body(&ws, &processed, includes_self_heal);
    assert!(
        body.contains("_This PR archives one or more changes whose implementation was already present on the base branch."),
        "PR body should include the self-heal disclaimer; got: {body}"
    );
}

/// Self-heal precondition unmet: tasks.md has an unchecked task → the
/// pass falls through to the existing Failed path. Change must remain
/// in pending; nothing committed; nothing archived.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn self_heal_falls_through_to_failed_when_tasks_incomplete() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, paths) = crate::testing::test_daemon_paths();
    // all_done=false → tasks.md contains a `[ ]` line.
    add_committed_self_heal_change(&ws, "tasks-open", false, true);

    let pre_main = crate::git::rev_parse(&ws, "main").unwrap();

    let executor = CompletingExecutorNoDiff;
    let repo = fixture_repo(&ws);
    let github_cfg = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let (processed, includes_self_heal) = run_pass_through_commits(
        &paths,
        &ws,
        &repo,
        &github_cfg,
        &executor,
        None,
        u32::MAX,
        u32::MAX,
        &crate::audits::AuditRegistry::default(),
        None,
        &std::collections::HashMap::new(),
        &std::sync::Mutex::new(Vec::new()),
    )
    .await
    .expect("pass returns Failed via fall-through, not Err");
    assert!(
        processed.is_empty(),
        "no archived changes expected; got {processed:?}"
    );
    assert!(
        !includes_self_heal,
        "self-heal flag must remain false when preconditions unmet"
    );

    // Change is NOT archived; still in pending; no commit on agent-q.
    assert!(
        ws.join("openspec/changes/tasks-open").exists(),
        "change must remain in active changes for retry"
    );
    let archive_root = ws.join("openspec/changes/archive");
    if archive_root.exists() {
        for entry in std::fs::read_dir(&archive_root).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            assert!(
                !name.ends_with("-tasks-open"),
                "must not archive tasks-open with an open task"
            );
        }
    }
    assert_eq!(
        queue::list_pending(&paths, &ws).unwrap(),
        vec!["tasks-open".to_string()],
        "change must be back in pending"
    );
    let agent_sha = crate::git::rev_parse(&ws, "agent-q").unwrap();
    assert_eq!(agent_sha, pre_main, "no commit must be made");
}
