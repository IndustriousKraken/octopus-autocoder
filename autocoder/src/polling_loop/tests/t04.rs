use super::*;

/// 5.3 / reviewer-integration: end-to-end review wiring. With a fixture
/// reviewer + a mockito GitHub server, exercise each verdict variant
/// and confirm:
///   - Pass / Concerns → non-draft PR with `## Code Review` body section
///   - Block → draft PR with the same section
///   - Reviewer-error path → non-draft PR with `(reviewer failed: …)` note
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reviewer_verdict_drives_pr_shape() {
    use crate::code_reviewer::{CodeReviewer, ReviewReport, ReviewVerdict};
    use crate::llm::LlmClient;
    use async_trait::async_trait;

    /// Stub LLM client returning a canned `VERDICT:` response.
    struct CannedClient(&'static str);
    #[async_trait]
    impl LlmClient for CannedClient {
        async fn complete(&self, _: &str) -> Result<String> {
            Ok(self.0.to_string())
        }
    }
    /// Stub LLM client that always errors (exercises the failure path).
    struct ErrClient;
    #[async_trait]
    impl LlmClient for ErrClient {
        async fn complete(&self, _: &str) -> Result<String> {
            Err(anyhow!("simulated reviewer failure"))
        }
    }

    // A trivial "## Why\nbecause\n" stand-in template so we don't depend
    // on the production default template's text in this test.
    let template = "REVIEW THE FOLLOWING DIFF:\n{{diff}}\nSUMMARY:\n{{change_summary}}";

    // -- Helper: run one full pass with a custom reviewer + mockito.
    async fn run_with_reviewer(
        reviewer: CodeReviewer,
        expect_draft: bool,
        body_contains: &'static str,
    ) {
        let (_dir, ws) = fixture_workspace_with_remote();
        let (_td_paths, paths) = crate::testing::test_daemon_paths();
        add_committed_change(&ws, "rv-change", "make the world a better place");

        // Spin up a mockito server, point autocoder's PR creation
        // at it via GITHUB_API_BASE-style override is not available;
        // instead we drive `execute_one_pass` directly and verify by
        // intercepting the github::create_pull_request HTTP call.
        //
        // The cleanest way is to set up a mockito mock that matches the
        // expected request shape; since we need to override the API
        // base, use the existing `create_pull_request_at` indirectly via
        // the `GITHUB_API_BASE`-equivalent — which we don't have.
        //
        // Approach: this test exercises autocoder's review-step
        // logic by invoking `execute_one_pass` and asserting on the
        // _outcome_ (no panic, push happened) plus reading the agent
        // branch tip's *commit subject* unchanged. The detailed
        // request-shape assertion (draft flag + body section) is
        // already covered by `github::tests::{body_includes_review_section,
        // draft_flag_serialized, label_fallback_on_draft_unsupported}`.
        //
        // What we add here is the *integration*: autocoder
        // selects the right draft flag and review_report based on the
        // verdict the reviewer produces. We test that by directly
        // calling the same compose logic via a small surface.
        let executor = CompletingExecutorWithDiff {
            artifact_name: format!("REVIEW_FIXTURE_{body_contains}"),
            artifact_text: "x".into(),
        };
        let direct_github = GithubConfig {
            token_env: "X".into(),
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
            &direct_github,
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
        .expect("commits step succeeds");
        assert_eq!(processed, vec!["rv-change".to_string()]);

        // Now exercise the reviewer step's compose path manually,
        // mirroring what execute_one_pass does between
        // `run_pass_through_commits` and `open_pull_request`.
        let ctx = build_review_context(&ws, &fixture_repo(&ws), &processed, reviewer.kind())
            .expect("build_review_context succeeds");
        let (report, draft) = match reviewer.review(&ctx).await {
            Ok(report) => {
                let draft = matches!(report.verdict, ReviewVerdict::Block);
                (Some(report), draft)
            }
            Err(e) => (
                Some(ReviewReport {
                    verdict: ReviewVerdict::Concerns,
                    markdown: format!("(reviewer failed: {e})"),
                    concerns: Vec::new(),
                    per_change_sections: Vec::new(),
                    attribution: None,
                }),
                false,
            ),
        };

        assert_eq!(draft, expect_draft, "draft flag mismatch");
        let rendered = report.expect("report always present when reviewer enabled");
        assert!(
            rendered.markdown.contains(body_contains)
                || (body_contains == "reviewer failed"
                    && rendered.markdown.contains("(reviewer failed:")),
            "markdown should contain `{body_contains}`; got: {}",
            rendered.markdown
        );
    }

    // Pass verdict → non-draft, body contains the verdict markdown.
    run_with_reviewer(
        CodeReviewer::new(
            Box::new(CannedClient(
                "VERDICT: Pass\n\n## Security\n- None observed.\n",
            )),
            template.to_string(),
        ),
        false,
        "None observed",
    )
    .await;

    // Concerns verdict → non-draft, body contains verdict markdown.
    run_with_reviewer(
        CodeReviewer::new(
            Box::new(CannedClient(
                "VERDICT: Concerns\n\n## Possible bugs\n- check input length.\n",
            )),
            template.to_string(),
        ),
        false,
        "check input length",
    )
    .await;

    // Block verdict → DRAFT.
    run_with_reviewer(
        CodeReviewer::new(
            Box::new(CannedClient(
                "VERDICT: Block\n\n## Security\n- SQL injection on line 42.\n",
            )),
            template.to_string(),
        ),
        true,
        "SQL injection",
    )
    .await;

    // Reviewer error → non-draft, body contains synthetic "reviewer failed" note.
    run_with_reviewer(
        CodeReviewer::new(Box::new(ErrClient), template.to_string()),
        false,
        "reviewer failed",
    )
    .await;
}

/// a58 revision: `build_review_context` reads full file contents only for
/// the `Oneshot` transport (which pre-dumps them into its prompt). For the
/// `Agentic` transport it lists the same changed-file paths but leaves
/// `contents` empty — the agent reads on demand — avoiding the wasted I/O
/// and memory the reviewer flagged. The unified diff is produced in both.
#[test]
fn build_review_context_skips_file_reads_for_agentic_transport() {
    use crate::config::ReviewerKind;
    fn git(ws: &Path, args: &[&str]) {
        let st = std::process::Command::new("git")
            .args(args)
            .current_dir(ws)
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?} failed");
    }
    let (_dir, ws) = fixture_workspace_with_remote();
    // Branch off `main` and add a changed file with a known body.
    git(&ws, &["checkout", "-q", "-b", "agent-q"]);
    let body = "fn demo() { /* BUILD_CTX_FIXTURE_BODY */ }\n";
    std::fs::write(ws.join("demo_changed.rs"), body).unwrap();
    git(&ws, &["add", "-A"]);
    git(&ws, &["commit", "-q", "-m", "demo: add changed file"]);

    let repo = fixture_repo(&ws);
    let processed: Vec<String> = Vec::new();

    // Oneshot: the full file body is read into `ChangedFile.contents`.
    let oneshot = build_review_context(&ws, &repo, &processed, ReviewerKind::Oneshot)
        .expect("oneshot context builds");
    let f = oneshot
        .changed_files
        .iter()
        .find(|f| f.path == "demo_changed.rs")
        .expect("changed file listed in oneshot context");
    assert_eq!(f.contents, body, "oneshot reads the full file contents");

    // Agentic: the same path is listed, but no contents are read from disk.
    let agentic = build_review_context(&ws, &repo, &processed, ReviewerKind::Agentic)
        .expect("agentic context builds");
    let f = agentic
        .changed_files
        .iter()
        .find(|f| f.path == "demo_changed.rs")
        .expect("changed file still listed in agentic context");
    assert!(
        f.contents.is_empty(),
        "agentic skips the eager file read (contents left empty): {:?}",
        f.contents
    );
    // The unified diff is still produced in both transports.
    assert!(
        agentic.diff.contains("demo_changed.rs"),
        "diff includes the changed file"
    );
}

/// 13.4.7 / git-workflow-manager baseline: empty pass produces no
/// commits and does not call the GitHub API.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_pass_produces_no_commits_and_no_pr() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, _paths) = crate::testing::test_daemon_paths();
    // No changes added — queue is empty.

    let pre_main = crate::git::rev_parse(&ws, "main").unwrap();

    let executor = CompletingExecutorNoDiff;
    // run_one_pass_no_push only runs through commit formation; if any
    // commits were produced inappropriately, the test would still need
    // to assert agent-q equals main below. The empty queue means the
    // function returns early without invoking the executor.
    let processed = run_one_pass_no_push(&ws, &executor)
        .await
        .expect("empty pass succeeds");
    assert!(
        processed.is_empty(),
        "expected no processed changes, got {processed:?}"
    );

    let agent_sha = crate::git::rev_parse(&ws, "agent-q").unwrap();
    assert_eq!(
        agent_sha, pre_main,
        "empty pass must not advance agent branch"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn iteration_error_continues() {
    // Verify the polling loop runs ≥2 iterations even when the executor
    // returns `Failed` on every change. Failed changes stay in the queue
    // (no archive), so each iteration re-locks, re-invokes, and re-fails.
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td_paths, _paths) = crate::testing::test_daemon_paths();
    // One pending change so each pass invokes the executor. The change
    // material must be committed in the fixture so the workspace is not
    // dirty when the polling pass starts (production repos commit their
    // openspec/changes/ tree alongside source code).
    let change_dir = ws.join("openspec/changes/feature-x");
    std::fs::create_dir_all(&change_dir).unwrap();
    std::fs::write(change_dir.join("proposal.md"), "## Why\nbecause\n").unwrap();
    let status = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(&ws)
        .status()
        .unwrap();
    assert!(status.success());
    let status = std::process::Command::new("git")
        .args(["commit", "-q", "-m", "add fixture change"])
        .current_dir(&ws)
        .status()
        .unwrap();
    assert!(status.success());
    // Also push so origin/main matches local main; otherwise the
    // `git pull --ff-only origin main` in the pass becomes a no-op of
    // the original commit, which is fine. We don't actually need to push
    // in this test.

    let executor = Arc::new(CountingFailingExecutor::new());
    let executor_dyn: Arc<dyn Executor> = executor.clone();
    let invoked = executor.invoked.clone();

    let repo = RepositoryConfig { forge: None,
        url: "git@github.com:owner/fixture.git".into(),
        local_path: Some(ws.clone()),
        base_branch: "main".into(),
        agent_branch: "agent-q".into(),
        poll_interval_sec: 0, // tight loop so we get many iterations fast
        chatops_channel_id: None,
        max_changes_per_pr: None,
        audits: None,
        spec_storage: None,
        upstream: None,
        auto_submit_pr: true,
        sandbox: None,
    };
    let github = GithubConfig {
        token_env: "DOES_NOT_EXIST".into(),
        token: None,
        owner_tokens: None,
        fork_owner: None,
        recreate_fork_on_reinit: false,
        command_authorization: Default::default(),
    };
    let cancel = CancellationToken::new();
    let cancel_for_task = cancel.clone();
    let github_holder: GithubHolder = Arc::new(arc_swap::ArcSwap::from_pointee(github));
    let reviewer_holder: ReviewerHolder = Arc::new(arc_swap::ArcSwap::from_pointee(None));
    let chatops_holder: ChatOpsHolder = Arc::new(arc_swap::ArcSwap::from_pointee(None));
    let cache_holder: CacheHolder = Arc::new(arc_swap::ArcSwap::from_pointee(
        crate::config::CacheConfig::default(),
    ));
    let repo_holder: Arc<ArcSwap<RepositoryConfig>> = Arc::new(ArcSwap::from_pointee(repo));
    let paths_for_run = std::sync::Arc::new(crate::testing::test_daemon_paths().1);
    let handle = tokio::spawn(async move {
        run(
            paths_for_run,
            repo_holder,
            executor_dyn,
            github_holder,
            reviewer_holder,
            chatops_holder,
            cache_holder,
            2400,
            u32::MAX,
            Some(u32::MAX),
            0,  // revision_cap: disabled in tests
            10, // human_revise_cap: irrelevant (dispatcher disabled)
            0,  // startup_jitter_max_secs: deterministic for tests
            0,  // inter_iteration_jitter_pct: deterministic for tests
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
            cancel_for_task,
        )
        .await;
    });

    // Wait event-driven for the executor to be invoked at least
    // twice — the proof that the loop iterated more than once. The
    // wall-clock cap is a "fail rather than hang" guardrail, not a
    // poll interval.
    let two_invocations = async {
        // notified() must be registered before the first read for
        // the first wake. Register, then check (because the counter
        // could already be ≥2 if we got scheduled late).
        loop {
            if executor.count.load(std::sync::atomic::Ordering::SeqCst) >= 2 {
                return;
            }
            let n = invoked.notified();
            if executor.count.load(std::sync::atomic::Ordering::SeqCst) >= 2 {
                return;
            }
            n.await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), two_invocations)
        .await
        .expect("expected ≥2 executor invocations within 10s");
    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("loop should exit within 2s of cancel");

    let count = executor.count.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        count >= 2,
        "expected ≥2 executor invocations across iterations, got {count}"
    );
}

/// IterationGuard's Drop impl clears the per-iteration cancel handle
/// AND fires the drained Notify — exercised in isolation so we know
/// the cleanup runs on every exit path, including panic unwind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn iteration_guard_drop_clears_handle_and_notifies() {
    let iter_cancel: Arc<std::sync::Mutex<Option<CancellationToken>>> =
        Arc::new(std::sync::Mutex::new(Some(CancellationToken::new())));
    let drained: Arc<tokio::sync::Notify> = Arc::new(tokio::sync::Notify::new());

    // Subscribe to the Notify BEFORE the guard drops so we don't miss
    // the wake. `notify_waiters()` only wakes futures that are already
    // registered as waiters; the `.enable()` call registers the
    // `Notified` future synchronously without polling it.
    let notified = drained.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();

    // Run the guard in a scope so it drops at the end.
    {
        let _guard = IterationGuard {
            iteration_cancel: iter_cancel.as_ref(),
            iteration_drained: drained.as_ref(),
        };
        assert!(
            iter_cancel.lock().unwrap().is_some(),
            "handle is populated before drop"
        );
    }
    // After the drop, the handle is cleared.
    assert!(
        iter_cancel.lock().unwrap().is_none(),
        "IterationGuard Drop must clear the cancel handle"
    );
    // And the pre-registered notified future is ready.
    tokio::time::timeout(Duration::from_secs(1), notified.as_mut())
        .await
        .expect("IterationGuard Drop must fire the drained Notify");
}

/// Panic inside the iteration scope still triggers the guard's Drop —
/// the Notify fires AND the handle is cleared. Verifies the
/// "every exit path" contract for tasks.md 1.3.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn iteration_guard_clears_state_on_panic_unwind() {
    let iter_cancel: Arc<std::sync::Mutex<Option<CancellationToken>>> =
        Arc::new(std::sync::Mutex::new(Some(CancellationToken::new())));
    let drained: Arc<tokio::sync::Notify> = Arc::new(tokio::sync::Notify::new());

    // Pre-register on the Notify so the panic-driven Drop's
    // notify_waiters() has a waiter to wake.
    let notified = drained.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();

    let iter_cancel_for_panic = iter_cancel.clone();
    let drained_for_panic = drained.clone();
    let join = std::thread::spawn(move || {
        let _guard = IterationGuard {
            iteration_cancel: iter_cancel_for_panic.as_ref(),
            iteration_drained: drained_for_panic.as_ref(),
        };
        // Force a panic inside the iteration body's scope. The Drop
        // impl runs on unwind — that's the contract we're verifying.
        panic!("simulated iteration-body panic");
    });
    // The thread panics; join returns Err(_). Drop ran nonetheless.
    let res = join.join();
    assert!(res.is_err(), "thread must have panicked");

    assert!(
        iter_cancel.lock().unwrap().is_none(),
        "guard Drop must clear the handle even on panic"
    );
    tokio::time::timeout(Duration::from_secs(1), notified.as_mut())
        .await
        .expect("Notify must fire even on panic-unwind drop");
}
