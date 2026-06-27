//! Unit tests for the control socket. Relocated verbatim from the former
//! inline `#[cfg(test)] mod tests` block in `control_socket.rs` into this
//! sibling test module when the handlers were split into submodules; `super`
//! still resolves to the `control_socket` module root, so every `use super::*`
//! reference (types, plumbing, AND the re-exported handlers) resolves exactly
//! as before.
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    fn write_yaml(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join("config.yaml");
        std::fs::write(&p, body).unwrap();
        p
    }

    /// Test-only spawn closure that pretends to start a polling task.
    /// The "task" just parks on its cancellation token and removes its
    /// own map entry on exit, mirroring the production spawn helper's
    /// lifecycle without doing any real work. Lets the reload-handler
    /// tests inspect the task map without depending on real workspaces,
    /// executors, or filesystem state.
    fn fake_spawn(
        task_map: RepoTaskMap,
        task_map_changed: Arc<Notify>,
        parent_cancel: CancellationToken,
    ) -> SpawnRepoFn {
        Arc::new(move |repo: RepositoryConfig| {
            let url = repo.url.clone();
            let mut guard = task_map.lock().unwrap();
            if guard.contains_key(&url) {
                return SpawnOutcome::AlreadyPresent;
            }
            let child_cancel = parent_cancel.child_token();
            let config: Arc<ArcSwap<RepositoryConfig>> =
                Arc::new(ArcSwap::from_pointee(repo));
            let cancel_for_task = child_cancel.clone();
            let map_for_task = task_map.clone();
            let map_changed_for_task = task_map_changed.clone();
            let url_for_task = url.clone();
            // Identity sentinel: this task owns exactly the entry whose `config`
            // is this `Arc`; the removal below is guarded on it.
            let config_for_task = config.clone();
            let join: JoinHandle<()> = tokio::spawn(async move {
                cancel_for_task.cancelled().await;
                {
                    let mut g = map_for_task.lock().unwrap();
                    // Identity-guarded removal: remove ONLY if the entry under
                    // this URL is STILL the one this task created. If a test (or
                    // a respawn) replaced the entry under the same URL key, leave
                    // it — otherwise a cancelled task's deferred remove clobbers
                    // the fresh handle, which under parallel load surfaces as the
                    // "URL went missing → reported as added" flake.
                    if g.get(&url_for_task)
                        .is_some_and(|h| Arc::ptr_eq(&h.config, &config_for_task))
                    {
                        g.remove(&url_for_task);
                    }
                }
                map_changed_for_task.notify_waiters();
            });
            guard.insert(
                url,
                RepoTaskHandle {
                    cancel: child_cancel,
                    config,
                    join,
                    pending_rebuild: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    pending_triages: Arc::new(Mutex::new(Vec::new())),
                    pending_audit_runs: Arc::new(Mutex::new(Vec::new())),
                    pending_proposal_requests: Arc::new(Mutex::new(Vec::new())),
                    pending_changelog_requests: Arc::new(Mutex::new(Vec::new())),
                    pending_brownfield_requests:
                        Arc::new(Mutex::new(std::collections::VecDeque::new())),
                    pending_scout_requests:
                        Arc::new(Mutex::new(std::collections::VecDeque::new())),
                    pending_spec_it_requests:
                        Arc::new(Mutex::new(std::collections::VecDeque::new())),
                    pending_sync_upstream_requests:
                        Arc::new(Mutex::new(std::collections::VecDeque::new())),
                    pending_brownfield_survey_requests:
                        Arc::new(Mutex::new(std::collections::VecDeque::new())),
                    pending_brownfield_batch_requests:
                        Arc::new(Mutex::new(std::collections::VecDeque::new())),
                    pending_revision_requests: RevisionRequestQueues::new(),
                    iteration_cancel: Arc::new(Mutex::new(None)),
                    iteration_drained: Arc::new(Notify::new()),
                },
            );
            drop(guard);
            task_map_changed.notify_waiters();
            SpawnOutcome::Spawned
        })
    }

    /// Build a `ControlState` whose task map is seeded with a fake
    /// handle for every repository in `cfg`. The `cancel` token is the
    /// parent of every fake task's child token, so cancelling it tears
    /// down the whole fixture cleanly.
    fn seeded_state(
        config_path: PathBuf,
        cfg: &Config,
        cancel: CancellationToken,
    ) -> ControlState {
        let task_map: RepoTaskMap = Arc::new(Mutex::new(HashMap::new()));
        let task_map_changed: Arc<Notify> = Arc::new(Notify::new());
        let spawn = fake_spawn(task_map.clone(), task_map_changed.clone(), cancel);
        for repo in &cfg.repositories {
            let _ = (spawn)(repo.clone());
        }
        let (_td_paths, paths) = crate::testing::test_daemon_paths();
        ControlState {
            github: Arc::new(ArcSwap::from_pointee(cfg.github.clone())),
            reviewer: Arc::new(ArcSwap::from_pointee(None)),
            chatops: Arc::new(ArcSwap::from_pointee(None)),
            cache: Arc::new(ArcSwap::from_pointee(cfg.cache.clone())),
            last_config: Arc::new(ArcSwap::from_pointee(cfg.clone())),
            config_path,
            repo_tasks: task_map,
            repo_tasks_changed: task_map_changed,
            spawn_repo: spawn,
            canonical_rag_registry: crate::rag::CanonicalRagRegistry::new(),
            outcome_store: crate::outcome_store::OutcomeStore::new(),
            submission_store: crate::submission_store::SubmissionStore::new(),
            paths: Arc::new(paths),
        }
    }

    #[test]
    fn socket_path_is_under_runtime_dir() {
        let (_td_paths, paths) = crate::testing::test_daemon_paths();
        let p = socket_path(&paths);
        let s = p.to_string_lossy().to_string();
        assert!(
            s.ends_with("control.sock"),
            "expected `control.sock` suffix: {s}"
        );
    }

    async fn send_request(socket: &Path, action_json: &str) -> serde_json::Value {
        let stream = tokio::net::UnixStream::connect(socket).await.unwrap();
        let (read_half, mut write_half) = stream.into_split();
        write_half.write_all(action_json.as_bytes()).await.unwrap();
        if !action_json.ends_with('\n') {
            write_half.write_all(b"\n").await.unwrap();
        }
        // The request is newline-terminated, so the server has the full line
        // and may respond AND close before we shut down our write half. Under
        // heavy parallel load that close-race surfaces as `ENOTCONN` here —
        // benign (the request was already sent), so ignore the shutdown result
        // rather than `.unwrap()`-panicking on it.
        let _ = write_half.shutdown().await;
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        serde_json::from_str(line.trim()).unwrap()
    }

    async fn fixture_listener(
        initial_yaml: &str,
    ) -> (TempDir, PathBuf, ControlState, PathBuf, CancellationToken) {
        let dir = TempDir::new().unwrap();
        let cfg_path = write_yaml(dir.path(), initial_yaml);
        let cfg = Config::load_from(&cfg_path).expect("fixture yaml parses");
        let cancel = CancellationToken::new();
        let state = seeded_state(cfg_path.clone(), &cfg, cancel.clone());
        let socket = dir.path().join("control.sock");
        // Bind synchronously so the test knows — without polling — that the
        // socket is ready to accept connections by the time fixture_listener
        // returns. Spawn only the accept loop.
        let listener = bind_at(&socket).expect("bind control socket");
        let listener_state = state.clone();
        let listener_socket = socket.clone();
        let listener_cancel = cancel.clone();
        tokio::spawn(async move {
            let _ = serve(listener, listener_socket, listener_state, listener_cancel).await;
        });
        (dir, socket, state, cfg_path, cancel)
    }

    /// Inline token in the github block so semantic validation
    /// (`validate_github_token_routes`) succeeds without depending on
    /// process env vars.
    const BASE_YAML: &str = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github:
  token_env: GITHUB_TOKEN
  token:
    value: "ghp_fixture"
"#;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reload_with_no_changes_responds_unchanged() {
        let (_dir, socket, _state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let resp = send_request(&socket, r#"{"action":"reload"}"#).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert!(
            resp["applied"].as_array().unwrap().is_empty(),
            "applied must be empty: {resp}"
        );
        assert!(
            resp["requires_restart"].as_array().unwrap().is_empty(),
            "requires_restart must be empty: {resp}"
        );
        let unchanged: Vec<String> = resp["unchanged"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        for section in ["github", "reviewer", "chatops", "cache", "repositories", "executor"] {
            assert!(
                unchanged.contains(&section.to_string()),
                "section `{section}` missing from unchanged: {unchanged:?}"
            );
        }
        cancel.cancel();
    }

    /// a03: the `revision_advise` action queues a [`RevisionAdviseRequest`]
    /// onto the matched repo's handle, carrying the operator's reply text for
    /// the read-only advisor session.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn revision_advise_action_queues_request() {
        let (_dir, socket, state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let resp = send_request(
            &socket,
            r#"{"action":"revision_advise","url":"git@github.com:owner/repo.git","change":"a03-x","channel":"C1","thread_ts":"9.9","reply_text":"align to canon?"}"#,
        )
        .await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        let q = {
            let guard = state.repo_tasks.lock().unwrap();
            guard
                .get("git@github.com:owner/repo.git")
                .unwrap()
                .pending_revision_requests
                .advise
                .clone()
        };
        let g = q.lock().unwrap();
        assert_eq!(g.len(), 1, "advise queue should have one entry");
        assert_eq!(g[0].change_slug, "a03-x");
        assert_eq!(g[0].reply_text, "align to canon?");
        assert_eq!(g[0].thread_ts, "9.9");
        cancel.cancel();
    }

    /// a03: the `revision_execute` action queues a [`RevisionExecuteRequest`]
    /// onto the matched repo's handle. De-duplicated on `change_slug` so a
    /// doubly-delivered `send it` enqueues only one run.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn revision_execute_action_queues_request_deduped() {
        let (_dir, socket, state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let req = r#"{"action":"revision_execute","url":"git@github.com:owner/repo.git","change":"a03-x","channel":"C1","thread_ts":"9.9"}"#;
        let resp = send_request(&socket, req).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        // A second identical request must not enqueue a duplicate.
        let resp2 = send_request(&socket, req).await;
        assert_eq!(resp2["ok"], serde_json::Value::Bool(true), "resp: {resp2}");
        let q = {
            let guard = state.repo_tasks.lock().unwrap();
            guard
                .get("git@github.com:owner/repo.git")
                .unwrap()
                .pending_revision_requests
                .execute
                .clone()
        };
        let g = q.lock().unwrap();
        assert_eq!(g.len(), 1, "execute queue should de-dupe on change_slug");
        assert_eq!(g[0].change_slug, "a03-x");
        cancel.cancel();
    }

    /// a03: a revision action for an unconfigured repo is refused (no live
    /// polling task to queue onto).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn revision_execute_unknown_repo_is_refused() {
        let (_dir, socket, _state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let resp = send_request(
            &socket,
            r#"{"action":"revision_execute","url":"git@github.com:owner/nope.git","change":"a03-x","channel":"C1","thread_ts":"9.9"}"#,
        )
        .await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        cancel.cancel();
    }

    // ---------------------------------------------------------------------
    // Enqueue-handler regression tests (shared `enqueue_request` helper).
    // These pin the per-action response JSON AND de-dup behaviour so the
    // behavior-preserving decomposition of the `handle_queue_*` handlers
    // stays byte-identical: the Vec disk-load path (proposal), a VecDeque
    // de-duped path (scout), the no-de-dup path with a `request_id`-less ack
    // (spec_it), AND the alternate-de-dup-field / alternate-ack path (batch).
    // ---------------------------------------------------------------------

    /// `queue_proposal_request` loads the on-disk `ProposalRequestState`, pushes
    /// one `ProposalRequest`, AND returns `{ok, url, request_id,
    /// poll_interval_sec}` — de-duplicating a doubly-delivered request_id.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queue_proposal_request_loads_state_enqueues_and_dedupes() {
        let (_dir, socket, state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let url = "git@github.com:owner/repo.git";
        let st = crate::proposal_requests::ProposalRequestState {
            request_id: "req-1".to_string(),
            repo_url: url.to_string(),
            channel: "C1".to_string(),
            thread_ts: "1.1".to_string(),
            ack_message_ts: "1.1".to_string(),
            operator_user: "U1".to_string(),
            request_text: "do a thing".to_string(),
            submitted_at: chrono::Utc::now(),
            status: crate::proposal_requests::ProposalRequestStatus::Pending,
            reason: None,
        };
        let root = crate::proposal_requests::default_state_root(&state.paths);
        crate::proposal_requests::write_state(&root, &st).unwrap();
        let req = r#"{"action":"queue_proposal_request","url":"git@github.com:owner/repo.git","request_id":"req-1"}"#;
        let resp = send_request(&socket, req).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(resp["url"], serde_json::json!(url), "resp: {resp}");
        assert_eq!(resp["request_id"], serde_json::json!("req-1"), "resp: {resp}");
        assert_eq!(resp["poll_interval_sec"], serde_json::json!(60), "resp: {resp}");
        // A second identical request must not enqueue a duplicate.
        let resp2 = send_request(&socket, req).await;
        assert_eq!(resp2["ok"], serde_json::Value::Bool(true), "resp: {resp2}");
        let q = {
            let guard = state.repo_tasks.lock().unwrap();
            guard.get(url).unwrap().pending_proposal_requests.clone()
        };
        let g = q.lock().unwrap();
        assert_eq!(g.len(), 1, "proposal queue should de-dupe on request_id");
        assert_eq!(g[0].request_id, "req-1");
        cancel.cancel();
    }

    /// `queue_proposal_request` with no on-disk state file returns the specific
    /// missing-state-file error AND enqueues nothing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queue_proposal_request_missing_state_is_error() {
        let (_dir, socket, state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let url = "git@github.com:owner/repo.git";
        let resp = send_request(
            &socket,
            r#"{"action":"queue_proposal_request","url":"git@github.com:owner/repo.git","request_id":"ghost"}"#,
        )
        .await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        assert!(
            resp["error"]
                .as_str()
                .unwrap()
                .contains("no proposal-request state file found"),
            "resp: {resp}"
        );
        let q = {
            let guard = state.repo_tasks.lock().unwrap();
            guard.get(url).unwrap().pending_proposal_requests.clone()
        };
        assert!(q.lock().unwrap().is_empty(), "nothing should be queued");
        cancel.cancel();
    }

    /// `queue_scout_request` enqueues one `ScoutRequest` onto the VecDeque queue
    /// AND de-duplicates on request_id.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queue_scout_request_enqueues_and_dedupes() {
        let (_dir, socket, state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let url = "git@github.com:owner/repo.git";
        let req = r#"{"action":"queue_scout_request","url":"git@github.com:owner/repo.git","request_id":"s-1","channel":"C1","thread_ts":"2.2","guidance":"look here"}"#;
        let resp = send_request(&socket, req).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(resp["request_id"], serde_json::json!("s-1"), "resp: {resp}");
        assert_eq!(resp["poll_interval_sec"], serde_json::json!(60), "resp: {resp}");
        let _ = send_request(&socket, req).await;
        let q = {
            let guard = state.repo_tasks.lock().unwrap();
            guard.get(url).unwrap().pending_scout_requests.clone()
        };
        let g = q.lock().unwrap();
        assert_eq!(g.len(), 1, "scout queue should de-dupe on request_id");
        assert_eq!(g[0].request_id, "s-1");
        assert_eq!(g[0].guidance.as_deref(), Some("look here"));
        cancel.cancel();
    }

    /// `queue_scout_request` for an unconfigured repo is refused before any
    /// enqueue.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queue_scout_request_unknown_repo_is_refused() {
        let (_dir, socket, _state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let resp = send_request(
            &socket,
            r#"{"action":"queue_scout_request","url":"git@github.com:owner/nope.git","request_id":"s-1","channel":"C1","thread_ts":"2.2"}"#,
        )
        .await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        cancel.cancel();
    }

    /// `queue_spec_it_request` is NOT de-duplicated — two selections enqueue two
    /// entries — AND its ack omits `request_id`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queue_spec_it_request_enqueues_without_dedup() {
        let (_dir, socket, state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let url = "git@github.com:owner/repo.git";
        let req = r#"{"action":"queue_spec_it_request","url":"git@github.com:owner/repo.git","scout_request_id":"s-1","item_id":2,"channel":"C1","thread_ts":"3.3"}"#;
        let resp = send_request(&socket, req).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(resp["url"], serde_json::json!(url), "resp: {resp}");
        assert!(
            resp.get("request_id").is_none(),
            "spec_it ack omits request_id: {resp}"
        );
        assert_eq!(resp["poll_interval_sec"], serde_json::json!(60), "resp: {resp}");
        let _ = send_request(&socket, req).await;
        let q = {
            let guard = state.repo_tasks.lock().unwrap();
            guard.get(url).unwrap().pending_spec_it_requests.clone()
        };
        let g = q.lock().unwrap();
        assert_eq!(g.len(), 2, "spec_it requests are not de-duplicated");
        assert_eq!(g[0].item_id, 2);
        assert_eq!(g[0].scout_request_id, "s-1");
        cancel.cancel();
    }

    /// `queue_brownfield_batch_request` enqueues one `BrownfieldBatchRequest`,
    /// de-dupes on `survey_request_id`, AND echoes `survey_request_id` (not
    /// `request_id`) in its ack.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queue_brownfield_batch_request_enqueues_and_dedupes() {
        let (_dir, socket, state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let url = "git@github.com:owner/repo.git";
        let req = r#"{"action":"queue_brownfield_batch_request","url":"git@github.com:owner/repo.git","survey_request_id":"sv-1","channel":"C1","thread_ts":"4.4"}"#;
        let resp = send_request(&socket, req).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(
            resp["survey_request_id"],
            serde_json::json!("sv-1"),
            "resp: {resp}"
        );
        assert_eq!(resp["poll_interval_sec"], serde_json::json!(60), "resp: {resp}");
        let _ = send_request(&socket, req).await;
        let q = {
            let guard = state.repo_tasks.lock().unwrap();
            guard
                .get(url)
                .unwrap()
                .pending_brownfield_batch_requests
                .clone()
        };
        let g = q.lock().unwrap();
        assert_eq!(g.len(), 1, "batch queue should de-dupe on survey_request_id");
        assert_eq!(g[0].survey_request_id, "sv-1");
        cancel.cancel();
    }

    /// a65: a reload that adds (or changes) `cache.workspaces_max_gb`
    /// hot-applies the new cap into the shared `cache` holder so polling
    /// tasks pick it up at their next iteration.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reload_applies_cache_cap_change() {
        let (_dir, socket, state, cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        // Initially unbounded.
        assert!(state.cache.load().workspaces_max_gb.is_none());
        let new_yaml = format!("{BASE_YAML}cache:\n  workspaces_max_gb: 25\n");
        std::fs::write(&cfg_path, new_yaml).unwrap();
        let resp = send_request(&socket, r#"{"action":"reload"}"#).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        let applied: Vec<String> = resp["applied"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(
            applied.contains(&"cache".to_string()),
            "cache must be in applied: {applied:?}"
        );
        // The shared holder now carries the new cap.
        assert_eq!(state.cache.load().workspaces_max_gb, Some(25));
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reload_applies_github_changes() {
        let (_dir, socket, state, cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let new_yaml = BASE_YAML.replace("token_env: GITHUB_TOKEN", "token_env: NEW_TOKEN_VAR");
        std::fs::write(&cfg_path, new_yaml).unwrap();
        let resp = send_request(&socket, r#"{"action":"reload"}"#).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        let applied: Vec<String> = resp["applied"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(
            applied.contains(&"github".to_string()),
            "github must be in applied: {applied:?}"
        );
        let now = state.github.load_full();
        assert_eq!(now.token_env, "NEW_TOKEN_VAR");
        cancel.cancel();
    }

    /// Mode-toggle reload: writing a config that flips
    /// `reviewer.mode` from bundled to per_change rebuilds the live
    /// reviewer slot. The seeded test fixture starts with an empty
    /// reviewer slot (the daemon initializes it at startup outside
    /// `handle_reload`), so the assertion is that the reload-driven
    /// hot-swap populates the slot with the new mode + budget.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reload_applies_reviewer_mode_change() {
        let base_with_reviewer = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github:
  token_env: GITHUB_TOKEN
  token:
    value: "ghp_fixture"
reviewer:
  enabled: true
  provider: anthropic
  model: claude-sonnet-4-6
  api_key:
    value: "sk-fixture"
"#;
        let (_dir, socket, state, cfg_path, cancel) = fixture_listener(base_with_reviewer).await;
        // Operator edits the config to flip mode + raise budget.
        let new_yaml = base_with_reviewer.replace(
            "  api_key:\n    value: \"sk-fixture\"\n",
            "  api_key:\n    value: \"sk-fixture\"\n  mode: per_change\n  prompt_budget_chars: 4000000\n",
        );
        std::fs::write(&cfg_path, new_yaml).unwrap();
        let resp = send_request(&socket, r#"{"action":"reload"}"#).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        let applied: Vec<String> = resp["applied"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(
            applied.contains(&"reviewer".to_string()),
            "reviewer must be in applied: {applied:?}"
        );
        // The hot-swapped reviewer slot sees the new mode + budget.
        {
            let r = state.reviewer.load_full();
            let inner = r
                .as_ref()
                .as_ref()
                .expect("reviewer slot populated by reload");
            assert_eq!(inner.mode(), crate::config::ReviewerMode::PerChange);
            assert_eq!(inner.prompt_budget(), 4_000_000);
        }
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reload_reports_requires_restart_for_executor_change() {
        let (_dir, socket, state, cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let new_yaml = BASE_YAML.replace(
            "executor:\n  kind: claude_cli",
            "executor:\n  kind: claude_cli\n  timeout_secs: 600",
        );
        std::fs::write(&cfg_path, new_yaml).unwrap();
        let resp = send_request(&socket, r#"{"action":"reload"}"#).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        let requires_restart: Vec<String> = resp["requires_restart"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(
            requires_restart.contains(&"executor".to_string()),
            "executor must be in requires_restart: {requires_restart:?}"
        );
        // last_config now reflects the new timeout, but the in-memory
        // executor shared with polling tasks is NOT swapped.
        let snap = state.last_config.load_full();
        assert_eq!(snap.executor.timeout_secs, 600);
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reload_rejected_on_invalid_yaml() {
        let (_dir, socket, _state, cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        std::fs::write(&cfg_path, "::: not [valid: yaml [[ {{").unwrap();
        let resp = send_request(&socket, r#"{"action":"reload"}"#).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        let err = resp["error"].as_str().unwrap();
        assert!(
            err.to_lowercase().contains("parsing")
                || err.to_lowercase().contains("yaml")
                || err.to_lowercase().contains("expected")
                || err.to_lowercase().contains("did not find"),
            "error must hint at parse failure: {err}"
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reload_rejected_on_validation_failure() {
        let (dir, socket, _state, cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let collision_path = dir.path().join("shared-ws");
        let new_yaml = format!(
            r#"
repositories:
  - url: "git@github.com:owner/repo-a.git"
    local_path: "{shared}"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
  - url: "git@github.com:owner/repo-b.git"
    local_path: "{shared}"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github:
  token_env: GITHUB_TOKEN
"#,
            shared = collision_path.display(),
        );
        std::fs::write(&cfg_path, new_yaml).unwrap();
        let resp = send_request(&socket, r#"{"action":"reload"}"#).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        let err = resp["error"].as_str().unwrap();
        assert!(
            err.contains("collision") || err.contains("resolve to"),
            "error must name workspace collision: {err}"
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unknown_action_returns_error() {
        let (_dir, socket, _state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let resp = send_request(&socket, r#"{"action":"nonsense"}"#).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        let err = resp["error"].as_str().unwrap();
        assert!(err.contains("nonsense"), "error must name action: {err}");
        assert!(
            err.to_lowercase().contains("unknown"),
            "error must say `unknown`: {err}"
        );
        cancel.cancel();
    }

    /// a59: the `review_target` action is wired AND validates its target —
    /// a malformed PR number is rejected with ok:false before any review runs.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn review_target_rejects_malformed_target() {
        let (_dir, socket, _state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let resp = send_request(
            &socket,
            r#"{"action":"review_target","url":"git@github.com:owner/repo.git","target":["pr","notanumber"]}"#,
        )
        .await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        let err = resp["error"].as_str().unwrap();
        assert!(err.contains("not a valid PR number"), "error: {err}");
        cancel.cancel();
    }

    /// a59: an unknown repo substring is rejected with ok:false (the
    /// substring selector resolves zero repos).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn review_target_unknown_repo_errors() {
        let (_dir, socket, _state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let resp = send_request(
            &socket,
            r#"{"action":"review_target","url":"nope-no-such-repo","target":["pr","1"]}"#,
        )
        .await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        let err = resp["error"].as_str().unwrap();
        assert!(err.contains("no configured repository"), "error: {err}");
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dispatch_returns_error_on_unparseable_json() {
        let (_dir, socket, _state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let resp = send_request(&socket, "not-json").await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        let err = resp["error"].as_str().unwrap();
        assert!(
            err.contains("malformed JSON"),
            "error must contain `malformed JSON`: {err}"
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dispatch_returns_error_when_action_field_missing() {
        let (_dir, socket, _state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        for body in ["{}", r#"{"unrelated":"x"}"#] {
            let resp = send_request(&socket, body).await;
            assert_eq!(
                resp["ok"],
                serde_json::Value::Bool(false),
                "resp for {body}: {resp}"
            );
            let err = resp["error"].as_str().unwrap();
            assert!(
                err.contains("missing"),
                "error must contain `missing` for body {body}: {err}"
            );
            assert!(
                err.contains("action"),
                "error must contain `action` for body {body}: {err}"
            );
            assert!(
                !err.contains("malformed JSON"),
                "missing-action error must be distinguishable from `malformed JSON` for body {body}: {err}"
            );
        }
        cancel.cancel();
    }

    /// Helper: copy the task map's current URLs into a sorted Vec for
    /// stable assertions.
    fn task_map_urls(state: &ControlState) -> Vec<String> {
        let guard = state.repo_tasks.lock().unwrap();
        let mut urls: Vec<String> = guard.keys().cloned().collect();
        urls.sort();
        urls
    }

    /// Wait up to `timeout_ms` for `pred` to return true, driven by
    /// `notify`. The caller passes a `Notify` that fires whenever the
    /// underlying state changes; this function only re-evaluates `pred`
    /// in response to a notify, so the test stays event-driven instead of
    /// sleep-polling. The `timeout` is a hard wall-clock cap (the
    /// legitimate "I'd rather fail than hang" use of a timer), not a poll
    /// interval.
    async fn wait_for(
        timeout_ms: u64,
        notify: Arc<Notify>,
        mut pred: impl FnMut() -> bool,
    ) -> bool {
        // Fast path — predicate is already true.
        if pred() {
            return true;
        }
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .unwrap_or_default();
            if remaining.is_zero() {
                return pred();
            }
            // Register interest BEFORE evaluating the predicate so a notify
            // racing with the check is not lost.
            let notified = notify.notified();
            if pred() {
                return true;
            }
            if tokio::time::timeout(remaining, notified).await.is_err() {
                return pred();
            }
        }
    }

    fn delta_urls(resp: &serde_json::Value, key: &str) -> Vec<String> {
        resp["repositories_delta"][key]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn applied_list(resp: &serde_json::Value) -> Vec<String> {
        resp["applied"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn unchanged_list(resp: &serde_json::Value) -> Vec<String> {
        resp["unchanged"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reload_adds_repository_spawns_task() {
        let (_dir, socket, state, cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let new_yaml = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
  - url: "git@github.com:owner/repo-added.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github:
  token_env: GITHUB_TOKEN
  token:
    value: "ghp_fixture"
"#;
        std::fs::write(&cfg_path, new_yaml).unwrap();
        let resp = send_request(&socket, r#"{"action":"reload"}"#).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        let applied = applied_list(&resp);
        assert!(
            applied.contains(&"repositories".to_string()),
            "`repositories` must be in applied: {applied:?}"
        );
        let added = delta_urls(&resp, "added");
        assert_eq!(
            added,
            vec!["git@github.com:owner/repo-added.git".to_string()]
        );
        assert!(
            delta_urls(&resp, "removed").is_empty(),
            "removed must be empty: {resp}"
        );
        assert!(
            delta_urls(&resp, "changed").is_empty(),
            "changed must be empty: {resp}"
        );
        // The new URL must be present in the task map.
        let urls = task_map_urls(&state);
        assert!(
            urls.contains(&"git@github.com:owner/repo-added.git".to_string()),
            "task map must contain added URL: {urls:?}"
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reload_removes_repository_cancels_task() {
        // Start with two repos.
        let two_repo_yaml = r#"
repositories:
  - url: "git@github.com:owner/repo-a.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
  - url: "git@github.com:owner/repo-b.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github:
  token_env: GITHUB_TOKEN
  token:
    value: "ghp_fixture"
"#;
        let (_dir, socket, state, cfg_path, cancel) = fixture_listener(two_repo_yaml).await;
        // Drop repo-b.
        let new_yaml = r#"
repositories:
  - url: "git@github.com:owner/repo-a.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github:
  token_env: GITHUB_TOKEN
  token:
    value: "ghp_fixture"
"#;
        std::fs::write(&cfg_path, new_yaml).unwrap();
        let resp = send_request(&socket, r#"{"action":"reload"}"#).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        let removed = delta_urls(&resp, "removed");
        assert_eq!(
            removed,
            vec!["git@github.com:owner/repo-b.git".to_string()],
            "removed must be exactly repo-b: {resp}"
        );
        // The fake task is parked on its own child token; cancelling
        // makes it exit and remove its map entry. The fixture fires
        // `repo_tasks_changed` on every map mutation, so we can wait
        // event-driven instead of sleep-polling.
        let state_ref = state.clone();
        let observed = wait_for(1000, state.repo_tasks_changed.clone(), move || {
            !state_ref
                .repo_tasks
                .lock()
                .unwrap()
                .contains_key("git@github.com:owner/repo-b.git")
        })
        .await;
        assert!(
            observed,
            "removed URL's task should have exited and removed its map entry within 1s"
        );
        // repo-a still present.
        let urls = task_map_urls(&state);
        assert_eq!(urls, vec!["git@github.com:owner/repo-a.git".to_string()]);
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reload_changes_repository_settings_in_place() {
        let (_dir, socket, state, cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        // Change base_branch from main → dev. URL unchanged.
        let new_yaml = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: dev
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github:
  token_env: GITHUB_TOKEN
  token:
    value: "ghp_fixture"
"#;
        std::fs::write(&cfg_path, new_yaml).unwrap();
        let resp = send_request(&socket, r#"{"action":"reload"}"#).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        let changed = delta_urls(&resp, "changed");
        assert_eq!(
            changed,
            vec!["git@github.com:owner/repo.git".to_string()],
            "changed must contain URL: {resp}"
        );
        assert!(
            delta_urls(&resp, "added").is_empty(),
            "added must be empty: {resp}"
        );
        assert!(
            delta_urls(&resp, "removed").is_empty(),
            "removed must be empty: {resp}"
        );
        // Verify the swap holder now contains base_branch = dev.
        let url = "git@github.com:owner/repo.git";
        let guard = state.repo_tasks.lock().unwrap();
        let handle = guard.get(url).expect("URL still present");
        let snapshot = handle.config.load();
        assert_eq!(snapshot.base_branch, "dev");
        drop(guard);
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reload_repo_url_change_is_remove_plus_add() {
        let (_dir, socket, _state, cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        // Swap URL X for URL Y.
        let new_yaml = r#"
repositories:
  - url: "git@github.com:owner/repo-new-url.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github:
  token_env: GITHUB_TOKEN
  token:
    value: "ghp_fixture"
"#;
        std::fs::write(&cfg_path, new_yaml).unwrap();
        let resp = send_request(&socket, r#"{"action":"reload"}"#).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        let added = delta_urls(&resp, "added");
        let removed = delta_urls(&resp, "removed");
        assert_eq!(
            added,
            vec!["git@github.com:owner/repo-new-url.git".to_string()]
        );
        assert_eq!(
            removed,
            vec!["git@github.com:owner/repo.git".to_string()]
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reload_executor_change_still_requires_restart() {
        let (_dir, socket, state, cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let new_yaml = BASE_YAML.replace(
            "executor:\n  kind: claude_cli",
            "executor:\n  kind: claude_cli\n  timeout_secs: 600",
        );
        std::fs::write(&cfg_path, new_yaml).unwrap();
        let resp = send_request(&socket, r#"{"action":"reload"}"#).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        let requires_restart: Vec<String> = resp["requires_restart"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(
            requires_restart.contains(&"executor".to_string()),
            "executor must be in requires_restart: {requires_restart:?}"
        );
        // Repositories section unchanged AND it is NOT in requires_restart.
        assert!(
            !requires_restart.contains(&"repositories".to_string()),
            "`repositories` must no longer be in requires_restart after \
             hot-reload-repositories-list lands: {requires_restart:?}"
        );
        let unchanged = unchanged_list(&resp);
        assert!(
            unchanged.contains(&"repositories".to_string()),
            "repositories must be in unchanged since the YAML edit only touched executor: {unchanged:?}"
        );
        let snap = state.last_config.load_full();
        assert_eq!(snap.executor.timeout_secs, 600);
        cancel.cancel();
    }

    /// Build YAML for a workspace at an explicit `local_path` so the
    /// operator-command tests don't try to look under /tmp/workspaces.
    fn local_path_yaml(local_path: &Path) -> String {
        format!(
            r#"
repositories:
  - url: "git@github.com:owner/myrepo.git"
    local_path: "{}"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github:
  token_env: GITHUB_TOKEN
  token:
    value: "ghp_fixture"
"#,
            local_path.display()
        )
    }

    /// Create a workspace fixture with an openspec/changes/<name>/proposal.md
    /// file so `queue::list_pending` includes it.
    fn make_change(workspace: &Path, change: &str) {
        let dir = workspace.join("openspec/changes").join(change);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("proposal.md"), "## Why\nfixture\n").unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_perma_stuck_removes_marker_and_returns_ok() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        make_change(&workspace, "a06-foo");
        std::fs::write(
            workspace.join("openspec/changes/a06-foo/.perma-stuck.json"),
            r#"{"change":"a06-foo","consecutive_failures":2,"last_reason":"x","marked_stuck_at":"2026-01-01T00:00:00Z","operator_action":"x"}"#,
        )
        .unwrap();
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "clear_perma_stuck_marker",
            "url": "git@github.com:owner/myrepo.git",
            "change": "a06-foo",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert!(
            !workspace
                .join("openspec/changes/a06-foo/.perma-stuck.json")
                .exists(),
            "marker file should be gone"
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_perma_stuck_errors_when_marker_absent() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        make_change(&workspace, "a06-foo");
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "clear_perma_stuck_marker",
            "url": "git@github.com:owner/myrepo.git",
            "change": "a06-foo",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        let err = resp["error"].as_str().unwrap();
        // a40: prefix resolution intercepts the absent-marker case at the
        // resolver layer (the change dir exists but carries no scope marker
        // → NoMatch). The error names the marker file explicitly.
        assert!(
            err.contains(".perma-stuck.json"),
            "error must name marker file: {err}"
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_revision_removes_marker_and_returns_ok() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        make_change(&workspace, "a07-bar");
        std::fs::write(
            workspace.join("openspec/changes/a07-bar/.needs-spec-revision.json"),
            r#"{"change":"a07-bar","marked_at":"2026-01-01T00:00:00Z","unimplementable_tasks":[],"revision_suggestion":"x","operator_action":"x"}"#,
        )
        .unwrap();
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "clear_revision_marker",
            "url": "git@github.com:owner/myrepo.git",
            "change": "a07-bar",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert!(
            !workspace
                .join("openspec/changes/a07-bar/.needs-spec-revision.json")
                .exists()
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_revision_errors_when_marker_absent() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        make_change(&workspace, "a07-bar");
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "clear_revision_marker",
            "url": "git@github.com:owner/myrepo.git",
            "change": "a07-bar",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        cancel.cancel();
    }

    // -------------------------------------------------------------
    // bulk-clear-markers: wildcard sweep at the control-socket handler.
    // -------------------------------------------------------------

    /// Build YAML for two repositories at explicit `local_path`s so the
    /// fleet-wide sweep tests have a real multi-repo config to enumerate.
    fn two_repo_yaml(ws_a: &Path, ws_b: &Path) -> String {
        format!(
            r#"
repositories:
  - url: "git@github.com:owner/repo-a.git"
    local_path: "{}"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
  - url: "git@github.com:owner/repo-b.git"
    local_path: "{}"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github:
  token_env: GITHUB_TOKEN
  token:
    value: "ghp_fixture"
"#,
            ws_a.display(),
            ws_b.display(),
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_perma_stuck_wildcard_clears_all_in_one_repo() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        make_change(&workspace, "a06-foo");
        make_change(&workspace, "a07-bar");
        make_change(&workspace, "a08-clean");
        write_marker_file(&workspace, "a06-foo", ".perma-stuck.json");
        write_marker_file(&workspace, "a07-bar", ".perma-stuck.json");
        // a06 also carries a companion ignore-for-queue marker — the sweep
        // must remove it too (matching the exact-form behavior).
        write_marker_file(&workspace, "a06-foo", ".ignore-for-queue.json");
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "clear_perma_stuck_marker",
            "url": "git@github.com:owner/myrepo.git",
            "change": "*",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        let results = resp["results"].as_array().expect("results array");
        assert_eq!(results.len(), 1, "one repo in the config");
        let cleared: Vec<&str> = results[0]["cleared"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(cleared, vec!["a06-foo", "a07-bar"]);
        assert_eq!(
            results[0]["removed_ignore_for_queue"],
            serde_json::Value::Bool(true)
        );
        // Both markers AND the companion ignore-for-queue are gone.
        assert!(
            !workspace
                .join("openspec/changes/a06-foo/.perma-stuck.json")
                .exists()
        );
        assert!(
            !workspace
                .join("openspec/changes/a06-foo/.ignore-for-queue.json")
                .exists()
        );
        assert!(
            !workspace
                .join("openspec/changes/a07-bar/.perma-stuck.json")
                .exists()
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_perma_stuck_wildcard_empty_repo_reports_nothing_to_clear() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        make_change(&workspace, "a08-clean");
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "clear_perma_stuck_marker",
            "url": "git@github.com:owner/myrepo.git",
            "change": "*",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        // Fail-loud, never silent: ok=true with an explicit empty `cleared`
        // (the chatops formatter renders this as "nothing to clear").
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        let results = resp["results"].as_array().expect("results array");
        assert_eq!(results.len(), 1);
        assert!(results[0]["cleared"].as_array().unwrap().is_empty());
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_revision_fleet_wildcard_sweeps_every_repo() {
        let dir = TempDir::new().unwrap();
        let ws_a = dir.path().join("ws-a");
        let ws_b = dir.path().join("ws-b");
        std::fs::create_dir_all(&ws_a).unwrap();
        std::fs::create_dir_all(&ws_b).unwrap();
        make_change(&ws_a, "a06-foo");
        write_marker_file(&ws_a, "a06-foo", ".needs-spec-revision.json");
        make_change(&ws_b, "a07-bar");
        write_marker_file(&ws_b, "a07-bar", ".needs-spec-revision.json");
        make_change(&ws_b, "a08-baz");
        write_marker_file(&ws_b, "a08-baz", ".needs-spec-revision.json");
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&two_repo_yaml(&ws_a, &ws_b)).await;
        let req = serde_json::json!({
            "action": "clear_revision_marker",
            "url": "*",
            "change": "*",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        let results = resp["results"].as_array().expect("results array");
        assert_eq!(results.len(), 2, "both repos enumerated: {resp}");
        // Every marker across both repos is removed.
        assert!(
            !ws_a
                .join("openspec/changes/a06-foo/.needs-spec-revision.json")
                .exists()
        );
        assert!(
            !ws_b
                .join("openspec/changes/a07-bar/.needs-spec-revision.json")
                .exists()
        );
        assert!(
            !ws_b
                .join("openspec/changes/a08-baz/.needs-spec-revision.json")
                .exists()
        );
        cancel.cancel();
    }

    /// 5.5 (load-bearing): a `*` target must NOT be passed to
    /// `resolve_change_prefix`. We assert the wildcard branch is taken
    /// *before* resolution by constructing a workspace where prefix
    /// resolution of a literal `*` would return `NoMatch` (no change dir is
    /// named `*`, and `*` is not a prefix of any marked change), then
    /// confirming the sweep STILL clears the marked change. If the handler
    /// routed `*` through `resolve_change_prefix`, it would NoMatch and
    /// clear nothing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_perma_stuck_wildcard_bypasses_resolve_change_prefix() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        make_change(&workspace, "a06-foo");
        write_marker_file(&workspace, "a06-foo", ".perma-stuck.json");

        // Pre-condition: prove that resolving a literal `*` against this
        // workspace is a NoMatch. If the handler called this, the sweep
        // would clear nothing.
        let resolved = queue::resolve_change_prefix(
            &workspace,
            "*",
            queue::ChangePrefixMarkerScope::PermaStuck,
        );
        assert!(
            matches!(
                resolved,
                Err(queue::ResolvePrefixError::NoMatch { .. })
            ),
            "literal '*' must NoMatch through the resolver: {resolved:?}"
        );

        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "clear_perma_stuck_marker",
            "url": "git@github.com:owner/myrepo.git",
            "change": "*",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        let cleared: Vec<&str> = resp["results"][0]["cleared"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        // The sweep succeeded DESPITE the resolver returning NoMatch for `*`
        // — proving the wildcard branch is taken before resolution.
        assert_eq!(cleared, vec!["a06-foo"]);
        assert!(
            !workspace
                .join("openspec/changes/a06-foo/.perma-stuck.json")
                .exists()
        );
        cancel.cancel();
    }

    /// Seed a posted, public-origin issue candidate under the daemon state
    /// root so the `promote_issue_candidate` handler can act on it.
    fn seed_posted_candidate(
        state_root: &Path,
        url: &str,
        number: u64,
        slug: &str,
    ) -> String {
        let id = crate::lanes::ingestion::candidate_id(url, number);
        let candidate = crate::lanes::ingestion::CandidateState {
            id: id.clone(),
            repo_url: url.to_string(),
            source_issue: number,
            slug: slug.to_string(),
            origin: crate::lanes::ingestion::IssueOrigin::Public,
            issue_md: "## Report\nmaintainer-approved diagnosis\n".to_string(),
            tasks_md: "- [ ] 1.1 fix the code to conform to the spec\n".to_string(),
            report_body: "raw public reporter body {{x}}".to_string(),
            posted_at: chrono::Utc::now(),
            status: crate::lanes::ingestion::CandidateStatus::Posted,
            thread_ts: Some("1755.cand".to_string()),
            channel: Some("C_OPS".to_string()),
        };
        crate::lanes::ingestion::write_candidate(state_root, &candidate).unwrap();
        id
    }

    /// 5.3: the `promote_issue_candidate` handler writes `issues/<slug>/`
    /// (public origin includes `report-body.md`), flips the candidate to
    /// promoted, is idempotent on a second invocation, AND errors on a
    /// missing candidate.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn promote_issue_candidate_writes_queues_and_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let (_dir, socket, state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;

        let url = "git@github.com:owner/myrepo.git";
        let id = seed_posted_candidate(&state.paths.state, url, 7, "drop-newline");

        let req = serde_json::json!({
            "action": "promote_issue_candidate",
            "url": url,
            "candidate_id": id,
            "channel": "C_OPS",
            "thread_ts": "1755.cand",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(resp["slug"], "drop-newline");

        // The unit is written under issues/<slug>/ — its bot-authored files
        // plus the quarantined report-body for a public-origin candidate.
        let issue_dir = workspace.join("issues/drop-newline");
        assert!(issue_dir.join("issue.md").exists(), "issue.md written");
        assert!(issue_dir.join("tasks.md").exists(), "tasks.md written");
        assert!(
            issue_dir.join("report-body.md").exists(),
            "public-origin report-body.md quarantined"
        );

        // The candidate is flipped to Promoted.
        let after = crate::lanes::ingestion::read_candidate(&state.paths.state, &id)
            .unwrap()
            .unwrap();
        assert_eq!(
            after.status,
            crate::lanes::ingestion::CandidateStatus::Promoted
        );

        // Second invocation is idempotent: reports already_promoted, no
        // re-write (and no already-exists error).
        let resp2 = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp2["ok"], serde_json::Value::Bool(true), "resp2: {resp2}");
        assert_eq!(resp2["already_promoted"], serde_json::Value::Bool(true));
        assert_eq!(resp2["slug"], "drop-newline");

        // A missing candidate returns an error.
        let missing = serde_json::json!({
            "action": "promote_issue_candidate",
            "url": url,
            "candidate_id": "owner-myrepo-999",
            "channel": "C_OPS",
            "thread_ts": "1755.none",
        });
        let resp3 = send_request(&socket, &missing.to_string()).await;
        assert_eq!(resp3["ok"], serde_json::Value::Bool(false), "resp3: {resp3}");
        cancel.cancel();
    }

    // -------------------------------------------------------------
    // a40 prefix-resolution tests for the marker-clearing handlers.
    // The four actions accept either an exact change-directory name OR
    // a leading prefix, scoped to the action's relevant marker file.
    // -------------------------------------------------------------

    fn write_marker_file(workspace: &Path, change: &str, marker: &str) {
        let p = workspace
            .join("openspec/changes")
            .join(change)
            .join(marker);
        std::fs::write(p, "{}").unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_perma_stuck_resolves_prefix_to_canonical_slug() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        make_change(&workspace, "a37-unify-llm-provider-config");
        make_change(&workspace, "a38-bar");
        write_marker_file(
            &workspace,
            "a37-unify-llm-provider-config",
            ".perma-stuck.json",
        );
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "clear_perma_stuck_marker",
            "url": "git@github.com:owner/myrepo.git",
            "change": "a37",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(
            resp["change"].as_str().unwrap(),
            "a37-unify-llm-provider-config",
            "response must echo the canonical slug, not the prefix"
        );
        assert!(
            !workspace
                .join("openspec/changes/a37-unify-llm-provider-config/.perma-stuck.json")
                .exists()
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_perma_stuck_no_match_returns_scope_named_error() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        make_change(&workspace, "a37-foo");
        // No perma-stuck marker on a37-foo.
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "clear_perma_stuck_marker",
            "url": "git@github.com:owner/myrepo.git",
            "change": "a99",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        let err = resp["error"].as_str().unwrap();
        assert!(
            err.contains("a99") && err.contains(".perma-stuck.json"),
            "no-match error must name prefix and marker: {err}"
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_perma_stuck_multi_match_lists_candidates() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        make_change(&workspace, "a37-foo");
        make_change(&workspace, "a38-bar");
        write_marker_file(&workspace, "a37-foo", ".perma-stuck.json");
        write_marker_file(&workspace, "a38-bar", ".perma-stuck.json");
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "clear_perma_stuck_marker",
            "url": "git@github.com:owner/myrepo.git",
            "change": "a3",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        let err = resp["error"].as_str().unwrap();
        assert!(err.contains("a37-foo") && err.contains("a38-bar"), "{err}");
        assert!(err.contains("longer prefix"), "{err}");
        // Neither marker should have been removed.
        assert!(
            workspace
                .join("openspec/changes/a37-foo/.perma-stuck.json")
                .exists()
        );
        assert!(
            workspace
                .join("openspec/changes/a38-bar/.perma-stuck.json")
                .exists()
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_revision_resolves_prefix_to_canonical_slug() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        make_change(&workspace, "a37-foo");
        make_change(&workspace, "a38-bar");
        write_marker_file(&workspace, "a37-foo", ".needs-spec-revision.json");
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "clear_revision_marker",
            "url": "git@github.com:owner/myrepo.git",
            "change": "a37",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(
            resp["change"].as_str().unwrap(),
            "a37-foo",
            "response must echo the canonical slug, not the prefix"
        );
        assert!(
            !workspace
                .join("openspec/changes/a37-foo/.needs-spec-revision.json")
                .exists()
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_revision_no_match_returns_scope_named_error() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        make_change(&workspace, "a37-foo");
        // a37-foo has perma-stuck only, NOT needs-spec-revision.
        write_marker_file(&workspace, "a37-foo", ".perma-stuck.json");
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "clear_revision_marker",
            "url": "git@github.com:owner/myrepo.git",
            "change": "a37",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        let err = resp["error"].as_str().unwrap();
        assert!(
            err.contains("a37") && err.contains(".needs-spec-revision.json"),
            "no-match error must name prefix and the correct scope marker: {err}"
        );
        // Perma-stuck marker untouched.
        assert!(
            workspace
                .join("openspec/changes/a37-foo/.perma-stuck.json")
                .exists()
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_revision_multi_match_lists_candidates() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        make_change(&workspace, "a37-foo");
        make_change(&workspace, "a38-bar");
        write_marker_file(&workspace, "a37-foo", ".needs-spec-revision.json");
        write_marker_file(&workspace, "a38-bar", ".needs-spec-revision.json");
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "clear_revision_marker",
            "url": "git@github.com:owner/myrepo.git",
            "change": "a3",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        let err = resp["error"].as_str().unwrap();
        assert!(err.contains("a37-foo") && err.contains("a38-bar"), "{err}");
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ignore_for_queue_no_match_returns_either_blocking_scope_error() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        make_change(&workspace, "a37-foo");
        // No blocking markers on a37-foo, so any prefix is a no-match.
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "ignore_for_queue_marker",
            "url": "git@github.com:owner/myrepo.git",
            "change": "a37",
            "marked_by": "tester",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        let err = resp["error"].as_str().unwrap();
        assert!(
            err.contains("a37")
                && err.contains(".perma-stuck.json")
                && err.contains(".needs-spec-revision.json"),
            "EitherBlocking no-match error must name both markers: {err}"
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_ignore_for_queue_no_match_returns_scope_named_error() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        make_change(&workspace, "a37-foo");
        // No ignore-for-queue marker present.
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "clear_ignore_for_queue_marker",
            "url": "git@github.com:owner/myrepo.git",
            "change": "a37",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        let err = resp["error"].as_str().unwrap();
        assert!(
            err.contains("a37") && err.contains(".ignore-for-queue.json"),
            "no-match error must name prefix and marker: {err}"
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unknown_url_returns_error_for_marker_clear() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "clear_perma_stuck_marker",
            "url": "git@github.com:owner/UNKNOWN.git",
            "change": "x",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        let err = resp["error"].as_str().unwrap();
        assert!(err.contains("no repository configured"), "got: {err}");
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wipe_workspace_removes_directory_and_returns_path() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(workspace.join("openspec/changes")).unwrap();
        std::fs::write(workspace.join("dummy.txt"), "x").unwrap();
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        assert!(workspace.exists());
        let req = serde_json::json!({
            "action": "wipe_workspace",
            "url": "git@github.com:owner/myrepo.git",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert!(!workspace.exists(), "workspace should have been removed");
        assert_eq!(
            resp["path"].as_str().unwrap(),
            workspace.display().to_string(),
            "response must echo the removed path"
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wipe_workspace_is_idempotent_when_directory_absent() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        // Intentionally do NOT create the workspace directory.
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "wipe_workspace",
            "url": "git@github.com:owner/myrepo.git",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(
            resp["ok"],
            serde_json::Value::Bool(true),
            "missing dir must be Ok: {resp}"
        );
        assert_eq!(resp["already_absent"], serde_json::Value::Bool(true));
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wipe_workspace_reports_no_iteration_in_flight_when_handle_unset() {
        // The seeded fixture's per-repo handle has `iteration_cancel: None`
        // (the fake polling task isn't running an actual iteration loop).
        // The wipe handler must short-circuit straight to the deletion and
        // report the "no iteration in flight" outcome.
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "wipe_workspace",
            "url": "git@github.com:owner/myrepo.git",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(
            resp["drain_outcome"].as_str().unwrap(),
            "no iteration in flight",
            "resp: {resp}"
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wipe_workspace_drains_cleanly_when_iteration_responds_quickly() {
        // Plant a per-iteration cancel handle on the fake polling task and
        // arrange for the iteration_drained Notify to fire as soon as the
        // cancel is observed. The wipe handler should record a
        // "drained cleanly in <Xs>" outcome and proceed with the deletion.
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let (_dir, socket, state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let url = "git@github.com:owner/myrepo.git";

        // Install a per-iteration cancel + spawn a tiny task that fires
        // the Notify when the cancel observes a cancellation.
        let (iter_cancel, drained) = {
            let guard = state.repo_tasks.lock().unwrap();
            let h = guard.get(url).expect("seeded handle");
            let token = CancellationToken::new();
            *h.iteration_cancel.lock().unwrap() = Some(token.clone());
            (token, h.iteration_drained.clone())
        };
        let watcher = tokio::spawn(async move {
            iter_cancel.cancelled().await;
            drained.notify_waiters();
        });

        let req = serde_json::json!({
            "action": "wipe_workspace",
            "url": url,
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        let outcome = resp["drain_outcome"].as_str().unwrap();
        assert!(
            outcome.starts_with("drained cleanly in "),
            "expected drained-cleanly outcome, got: {outcome}"
        );
        assert!(!workspace.exists(), "workspace must be deleted after drain");

        let _ = watcher.await;
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wipe_workspace_reports_drain_timeout_when_iteration_ignores_cancel() {
        // Plant a per-iteration cancel handle BUT do NOT fire the
        // iteration_drained Notify. The drain must time out and the wipe
        // must run anyway.
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        // Set wipe_drain_timeout_secs: 1 so the timeout fires fast.
        let yaml = format!(
            r#"
repositories:
  - url: "git@github.com:owner/myrepo.git"
    local_path: "{}"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
  wipe_drain_timeout_secs: 1
github:
  token_env: GITHUB_TOKEN
  token:
    value: "ghp_fixture"
"#,
            workspace.display()
        );
        let (_dir, socket, state, _cfg_path, cancel) = fixture_listener(&yaml).await;
        let url = "git@github.com:owner/myrepo.git";
        {
            let guard = state.repo_tasks.lock().unwrap();
            let h = guard.get(url).expect("seeded handle");
            *h.iteration_cancel.lock().unwrap() = Some(CancellationToken::new());
        }
        let req = serde_json::json!({
            "action": "wipe_workspace",
            "url": url,
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(
            resp["drain_outcome"].as_str().unwrap(),
            "drain timeout — iteration may have been stuck",
            "resp: {resp}"
        );
        assert!(!workspace.exists(), "wipe must run regardless of drain");
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wipe_workspace_already_absent_includes_drain_outcome() {
        // Idempotent no-op case: the workspace is already gone AND no
        // iteration is in flight. The response must still carry the
        // (no-iteration) drain_outcome for the chatops formatter.
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("never-created");
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "wipe_workspace",
            "url": "git@github.com:owner/myrepo.git",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(resp["already_absent"], serde_json::Value::Bool(true));
        assert_eq!(
            resp["drain_outcome"].as_str().unwrap(),
            "no iteration in flight"
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn repo_status_assembles_marker_alert_and_queue_snapshot() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        make_change(&workspace, "a06-foo");
        make_change(&workspace, "a07-bar");
        make_change(&workspace, "a08-ready");
        // Marker on a06 + a07.
        std::fs::write(
            workspace.join("openspec/changes/a06-foo/.perma-stuck.json"),
            r#"{"change":"a06-foo","consecutive_failures":2,"last_reason":"x","marked_stuck_at":"2026-01-01T00:00:00Z","operator_action":"x"}"#,
        )
        .unwrap();
        std::fs::write(
            workspace.join("openspec/changes/a07-bar/.needs-spec-revision.json"),
            r#"{"change":"a07-bar","marked_at":"2026-01-01T00:00:00Z","unimplementable_tasks":[],"revision_suggestion":"x","operator_action":"x"}"#,
        )
        .unwrap();
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "repo_status",
            "url": "git@github.com:owner/myrepo.git",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        let status = &resp["status"];
        assert_eq!(status["url"], "git@github.com:owner/myrepo.git");
        let perma: Vec<String> = status["perma_stuck_changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["change"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(perma, vec!["a06-foo".to_string()]);
        let revision: Vec<String> = status["revision_marked_changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["change"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(revision, vec!["a07-bar".to_string()]);
        // Pending = a08-ready (the others are marker-excluded).
        let pending: Vec<String> = status["pending_changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(pending, vec!["a08-ready".to_string()]);
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn repo_status_all_aggregates_one_round_trip_per_repo() {
        // Two-repo fixture: the daemon should bundle both per-repo
        // statuses into a single response so the chatops menu only
        // pays one round trip.
        let dir = TempDir::new().unwrap();
        let ws_a = dir.path().join("ws-a");
        let ws_b = dir.path().join("ws-b");
        std::fs::create_dir_all(&ws_a).unwrap();
        std::fs::create_dir_all(&ws_b).unwrap();
        make_change(&ws_a, "a06-foo");
        let yaml = format!(
            r#"
repositories:
  - url: "git@github.com:owner/aaa.git"
    local_path: "{}"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
  - url: "git@github.com:owner/bbb.git"
    local_path: "{}"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github:
  token_env: GITHUB_TOKEN
  token:
    value: "ghp_fixture"
"#,
            ws_a.display(),
            ws_b.display()
        );
        let (_dir, socket, _state, _cfg_path, cancel) = fixture_listener(&yaml).await;
        let resp = send_request(&socket, r#"{"action":"repo_status_all"}"#).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        let results = resp["results"]
            .as_array()
            .expect("results must be an array");
        assert_eq!(results.len(), 2, "two repos → two results");
        let urls: Vec<String> = results
            .iter()
            .map(|e| e["url"].as_str().unwrap().to_string())
            .collect();
        assert!(urls.contains(&"git@github.com:owner/aaa.git".to_string()));
        assert!(urls.contains(&"git@github.com:owner/bbb.git".to_string()));
        // Every per-repo entry is ok=true and ships a status payload.
        for entry in results {
            assert_eq!(
                entry["ok"], serde_json::Value::Bool(true),
                "every entry must be ok=true: {entry}"
            );
            assert!(
                entry.get("status").is_some(),
                "every entry must ship `status`: {entry}"
            );
        }
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn repo_status_handles_missing_workspace_gracefully() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("never-created");
        let (_dir, socket, _state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let req = serde_json::json!({
            "action": "repo_status",
            "url": "git@github.com:owner/myrepo.git",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        let status = &resp["status"];
        assert!(status["perma_stuck_changes"].as_array().unwrap().is_empty());
        assert!(status["pending_changes"].as_array().unwrap().is_empty());
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn end_to_end_dispatcher_drives_full_flow_through_real_socket() {
        use crate::chatops::operator_commands::{
            ControlSocketSubmitter, OperatorCommandDispatcher, RepoIdentity, Reply,
        };

        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        make_change(&workspace, "a06-foo");
        std::fs::write(
            workspace.join("openspec/changes/a06-foo/.perma-stuck.json"),
            r#"{"change":"a06-foo","consecutive_failures":2,"last_reason":"x","marked_stuck_at":"2026-01-01T00:00:00Z","operator_action":"x"}"#,
        )
        .unwrap();

        let (_dir, socket, state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let submitter = ControlSocketSubmitter::new(socket.clone());
        let dispatcher = OperatorCommandDispatcher::new(&state.paths);
        let repos: Vec<RepoIdentity> = state
            .last_config
            .load_full()
            .repositories
            .iter()
            .map(|r| RepoIdentity {
                url: r.url.clone(),
                workspace_path: crate::workspace::resolve_path(&state.paths, r),
            })
            .collect();
        let bot = "<@UBOT>";
        let reply = dispatcher
            .handle_message(
                &format!("{bot} clear-perma-stuck myrepo a06-foo"),
                "C1",
                bot,
                &repos,
                &submitter,
            )
            .await
            .expect("dispatcher must produce a reply");
        let reply_text = match reply {
            Reply::Sync(s) => s,
            other => panic!("expected Sync reply, got {other:?}"),
        };
        assert!(reply_text.starts_with("✓"), "expected success reply: {reply_text}");
        assert!(
            !workspace
                .join("openspec/changes/a06-foo/.perma-stuck.json")
                .exists(),
            "marker must have been removed"
        );
        cancel.cancel();
    }

    /// a40: end-to-end backtick-wrapped + prefix-only chatops flow. The
    /// operator types `clear-revision myrepo \`a37\`` (backtick-wrapped
    /// prefix). The parser strips the backticks, the control-socket handler
    /// resolves the prefix to the canonical slug `a37-foo`, and the marker
    /// file is removed. The dispatcher's reply names the canonical slug.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn end_to_end_backtick_wrapped_prefix_resolves_and_clears_marker() {
        use crate::chatops::operator_commands::{
            ControlSocketSubmitter, OperatorCommandDispatcher, RepoIdentity, Reply,
        };

        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        // Exactly one change with .needs-spec-revision.json — the prefix
        // `a37` resolves unambiguously.
        make_change(&workspace, "a37-foo");
        std::fs::write(
            workspace.join("openspec/changes/a37-foo/.needs-spec-revision.json"),
            r#"{"change":"a37-foo","marked_at":"2026-01-01T00:00:00Z","unimplementable_tasks":[],"revision_suggestion":"x","operator_action":"x"}"#,
        )
        .unwrap();
        // A second change that does NOT carry the marker — included to
        // verify scope-filtering works: prefix `a` would otherwise match
        // both directories.
        make_change(&workspace, "a38-bar");

        let (_dir, socket, state, _cfg_path, cancel) =
            fixture_listener(&local_path_yaml(&workspace)).await;
        let submitter = ControlSocketSubmitter::new(socket.clone());
        let dispatcher = OperatorCommandDispatcher::new(&state.paths);
        let repos: Vec<RepoIdentity> = state
            .last_config
            .load_full()
            .repositories
            .iter()
            .map(|r| RepoIdentity {
                url: r.url.clone(),
                workspace_path: crate::workspace::resolve_path(&state.paths, r),
            })
            .collect();
        let bot = "<@UBOT>";
        let reply = dispatcher
            .handle_message(
                &format!("{bot} clear-revision myrepo `a37`"),
                "C1",
                bot,
                &repos,
                &submitter,
            )
            .await
            .expect("dispatcher must produce a reply");
        let reply_text = match reply {
            Reply::Sync(s) => s,
            other => panic!("expected Sync reply, got {other:?}"),
        };
        assert!(
            reply_text.starts_with("✓"),
            "expected success reply: {reply_text}"
        );
        // The dispatcher reply names the canonical slug (a37-foo), NOT
        // the prefix (a37).
        assert!(
            reply_text.contains("a37-foo"),
            "reply must echo canonical slug: {reply_text}"
        );
        // The marker file is gone — the resolver correctly mapped the
        // prefix to a37-foo and the marker-removal call targeted the
        // canonical directory.
        assert!(
            !workspace
                .join("openspec/changes/a37-foo/.needs-spec-revision.json")
                .exists(),
            "marker file under canonical directory must be removed"
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reload_transient_cancelled_url_is_not_respawned() {
        let (_dir, socket, state, cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let url = "git@github.com:owner/repo.git";
        // Simulate the transient state: the URL is in the task map but
        // its cancellation token is already cancelled (the task is
        // mid-shutdown — finishing its in-flight iteration). The fake
        // spawn helper auto-removes its map entry when cancelled, so to
        // hold the URL in the "cancelled-but-present" state we replace
        // the seeded handle with one whose backing task is parked
        // forever.
        let parked_repo = RepositoryConfig { forge: None,
            url: url.to_string(),
            local_path: None,
            base_branch: "main".into(),
            agent_branch: "agent-q".into(),
            poll_interval_sec: 60,
            chatops_channel_id: None,
            max_changes_per_pr: None,
            audits: None,
            spec_storage: None,
            upstream: None,
            auto_submit_pr: true,
            octopus_guide: None,
            sandbox: None,
        };
        {
            let mut guard = state.repo_tasks.lock().unwrap();
            if let Some(prev) = guard.remove(url) {
                // Cancel the auto-spawned task so its wrapper finishes
                // and drops its references. We don't care about its
                // map-removal because we just removed the entry under
                // the lock.
                prev.cancel.cancel();
                prev.join.abort();
            }
            let pre_cancelled = CancellationToken::new();
            pre_cancelled.cancel();
            let parked = tokio::spawn(async {
                std::future::pending::<()>().await;
            });
            guard.insert(
                url.to_string(),
                RepoTaskHandle {
                    cancel: pre_cancelled,
                    config: Arc::new(ArcSwap::from_pointee(parked_repo)),
                    join: parked,
                    pending_rebuild: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    pending_triages: Arc::new(Mutex::new(Vec::new())),
                    pending_audit_runs: Arc::new(Mutex::new(Vec::new())),
                    pending_proposal_requests: Arc::new(Mutex::new(Vec::new())),
                    pending_changelog_requests: Arc::new(Mutex::new(Vec::new())),
                    pending_brownfield_requests:
                        Arc::new(Mutex::new(std::collections::VecDeque::new())),
                    pending_scout_requests:
                        Arc::new(Mutex::new(std::collections::VecDeque::new())),
                    pending_spec_it_requests:
                        Arc::new(Mutex::new(std::collections::VecDeque::new())),
                    pending_sync_upstream_requests:
                        Arc::new(Mutex::new(std::collections::VecDeque::new())),
                    pending_brownfield_survey_requests:
                        Arc::new(Mutex::new(std::collections::VecDeque::new())),
                    pending_brownfield_batch_requests:
                        Arc::new(Mutex::new(std::collections::VecDeque::new())),
                    pending_revision_requests: RevisionRequestQueues::new(),
                    iteration_cancel: Arc::new(Mutex::new(None)),
                    iteration_drained: Arc::new(Notify::new()),
                },
            );
        }
        // Re-write the SAME YAML (URL unchanged). The reload sees the
        // URL in `existing`, but its per-repo token is cancelled →
        // WARN + skip. The URL should NOT appear in added/changed/removed.
        std::fs::write(&cfg_path, BASE_YAML).unwrap();
        let resp = send_request(&socket, r#"{"action":"reload"}"#).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        let added = delta_urls(&resp, "added");
        let changed = delta_urls(&resp, "changed");
        let removed = delta_urls(&resp, "removed");
        assert!(
            !added.contains(&url.to_string()),
            "transient cancelled URL must not be in added: {resp}"
        );
        assert!(
            !changed.contains(&url.to_string()),
            "transient cancelled URL must not be in changed: {resp}"
        );
        assert!(
            !removed.contains(&url.to_string()),
            "transient cancelled URL must not be in removed: {resp}"
        );
        // No second task was spawned: the map still has exactly one
        // entry (the parked transient one).
        let urls = task_map_urls(&state);
        assert_eq!(
            urls,
            vec![url.to_string()],
            "no second task should have been spawned"
        );
        // Manual teardown: abort the parked task before the runtime
        // shuts down so it doesn't leak.
        {
            let mut guard = state.repo_tasks.lock().unwrap();
            if let Some(h) = guard.remove(url) {
                h.join.abort();
            }
        }
        cancel.cancel();
    }

    // ---------- queue_audit (chatops-on-demand-audit-trigger) ----------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queue_audit_appends_to_pending_audit_runs() {
        let (_dir, socket, state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let req = serde_json::json!({
            "action": "queue_audit",
            "url": "git@github.com:owner/repo.git",
            "audit_type": "security_bug_audit",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(resp["audit_type"], "security_bug_audit");
        assert_eq!(resp["url"], "git@github.com:owner/repo.git");
        assert!(resp["poll_interval_sec"].is_u64(), "poll interval echoed: {resp}");

        // The handle's queue now contains the audit-type name.
        let guard = state.repo_tasks.lock().unwrap();
        let handle = guard.get("git@github.com:owner/repo.git").expect("repo present");
        let q = handle.pending_audit_runs.lock().unwrap();
        assert_eq!(
            q.iter().map(|a| a.audit_type.clone()).collect::<Vec<_>>(),
            vec!["security_bug_audit".to_string()]
        );
        drop(q);
        drop(guard);
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queue_audit_is_deduplicated_per_repo() {
        let (_dir, socket, state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let req = serde_json::json!({
            "action": "queue_audit",
            "url": "git@github.com:owner/repo.git",
            "audit_type": "security_bug_audit",
        });
        // First submit.
        let resp1 = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp1["ok"], serde_json::Value::Bool(true));
        // Second submit with the same audit_type → success but no
        // duplicate entry.
        let resp2 = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp2["ok"], serde_json::Value::Bool(true));

        let guard = state.repo_tasks.lock().unwrap();
        let handle = guard.get("git@github.com:owner/repo.git").expect("repo present");
        let q = handle.pending_audit_runs.lock().unwrap();
        assert_eq!(
            q.iter().map(|a| a.audit_type.clone()).collect::<Vec<_>>(),
            vec!["security_bug_audit".to_string()],
            "duplicate audit_type must collapse to one entry"
        );
        drop(q);
        drop(guard);
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queue_audit_distinct_types_both_recorded() {
        let (_dir, socket, state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        for at in ["security_bug_audit", "drift_audit"] {
            let req = serde_json::json!({
                "action": "queue_audit",
                "url": "git@github.com:owner/repo.git",
                "audit_type": at,
            });
            let resp = send_request(&socket, &req.to_string()).await;
            assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        }
        let guard = state.repo_tasks.lock().unwrap();
        let handle = guard.get("git@github.com:owner/repo.git").expect("repo present");
        let q = handle.pending_audit_runs.lock().unwrap();
        assert!(q.iter().any(|a| a.audit_type == "security_bug_audit"));
        assert!(q.iter().any(|a| a.audit_type == "drift_audit"));
        assert_eq!(q.len(), 2);
        drop(q);
        drop(guard);
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queue_audit_unknown_url_returns_error() {
        let (_dir, socket, _state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let req = serde_json::json!({
            "action": "queue_audit",
            "url": "git@github.com:owner/UNKNOWN.git",
            "audit_type": "security_bug_audit",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        let err = resp["error"].as_str().unwrap();
        assert!(err.contains("no repository configured"), "got: {err}");
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queue_audit_missing_audit_type_field_returns_error() {
        let (_dir, socket, _state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let req = serde_json::json!({
            "action": "queue_audit",
            "url": "git@github.com:owner/repo.git",
        });
        let resp = send_request(&socket, &req.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        let err = resp["error"].as_str().unwrap();
        assert!(err.contains("audit_type"), "got: {err}");
        cancel.cancel();
    }

    // ---------- a20a4: fork-PR-mode head-qualifier regression ----------

    /// Regression test for the a20a4 status bug: in fork-PR mode,
    /// `fetch_latest_pr` must construct the GitHub head qualifier as
    /// `<fork_owner>:<branch>`, NOT `<upstream_owner>:<branch>`.
    ///
    /// Pre-fix code passed only the upstream owner to
    /// `latest_pr_for_head`, which used it as both the URL-path owner
    /// AND the head qualifier owner. In fork-PR mode this produced
    /// `head=upstream-owner:agent-q` queries that never matched
    /// fork-headed PRs — operators saw `latest PR: (none)` even when
    /// a PR was open.
    ///
    /// This test mocks GitHub with a strict `head=fork-acc:agent-q`
    /// matcher. Pre-fix code would issue the wrong query AND mockito's
    /// `.expect(1)` would not be met; the test would fail.
    #[tokio::test]
    async fn status_shows_latest_pr_in_fork_pr_mode() {
        let env_var = "STATUS_TOKEN_FORK_MODE";
        // SAFETY: tests run sequentially via tokio runtime; the env var
        // is unique to this test.
        unsafe {
            std::env::set_var(env_var, "tok");
        }
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/repos/owner/repo/pulls")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded(
                    "head".into(),
                    "fork-acc:agent-q".into(),
                ),
                mockito::Matcher::UrlEncoded("state".into(), "all".into()),
                mockito::Matcher::UrlEncoded("sort".into(), "created".into()),
                mockito::Matcher::UrlEncoded(
                    "direction".into(),
                    "desc".into(),
                ),
                mockito::Matcher::UrlEncoded("per_page".into(), "1".into()),
            ]))
            .with_status(200)
            .with_body(
                r#"[{
                    "number": 99,
                    "title": "Fork-mode PR",
                    "state": "open",
                    "html_url": "https://example.invalid/pr/99",
                    "created_at": "2026-05-25T10:00:00Z",
                    "merged_at": null,
                    "head": {"ref": "agent-q"}
                }]"#,
            )
            .expect(1)
            .create_async()
            .await;
        let repo = RepositoryConfig { forge: None,
            url: "git@github.com:owner/repo.git".to_string(),
            local_path: None,
            base_branch: "main".into(),
            agent_branch: "agent-q".into(),
            poll_interval_sec: 60,
            chatops_channel_id: None,
            max_changes_per_pr: None,
            audits: None,
            spec_storage: None,
            upstream: None,
            auto_submit_pr: true,
            octopus_guide: None,
            sandbox: None,
        };
        let gh = GithubConfig {
            token_env: env_var.to_string(),
            token: None,
            owner_tokens: None,
            fork_owner: Some("fork-acc".to_string()),
            recreate_fork_on_reinit: false,
            command_authorization: Default::default(),
        };

        let pr = super::fetch_latest_pr_at(&server.url(), &repo, &gh).await;
        assert!(
            pr.is_some(),
            "fork-PR mode must surface the open PR; pre-fix code returned None here"
        );
        let pr = pr.unwrap();
        assert_eq!(pr.number, 99);
        mock.assert_async().await;

        unsafe {
            std::env::remove_var(env_var);
        }
    }

    // ---------- open-PR park: daemon-side query (1.1 / 1.2) ----------

    fn open_pr_park_repo() -> RepositoryConfig {
        RepositoryConfig {
            forge: None,
            url: "git@github.com:owner/repo.git".to_string(),
            local_path: None,
            base_branch: "main".into(),
            agent_branch: "agent-q".into(),
            poll_interval_sec: 60,
            chatops_channel_id: None,
            max_changes_per_pr: None,
            audits: None,
            spec_storage: None,
            upstream: None,
            auto_submit_pr: true,
            octopus_guide: None,
            sandbox: None,
        }
    }

    fn open_pr_park_github(env_var: &str) -> GithubConfig {
        GithubConfig {
            token_env: env_var.to_string(),
            token: None,
            owner_tokens: None,
            fork_owner: None,
            recreate_fork_on_reinit: false,
            command_authorization: Default::default(),
        }
    }

    /// 1.1: `fetch_open_agent_prs_at` runs the SAME `state=open` head
    /// query the skip-iteration gate uses and returns the open PRs.
    #[tokio::test]
    async fn fetch_open_agent_prs_returns_open_prs() {
        let env_var = "STATUS_OPEN_PR_PARK_OK";
        // SAFETY: tests run sequentially; the env var is unique here.
        unsafe {
            std::env::set_var(env_var, "tok");
        }
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/repos/owner/repo/pulls")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("state".into(), "open".into()),
                mockito::Matcher::UrlEncoded("head".into(), "owner:agent-q".into()),
                mockito::Matcher::UrlEncoded("per_page".into(), "100".into()),
            ]))
            .with_status(200)
            .with_body(
                r#"[
                    {"number": 12, "title": "later", "state": "open",
                     "html_url": "https://example.invalid/pr/12",
                     "head": {"ref": "agent-q"}, "base": {"ref": "main"},
                     "created_at": "2026-05-25T10:00:00Z"},
                    {"number": 5, "title": "earlier", "state": "open",
                     "html_url": "https://example.invalid/pr/5",
                     "head": {"ref": "agent-q"}, "base": {"ref": "main"},
                     "created_at": "2026-05-24T10:00:00Z"}
                ]"#,
            )
            .expect(1)
            .create_async()
            .await;

        let prs = super::fetch_open_agent_prs_at(
            &server.url(),
            &open_pr_park_repo(),
            &open_pr_park_github(env_var),
        )
        .await;
        let prs = prs.expect("successful query must be Some");
        assert_eq!(prs.len(), 2);
        assert!(prs.contains(&5));
        mock.assert_async().await;

        unsafe {
            std::env::remove_var(env_var);
        }
    }

    /// 1.2: a GitHub failure degrades to `None` (park unknown) rather than
    /// fabricating a park or failing the whole status reply.
    #[tokio::test]
    async fn fetch_open_agent_prs_degrades_to_none_on_github_error() {
        let env_var = "STATUS_OPEN_PR_PARK_ERR";
        // SAFETY: tests run sequentially; the env var is unique here.
        unsafe {
            std::env::set_var(env_var, "tok");
        }
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", mockito::Matcher::Any)
            .with_status(500)
            .with_body("boom")
            .create_async()
            .await;

        let prs = super::fetch_open_agent_prs_at(
            &server.url(),
            &open_pr_park_repo(),
            &open_pr_park_github(env_var),
        )
        .await;
        assert!(prs.is_none(), "a GitHub error must degrade to None, not Some");

        unsafe {
            std::env::remove_var(env_var);
        }
    }

    // -----------------------------------------------------------------
    // a27a0: record_outcome + consume_outcome control-socket actions
    // -----------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn record_outcome_then_consume_outcome_round_trips_success() {
        let (_dir, socket, _state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let record_req = r#"{"action":"record_outcome","workspace_basename":"my-repo","change":"a30-foo","outcome":{"type":"success","final_answer":"done"}}"#;
        let resp = send_request(&socket, record_req).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");

        let consume_req = r#"{"action":"consume_outcome","workspace_basename":"my-repo","change":"a30-foo"}"#;
        let resp = send_request(&socket, consume_req).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(resp["outcome"]["type"], "success");
        assert_eq!(resp["outcome"]["final_answer"], "done");

        // Second consume drains: returns null.
        let resp2 = send_request(&socket, consume_req).await;
        assert_eq!(resp2["ok"], serde_json::Value::Bool(true), "resp: {resp2}");
        assert!(resp2["outcome"].is_null());

        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn record_outcome_round_trips_spec_needs_revision_payload() {
        let (_dir, socket, _state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let record_req = r#"{"action":"record_outcome","workspace_basename":"my-repo","change":"a30-foo","outcome":{"type":"spec_needs_revision","unimplementable_tasks":[{"task_id":"6.4","task_text":"Manual: SSH...","reason":"no SSH access"}],"revision_suggestion":"Replace 6.4 with a mocked unit test"}}"#;
        let resp = send_request(&socket, record_req).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");

        let consume_req = r#"{"action":"consume_outcome","workspace_basename":"my-repo","change":"a30-foo"}"#;
        let resp = send_request(&socket, consume_req).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(resp["outcome"]["type"], "spec_needs_revision");
        assert_eq!(
            resp["outcome"]["revision_suggestion"],
            "Replace 6.4 with a mocked unit test"
        );
        assert_eq!(resp["outcome"]["unimplementable_tasks"][0]["task_id"], "6.4");
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn record_outcome_replaces_prior_entry_for_same_key() {
        let (_dir, socket, _state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let first = r#"{"action":"record_outcome","workspace_basename":"my-repo","change":"a30-foo","outcome":{"type":"success","final_answer":"first"}}"#;
        let _ = send_request(&socket, first).await;
        let second = r#"{"action":"record_outcome","workspace_basename":"my-repo","change":"a30-foo","outcome":{"type":"spec_needs_revision","unimplementable_tasks":[{"task_id":"1","task_text":"t","reason":"r"}],"revision_suggestion":"s"}}"#;
        let _ = send_request(&socket, second).await;
        let consume_req = r#"{"action":"consume_outcome","workspace_basename":"my-repo","change":"a30-foo"}"#;
        let resp = send_request(&socket, consume_req).await;
        // Last-writer-wins: the second `record_outcome` wins.
        assert_eq!(resp["outcome"]["type"], "spec_needs_revision");
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn consume_outcome_unknown_key_returns_null() {
        let (_dir, socket, _state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let resp = send_request(
            &socket,
            r#"{"action":"consume_outcome","workspace_basename":"x","change":"y"}"#,
        )
        .await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert!(resp["outcome"].is_null());
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn record_outcome_with_unknown_variant_tag_returns_structured_error() {
        let (_dir, socket, _state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let resp = send_request(
            &socket,
            r#"{"action":"record_outcome","workspace_basename":"x","change":"y","outcome":{"type":"unknown_variant","data":{}}}"#,
        )
        .await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false));
        let err = resp["error"].as_str().unwrap();
        assert!(
            err.contains("unknown_variant"),
            "error should name the unknown tag: {err}"
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn record_outcome_with_missing_outcome_field_returns_error() {
        let (_dir, socket, _state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let resp = send_request(
            &socket,
            r#"{"action":"record_outcome","workspace_basename":"x","change":"y"}"#,
        )
        .await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false));
        let err = resp["error"].as_str().unwrap();
        assert!(err.contains("outcome"), "err: {err}");
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn record_outcome_round_trips_iteration_request_payload() {
        let (_dir, socket, _state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let record_req = r#"{"action":"record_outcome","workspace_basename":"my-repo","change":"a30-foo","outcome":{"type":"iteration_request","completed_tasks":["1","2"],"remaining_tasks":["3"],"reason":"task 3 needs a refactor I want to plan more carefully"}}"#;
        let resp = send_request(&socket, record_req).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");

        let consume_req = r#"{"action":"consume_outcome","workspace_basename":"my-repo","change":"a30-foo"}"#;
        let resp = send_request(&socket, consume_req).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        // The consume_outcome response includes the new variant tag
        // automatically thanks to serde::Serialize on the enum.
        assert_eq!(resp["outcome"]["type"], "iteration_request");
        assert_eq!(resp["outcome"]["completed_tasks"][0], "1");
        assert_eq!(resp["outcome"]["completed_tasks"][1], "2");
        assert_eq!(resp["outcome"]["remaining_tasks"][0], "3");
        assert_eq!(
            resp["outcome"]["reason"],
            "task 3 needs a refactor I want to plan more carefully"
        );
        cancel.cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn outcome_store_keys_do_not_collide_across_repos() {
        let (_dir, socket, _state, _cfg_path, cancel) = fixture_listener(BASE_YAML).await;
        let a = r#"{"action":"record_outcome","workspace_basename":"repo-a","change":"a30-foo","outcome":{"type":"success","final_answer":"A"}}"#;
        let b = r#"{"action":"record_outcome","workspace_basename":"repo-b","change":"a30-foo","outcome":{"type":"success","final_answer":"B"}}"#;
        let _ = send_request(&socket, a).await;
        let _ = send_request(&socket, b).await;
        let ca = send_request(
            &socket,
            r#"{"action":"consume_outcome","workspace_basename":"repo-a","change":"a30-foo"}"#,
        )
        .await;
        assert_eq!(ca["outcome"]["final_answer"], "A");
        let cb = send_request(
            &socket,
            r#"{"action":"consume_outcome","workspace_basename":"repo-b","change":"a30-foo"}"#,
        )
        .await;
        assert_eq!(cb["outcome"]["final_answer"], "B");
        cancel.cancel();
    }

    // a56: record_submission → consume_submission round-trips the payload
    // AND clears it (the executor-side relay's `record_submission` target).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn record_submission_then_consume_round_trips_and_clears() {
        let (_dir, socket, _state, _cfg, cancel) = fixture_listener(BASE_YAML).await;
        let rec = serde_json::json!({
            "action": "record_submission",
            "workspace_basename": "repo",
            "change": "a56-foo",
            "role": "reviewer",
            "payload": {"verdict": "approve", "notes": "ok"},
        });
        let resp = send_request(&socket, &rec.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");

        let con = r#"{"action":"consume_submission","workspace_basename":"repo","change":"a56-foo"}"#;
        let resp = send_request(&socket, con).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true));
        assert_eq!(resp["submission"]["verdict"], "approve");
        assert_eq!(resp["submission"]["notes"], "ok");

        // Second consume drains to null — the prior call cleared the store.
        let resp2 = send_request(&socket, con).await;
        assert_eq!(resp2["ok"], serde_json::Value::Bool(true));
        assert_eq!(resp2["submission"], serde_json::Value::Null);
        cancel.cancel();
    }

    // a56: consume with no stored submission is empty, not an error.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn consume_submission_with_no_entry_is_empty_not_error() {
        let (_dir, socket, _state, _cfg, cancel) = fixture_listener(BASE_YAML).await;
        let resp = send_request(
            &socket,
            r#"{"action":"consume_submission","workspace_basename":"repo","change":"never"}"#,
        )
        .await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true));
        assert_eq!(resp["submission"], serde_json::Value::Null);
        cancel.cancel();
    }

    // verifier-gates-persist-session-log task 4.1/5.4: the
    // `record_advertised_tool` action records, daemon-side, which submit tool
    // the MCP child advertised for a session's role — `Some(tool)` when one
    // matched, `None` when none did — keyed by (workspace_basename, change) and
    // surviving consume, so a no-submission consume can report mode (a). Assert
    // the recorded store facts, not log wording.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn record_advertised_tool_records_some_and_none_daemon_side() {
        let (_dir, socket, state, _cfg, cancel) = fixture_listener(BASE_YAML).await;

        // Role whose MCP child advertised a submit tool.
        let with = serde_json::json!({
            "action": "record_advertised_tool",
            "workspace_basename": "repo",
            "change": "a30-with",
            "role": "reviewer",
            "tool": "submit_review",
        });
        let resp = send_request(&socket, &with.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(
            state.submission_store.advertised_tool("repo", "a30-with"),
            Some(("reviewer".into(), Some("submit_review".into()))),
            "the advertised tool is recorded daemon-side with its role"
        );

        // Role whose MCP child advertised NO submit tool (tool omitted/null —
        // this is the mode (a) fact). Best-effort: still records ok.
        let without = serde_json::json!({
            "action": "record_advertised_tool",
            "workspace_basename": "repo",
            "change": "a30-none",
            "role": "implementer",
            "tool": serde_json::Value::Null,
        });
        let resp = send_request(&socket, &without.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(
            state.submission_store.advertised_tool("repo", "a30-none"),
            Some(("implementer".into(), None)),
            "a role with no matching tool records None — mode (a) is determinable"
        );

        // The advertised-tool fact survives a no-submission consume, so the
        // consume diagnostic can report advertised + relayed + consumed
        // together. A consume that finds no live submission returns null AND the
        // advertised record persists for the diagnostic to read.
        let con = r#"{"action":"consume_submission","workspace_basename":"repo","change":"a30-none"}"#;
        let resp = send_request(&socket, con).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true));
        assert_eq!(
            resp["submission"],
            serde_json::Value::Null,
            "no live submission for this held session"
        );
        assert!(
            !state.submission_store.was_ever_relayed("repo", "a30-none"),
            "never relayed — mode (b)/(a): advertised=none, relayed=no, consumed=none"
        );
        assert_eq!(
            state.submission_store.advertised_tool("repo", "a30-none"),
            Some(("implementer".into(), None)),
            "the advertised-tool fact survives the consume so the diagnostic can report it"
        );
        cancel.cancel();
    }

    // a56: a payload that fails the role's registered schema is rejected
    // (nothing stored) with a reason suitable for the relay to surface.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn record_submission_rejects_schema_invalid_payload() {
        let (_dir, socket, state, _cfg, cancel) = fixture_listener(BASE_YAML).await;
        // The listener serves a clone of `state` sharing the same Arc-backed
        // store, so a validator registered here is visible to the handler.
        state.submission_store.register_schema(
            "reviewer",
            std::sync::Arc::new(|p: &serde_json::Value| {
                if p.get("verdict").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty()) {
                    Ok(())
                } else {
                    Err("verdict must be a non-empty string".to_string())
                }
            }),
        );
        let rec = serde_json::json!({
            "action": "record_submission",
            "workspace_basename": "repo",
            "change": "a56-foo",
            "role": "reviewer",
            "payload": {"verdict": ""},
        });
        let resp = send_request(&socket, &rec.to_string()).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        assert!(
            resp["error"].as_str().unwrap_or_default().contains("verdict"),
            "error names the offending field: {resp}"
        );
        // Nothing was stored.
        let resp = send_request(
            &socket,
            r#"{"action":"consume_submission","workspace_basename":"repo","change":"a56-foo"}"#,
        )
        .await;
        assert_eq!(resp["submission"], serde_json::Value::Null);
        cancel.cancel();
    }

    /// The listener is a HARD precondition: with the control-socket env var
    /// unset (no `spawn_submission_listener` active), a submission drain
    /// returns `None` — exactly what makes a gate / audit fail closed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn without_listener_consume_returns_none_fail_closed() {
        let _g = crate::testing::ENV_LOCK.lock().unwrap();
        // SAFETY: serialized by ENV_LOCK; we restore by clearing below.
        unsafe {
            std::env::remove_var(crate::mcp_askuser_server::ENV_CONTROL_SOCKET);
        }
        let drained = crate::audits::try_consume_submission(
            Path::new("/tmp/myrepo"),
            "some-change",
        )
        .await;
        assert!(
            drained.is_none(),
            "with no listener the drain MUST be None (the gate/audit fails closed)"
        );
    }

    /// `spawn_submission_listener` stands up the transport in-process: it
    /// sets the env var to a live bound socket, registers the gate + audit
    /// schemas, and a recorded submission round-trips via the same
    /// `try_consume_submission` path the gates / audits use under a daemon.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_listener_stands_up_transport_and_captures_submission() {
        let _g = crate::testing::ENV_LOCK.lock().unwrap();
        let (_td, paths) = crate::testing::test_daemon_paths();
        let guard = spawn_submission_listener(&paths).expect("listener stands up");

        // The env var points at the live socket.
        let env_socket = std::env::var(crate::mcp_askuser_server::ENV_CONTROL_SOCKET)
            .expect("env var set by the helper");
        assert_eq!(
            PathBuf::from(&env_socket),
            guard.socket_path().to_path_buf(),
            "env var must name the bound socket"
        );
        assert!(guard.socket_path().exists(), "socket file must be live");

        // Simulate the MCP child relaying a `submit_findings` payload over the
        // socket. `architecture_advisor`'s role/schema is registered by the
        // helper via `register_submission_schemas`.
        let rec = serde_json::json!({
            "action": "record_submission",
            "workspace_basename": "myrepo",
            "change": "some-change",
            "role": "architecture_advisor",
            "payload": {"findings": []},
        });
        let resp = send_request(guard.socket_path(), &rec.to_string()).await;
        assert_eq!(
            resp["ok"],
            serde_json::Value::Bool(true),
            "record must succeed against the helper-registered schema: {resp}"
        );

        // The audit/gate drain path captures it (basename == "myrepo").
        let drained = crate::audits::try_consume_submission(
            Path::new("/some/where/myrepo"),
            "some-change",
        )
        .await;
        assert!(
            drained.is_some(),
            "with the listener up, the submission is captured"
        );

        // Drop tears the transport down and removes the socket file.
        let socket = guard.socket_path().to_path_buf();
        drop(guard);
        // Cancellation → serve unwinds → removes the file. Poll briefly.
        let mut gone = false;
        for _ in 0..50 {
            if !socket.exists() {
                gone = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(gone, "the socket file must be removed on guard drop");
        // Clean up the env var we set (serialized by ENV_LOCK).
        unsafe {
            std::env::remove_var(crate::mcp_askuser_server::ENV_CONTROL_SOCKET);
        }
    }

    // ================================================================
    // a01: workspace-mutating ops preempt and serialize against the pass
    // ================================================================

    use std::sync::Mutex as StdMutex;

    /// Recording fake signaller: captures the pgid(s) it was asked to
    /// SIGTERM so a test asserts the preempt fired the kill without
    /// signalling a real process group.
    struct RecordingSignaller {
        sent: StdMutex<Vec<i32>>,
    }
    impl RecordingSignaller {
        fn new() -> Self {
            Self {
                sent: StdMutex::new(Vec::new()),
            }
        }
    }
    impl PreemptSignaller for RecordingSignaller {
        fn sigterm_pgid(&self, pgid: i32) {
            self.sent.lock().unwrap().push(pgid);
        }
    }

    /// Local ProcessOps mock for the preempt tests: lets a test drive
    /// PID-liveness + comm so the busy-marker classification (acquire /
    /// fresh / ambiguous) is deterministic on any platform (macOS skips
    /// the real `/proc/<pid>/comm` read).
    struct PreemptMockOps {
        alive: Vec<u32>,
        comms: std::collections::HashMap<u32, String>,
        killpg_terminate_called: StdMutex<Vec<i32>>,
        killpg_kill_called: StdMutex<Vec<i32>>,
    }
    impl PreemptMockOps {
        fn new() -> Self {
            Self {
                alive: Vec::new(),
                comms: std::collections::HashMap::new(),
                killpg_terminate_called: StdMutex::new(Vec::new()),
                killpg_kill_called: StdMutex::new(Vec::new()),
            }
        }
        fn with_alive(mut self, pid: u32) -> Self {
            self.alive.push(pid);
            self
        }
        fn with_comm(mut self, pid: u32, comm: &str) -> Self {
            self.comms.insert(pid, comm.to_string());
            self
        }
    }
    impl busy_marker::ProcessOps for PreemptMockOps {
        fn pid_alive(&self, pid: u32) -> bool {
            self.alive.contains(&pid)
        }
        fn read_comm(&self, pid: u32) -> Option<String> {
            self.comms.get(&pid).cloned()
        }
        fn killpg_terminate(&self, pgid: i32) {
            self.killpg_terminate_called.lock().unwrap().push(pgid);
        }
        fn killpg_kill(&self, pgid: i32) {
            self.killpg_kill_called.lock().unwrap().push(pgid);
        }
        fn wait_for_exit(&self, _pid: u32, _max: std::time::Duration) {}
    }

    /// Write a busy-marker JSON directly for `workspace` under the test
    /// daemon paths, with control over `pid`, `comm`, age, and the
    /// recorded `change`. Mirrors `busy_marker::pre_populate_marker` but
    /// is local to this module (that one is private to busy_marker).
    fn write_marker(
        paths: &crate::paths::DaemonPaths,
        workspace: &Path,
        pid: u32,
        comm: &str,
        age_secs: i64,
        change: &str,
    ) {
        let path = busy_marker::marker_path(paths, workspace);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let started = chrono::Utc::now() - chrono::Duration::seconds(age_secs);
        let marker = busy_marker::BusyMarker {
            repo_url: "git@github.com:owner/repo.git".into(),
            pid,
            pgid: 1234,
            comm: comm.into(),
            started_at: started,
            stage: busy_marker::Stage::Executor,
            change: change.into(),
        };
        let body = serde_json::to_string_pretty(&marker).unwrap();
        std::fs::write(&path, body).unwrap();
    }

    /// Install an `iteration_cancel` token on the seeded handle for `url`
    /// so a test can observe the preempt firing the per-iteration cancel.
    fn install_iteration_cancel(state: &ControlState, url: &str) -> CancellationToken {
        let token = CancellationToken::new();
        let guard = state.repo_tasks.lock().unwrap();
        let handle = guard.get(url).expect("seeded handle present");
        *handle.iteration_cancel.lock().unwrap() = Some(token.clone());
        token
    }

    fn repo_from(cfg: &Config) -> RepositoryConfig {
        cfg.repositories[0].clone()
    }

    /// 4.1 — marker held (recorded change), sidecar present: the helper
    /// fires the iteration cancel + sidecar SIGTERM, then acquires, and
    /// returns the cancelled change slug. We use a DEAD pid so
    /// `try_acquire`'s immediate dead-pid recovery yields `Acquired`
    /// (modelling the post-SIGTERM "child exited" case) without the test
    /// having to race a real marker release.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn preempt_fires_cancel_and_sigterm_then_acquires_naming_change() {
        let dir = TempDir::new().unwrap();
        let cfg_path = write_yaml(dir.path(), BASE_YAML);
        let cfg = Config::load_from(&cfg_path).unwrap();
        let cancel = CancellationToken::new();
        let state = seeded_state(cfg_path, &cfg, cancel.clone());
        let repo = repo_from(&cfg);
        let workspace = dir.path().join("ws-preempt");
        std::fs::create_dir_all(&workspace).unwrap();

        // Marker with a dead pid + recorded change, plus a sidecar pid.
        write_marker(&state.paths, &workspace, 999_999, "claude", 5, "a07-foo");
        busy_marker::write_subprocess_marker(&state.paths, &workspace, 4242).unwrap();
        let token = install_iteration_cancel(&state, &repo.url);
        let sig = RecordingSignaller::new();
        // pid 999_999 is NOT marked alive → dead-pid recovery yields Acquired,
        // modelling the post-SIGTERM "child exited" case.
        let ops = PreemptMockOps::new();

        let outcome = preempt_and_acquire_busy_marker_with(&state, &repo, &workspace, &sig, &ops)
            .await
            .expect("preempt+acquire should succeed against a dead-pid marker");

        assert!(token.is_cancelled(), "iteration cancel must have fired");
        assert_eq!(
            sig.sent.lock().unwrap().clone(),
            vec![4242],
            "SIGTERM must target the sidecar pid"
        );
        assert_eq!(
            outcome.preempted_change.as_deref(),
            Some("a07-foo"),
            "preempted_change names the cancelled change"
        );
        assert!(
            busy_marker::marker_path(&state.paths, &workspace).exists(),
            "the held guard's marker file exists while in scope"
        );
        drop(outcome);
        assert!(
            !busy_marker::marker_path(&state.paths, &workspace).exists(),
            "marker released on guard drop"
        );
        cancel.cancel();
    }

    /// 4.2 — no marker present: acquire directly, no sidecar read, no
    /// SIGTERM, preempted_change is None.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn preempt_with_no_marker_acquires_directly() {
        let dir = TempDir::new().unwrap();
        let cfg_path = write_yaml(dir.path(), BASE_YAML);
        let cfg = Config::load_from(&cfg_path).unwrap();
        let cancel = CancellationToken::new();
        let state = seeded_state(cfg_path, &cfg, cancel.clone());
        let repo = repo_from(&cfg);
        let workspace = dir.path().join("ws-clean");
        std::fs::create_dir_all(&workspace).unwrap();
        let sig = RecordingSignaller::new();
        let ops = PreemptMockOps::new();

        let outcome = preempt_and_acquire_busy_marker_with(&state, &repo, &workspace, &sig, &ops)
            .await
            .expect("clean acquire");
        assert!(
            sig.sent.lock().unwrap().is_empty(),
            "no SIGTERM with no pass in flight"
        );
        assert_eq!(outcome.preempted_change, None);
        assert!(busy_marker::marker_path(&state.paths, &workspace).exists());
        cancel.cancel();
    }

    /// 4.3 — held-for-whole-op: while the returned guard is in scope a
    /// concurrent `try_acquire` for the same workspace yields
    /// SkipFreshInProgress; after the guard drops it yields Acquired.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn preempt_holds_marker_for_whole_op() {
        let dir = TempDir::new().unwrap();
        let cfg_path = write_yaml(dir.path(), BASE_YAML);
        let cfg = Config::load_from(&cfg_path).unwrap();
        let cancel = CancellationToken::new();
        let state = seeded_state(cfg_path, &cfg, cancel.clone());
        let repo = repo_from(&cfg);
        let workspace = dir.path().join("ws-hold");
        std::fs::create_dir_all(&workspace).unwrap();
        let sig = RecordingSignaller::new();
        let ops = PreemptMockOps::new();

        let outcome = preempt_and_acquire_busy_marker_with(&state, &repo, &workspace, &sig, &ops)
            .await
            .expect("clean acquire");
        // Concurrent acquire while held → fresh-in-progress (held by THIS
        // live process, so pid_alive=true and age below threshold).
        match busy_marker::try_acquire(&state.paths, &workspace, &repo.url, 600).unwrap() {
            busy_marker::AcquireOutcome::SkipFreshInProgress(_) => {}
            other => panic!(
                "expected SkipFreshInProgress while held, got {}",
                match other {
                    busy_marker::AcquireOutcome::Acquired(_) => "Acquired",
                    busy_marker::AcquireOutcome::SkipAmbiguous(_) => "Ambiguous",
                    busy_marker::AcquireOutcome::SkipFreshInProgress(_) => "Fresh",
                }
            ),
        }
        drop(outcome);
        match busy_marker::try_acquire(&state.paths, &workspace, &repo.url, 600).unwrap() {
            busy_marker::AcquireOutcome::Acquired(_) => {}
            _ => panic!("expected Acquired after guard drop"),
        }
        cancel.cancel();
    }

    /// 4.4 — ambiguous marker (PID alive, comm differs): the helper
    /// returns `Busy` and leaves the marker file in place. We use the
    /// current process's live PID with a deliberately wrong recorded comm
    /// and a past-threshold age so `try_acquire` classifies it ambiguous.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn preempt_on_ambiguous_marker_returns_busy_and_leaves_file() {
        let dir = TempDir::new().unwrap();
        let cfg_path = write_yaml(dir.path(), BASE_YAML);
        let cfg = Config::load_from(&cfg_path).unwrap();
        let cancel = CancellationToken::new();
        let state = seeded_state(cfg_path, &cfg, cancel.clone());
        let repo = repo_from(&cfg);
        let workspace = dir.path().join("ws-ambiguous");
        std::fs::create_dir_all(&workspace).unwrap();

        // Live pid (this process), recorded comm "definitely-not-real-comm"
        // that cannot match /proc/<pid>/comm, age past the 600s threshold →
        // ambiguous. No sidecar so the bounded wait returns immediately
        // (drain timeout default applies but the marker stays).
        let live = std::process::id();
        write_marker(
            &state.paths,
            &workspace,
            live,
            "definitely-not-real-comm",
            10_000,
            "a09-bar",
        );
        let sig = RecordingSignaller::new();
        // Mock: the recorded pid is alive, but its live comm ("claude")
        // differs from the recorded comm ("definitely-not-real-comm") →
        // try_acquire classifies SkipAmbiguous deterministically.
        let ops = PreemptMockOps::new().with_alive(live).with_comm(live, "claude");

        // Use a 0-second drain timeout via config override so the bounded
        // wait does not stall the test for the default. We do this by
        // swapping last_config with wipe_drain_timeout_secs = 0.
        let mut cfg2 = cfg.clone();
        cfg2.executor.wipe_drain_timeout_secs = 0;
        state.last_config.store(Arc::new(cfg2));

        match preempt_and_acquire_busy_marker_with(&state, &repo, &workspace, &sig, &ops).await {
            Err(PreemptAcquireError::Busy(_)) => {}
            Err(PreemptAcquireError::Internal(m)) => panic!("expected Busy, got Internal: {m}"),
            Ok(_) => panic!("ambiguous marker must surface a Busy error, not Acquired"),
        }
        assert!(
            busy_marker::marker_path(&state.paths, &workspace).exists(),
            "ambiguous marker MUST be left in place for investigation"
        );
        // Cleanup so a live-pid marker does not leak across tests.
        let _ = std::fs::remove_file(busy_marker::marker_path(&state.paths, &workspace));
        cancel.cancel();
    }

    /// unconditional-rollback §3.1 — the FORCEFUL path escalates on a
    /// still-held marker (the polite path would classify SkipFreshInProgress):
    /// it fires the busy-marker forced reclaim (SIGKILL the process group +
    /// clear the marker) and returns `Acquired`, NEVER `Busy`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn forceful_escalates_on_still_held_marker_and_acquires() {
        let dir = TempDir::new().unwrap();
        let cfg_path = write_yaml(dir.path(), BASE_YAML);
        let cfg = Config::load_from(&cfg_path).unwrap();
        let cancel = CancellationToken::new();
        let state = seeded_state(cfg_path, &cfg, cancel.clone());
        let repo = repo_from(&cfg);
        let workspace = dir.path().join("ws-forceful-held");
        std::fs::create_dir_all(&workspace).unwrap();

        // Live pid, comm matches, age BELOW threshold → the polite acquire
        // classifies SkipFreshInProgress (would be `Busy` for a non-destructive
        // op). marker.pgid = 1234 (set by write_marker), no sidecar.
        let live = std::process::id();
        write_marker(&state.paths, &workspace, live, "claude", 10, "a07-foo");
        let sig = RecordingSignaller::new();
        let ops = PreemptMockOps::new().with_alive(live).with_comm(live, "claude");

        // Zero drain timeout so the bounded marker-release wait returns at once
        // (the marker stays held, driving the escalation).
        let mut cfg2 = cfg.clone();
        cfg2.executor.wipe_drain_timeout_secs = 0;
        state.last_config.store(Arc::new(cfg2));

        let outcome =
            preempt_and_force_acquire_busy_marker_with(&state, &repo, &workspace, &sig, &ops)
                .await
                .expect("forceful path must escalate and ACQUIRE, never Busy");

        // The forced reclaim SIGKILL'd the held holder's process group (1234).
        assert_eq!(
            ops.killpg_kill_called.lock().unwrap().clone(),
            vec![1234],
            "forceful escalation must SIGKILL the held marker's process group"
        );
        // The marker the test wrote was cleared and a FRESH one acquired by
        // this live process.
        assert!(
            busy_marker::marker_path(&state.paths, &workspace).exists(),
            "the freshly-acquired guard's marker exists while in scope"
        );
        drop(outcome);
        assert!(
            !busy_marker::marker_path(&state.paths, &workspace).exists(),
            "marker released on guard drop"
        );
        cancel.cancel();
    }

    /// unconditional-rollback §3.2 — the FORCEFUL path also reclaims a
    /// PID-reuse-suspected (SkipAmbiguous) marker for the confirmed rollback,
    /// while the POLITE path still returns `Busy` on the SAME marker (the
    /// non-destructive ops' behavior is unchanged).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn forceful_reclaims_ambiguous_marker_while_polite_returns_busy() {
        let dir = TempDir::new().unwrap();
        let cfg_path = write_yaml(dir.path(), BASE_YAML);
        let cfg = Config::load_from(&cfg_path).unwrap();
        let cancel = CancellationToken::new();
        let state = seeded_state(cfg_path, &cfg, cancel.clone());
        let repo = repo_from(&cfg);

        let live = std::process::id();

        // --- POLITE path: ambiguous marker → Busy, file left in place. ---
        let ws_polite = dir.path().join("ws-ambiguous-polite");
        std::fs::create_dir_all(&ws_polite).unwrap();
        write_marker(&state.paths, &ws_polite, live, "definitely-not-real", 10_000, "a09-bar");
        let sig_p = RecordingSignaller::new();
        let ops_p = PreemptMockOps::new().with_alive(live).with_comm(live, "claude");
        let mut cfg2 = cfg.clone();
        cfg2.executor.wipe_drain_timeout_secs = 0;
        state.last_config.store(Arc::new(cfg2));
        match preempt_and_acquire_busy_marker_with(&state, &repo, &ws_polite, &sig_p, &ops_p).await {
            Err(PreemptAcquireError::Busy(_)) => {}
            Err(PreemptAcquireError::Internal(m)) => {
                panic!("polite path must return Busy on an ambiguous marker, got Internal: {m}")
            }
            Ok(_) => panic!("polite path must return Busy on an ambiguous marker, got Acquired"),
        }
        assert!(
            busy_marker::marker_path(&state.paths, &ws_polite).exists(),
            "polite path leaves the ambiguous marker for investigation"
        );
        let _ = std::fs::remove_file(busy_marker::marker_path(&state.paths, &ws_polite));

        // --- FORCEFUL path: ambiguous marker → reclaimed + Acquired. ---
        let ws_force = dir.path().join("ws-ambiguous-force");
        std::fs::create_dir_all(&ws_force).unwrap();
        write_marker(&state.paths, &ws_force, live, "definitely-not-real", 10_000, "a09-bar");
        let sig_f = RecordingSignaller::new();
        let ops_f = PreemptMockOps::new().with_alive(live).with_comm(live, "claude");
        let outcome =
            preempt_and_force_acquire_busy_marker_with(&state, &repo, &ws_force, &sig_f, &ops_f)
                .await
                .expect("forceful path must reclaim an ambiguous marker and ACQUIRE");
        assert_eq!(
            ops_f.killpg_kill_called.lock().unwrap().clone(),
            vec![1234],
            "forceful escalation past ambiguity must SIGKILL the held marker's process group"
        );
        assert!(busy_marker::marker_path(&state.paths, &ws_force).exists());
        drop(outcome);
        assert!(!busy_marker::marker_path(&state.paths, &ws_force).exists());
        cancel.cancel();
    }

    /// 4.5 — rollback handler: a dry_run invocation acquires NO marker
    /// (the marker is absent both during and after). The live path's
    /// acquire-before-mutation is covered by the helper unit tests above
    /// (4.1-4.4) plus the held-for-whole-op assertion; here we lock in the
    /// dry-run scenario's "no lock, no preempt" contract, which is the one
    /// directly observable without a real git remote.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rollback_dry_run_acquires_no_marker() {
        let dir = TempDir::new().unwrap();
        let cfg_path = write_yaml(dir.path(), BASE_YAML);
        let cfg = Config::load_from(&cfg_path).unwrap();
        let cancel = CancellationToken::new();
        let state = seeded_state(cfg_path, &cfg, cancel.clone());
        let repo = repo_from(&cfg);
        let workspace = workspace::resolve_path(&state.paths, &repo);

        let req = json!({
            "action": "rollback_recovery",
            "url": repo.url,
            "count": 1,
            "dry_run": true,
        });
        // The dry run fails at workspace init (no real remote to clone),
        // which is fine: the assertion is that NO busy marker was ever
        // acquired by the dry-run path, before OR after the failure.
        let _ = handle_rollback_recovery(&req, &state).await;
        assert!(
            !busy_marker::marker_path(&state.paths, &workspace).exists(),
            "dry-run path must NOT acquire the busy marker"
        );
        cancel.cancel();
    }

    /// 4.7 — dry-run is genuinely READ-ONLY: a `dry_run`
    /// `handle_rollback_recovery` resolves the plan against `origin/<base>`
    /// WITHOUT a checkout or reset, so it performs no working-tree mutation.
    /// We seed the workspace with a sentinel uncommitted change AND assert it
    /// survives the dry-run byte-for-byte, the HEAD does not move, the porcelain
    /// status is unchanged, AND the dry-run still returns the correct plan
    /// (the in-range change/issue slugs + a preview naming them). Asserts
    /// working-tree state, not message wording. With the OLD mutating preamble
    /// (`git checkout` + `git reset --hard origin/<base>`) the sentinel would be
    /// blown away — this test is the read-only regression guard.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rollback_dry_run_is_read_only_and_returns_plan() {
        use std::process::Command;

        fn run(path: &Path, args: &[&str]) {
            let st = Command::new("git")
                .args(args)
                .current_dir(path)
                .status()
                .unwrap();
            assert!(st.success(), "git {args:?} failed in {}", path.display());
        }
        fn porcelain(path: &Path) -> Vec<u8> {
            Command::new("git")
                .args(["status", "--porcelain"])
                .current_dir(path)
                .output()
                .unwrap()
                .stdout
        }

        let dir = TempDir::new().unwrap();
        // Build a managed workspace with autocoder's commit shape: a base
        // commit, then one "ship change" commit (code + dated archive +
        // canon fold) and one "ship issue" commit. The rollback range is
        // the last two commits.
        let workspace = dir.path().join("workspace");
        run(dir.path(), &["init", "-q", "-b", "main", "workspace"]);
        run(&workspace, &["config", "user.email", "t@e.com"]);
        run(&workspace, &["config", "user.name", "t"]);
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        std::fs::write(workspace.join("src/lib.rs"), "// base\n").unwrap();
        std::fs::create_dir_all(workspace.join("openspec/specs/widget")).unwrap();
        std::fs::write(
            workspace.join("openspec/specs/widget/spec.md"),
            "CANON widget v1\n",
        )
        .unwrap();
        run(&workspace, &["add", "-A"]);
        run(&workspace, &["commit", "-q", "-m", "base"]);

        // ship change `feature-a`
        std::fs::write(workspace.join("src/a.rs"), "// feature-a\n").unwrap();
        std::fs::write(
            workspace.join("openspec/specs/widget/spec.md"),
            "CANON widget v1\nMODIFIED a\n",
        )
        .unwrap();
        let arch_a = workspace.join("openspec/changes/archive/2026-05-01-feature-a");
        std::fs::create_dir_all(&arch_a).unwrap();
        std::fs::write(arch_a.join("proposal.md"), "## Why\nfeature-a\n").unwrap();
        run(&workspace, &["add", "-A"]);
        run(&workspace, &["commit", "-q", "-m", "feature-a: ship change"]);

        // ship issue `fix-thing`
        std::fs::write(workspace.join("src/c.rs"), "// fix fix-thing\n").unwrap();
        let arch_i = workspace.join("issues/archive/2026-05-02-fix-thing");
        std::fs::create_dir_all(&arch_i).unwrap();
        std::fs::write(arch_i.join("issue.md"), "## Report\nfix-thing\n").unwrap();
        run(&workspace, &["add", "-A"]);
        run(&workspace, &["commit", "-q", "-m", "fix-thing: ship issue fix"]);

        // Create a bare `origin` the workspace fetches from (so `origin/main`
        // resolves) and point the workspace's `origin` remote at it.
        let origin = dir.path().join("origin.git");
        run(dir.path(), &["clone", "-q", "--bare", "workspace", "origin.git"]);
        run(&workspace, &["remote", "add", "origin", origin.to_str().unwrap()]);
        run(&workspace, &["fetch", "-q", "origin"]);

        // Seed a SENTINEL uncommitted working-tree change — the thing a
        // concurrent agentic session might have in flight. The dry-run must
        // leave it untouched.
        let sentinel = workspace.join("src/SENTINEL.txt");
        std::fs::write(&sentinel, "in-flight agent work — do not clobber\n").unwrap();

        let head_before = {
            let out = Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&workspace)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let porcelain_before = porcelain(&workspace);
        let canon_before =
            std::fs::read_to_string(workspace.join("openspec/specs/widget/spec.md")).unwrap();

        // Configure a repo whose workspace IS the prepared dir (via
        // `local_path`) and whose URL is the bare origin, so
        // `ensure_initialized` sees an existing `.git` and only fetches.
        let cfg_path = write_yaml(dir.path(), BASE_YAML);
        let mut cfg = Config::load_from(&cfg_path).unwrap();
        cfg.repositories[0].url = format!("file://{}", origin.display());
        cfg.repositories[0].local_path = Some(workspace.clone());
        let cancel = CancellationToken::new();
        let state = seeded_state(cfg_path, &cfg, cancel.clone());
        let repo = repo_from(&cfg);

        let req = json!({
            "action": "rollback_recovery",
            "url": repo.url,
            "count": 2,
            "dry_run": true,
        });
        let resp = handle_rollback_recovery(&req, &state).await;

        // The dry-run returns a correct plan resolved against origin/main.
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(resp["dry_run"], serde_json::Value::Bool(true));
        assert_eq!(resp["commit_count"], serde_json::json!(2), "resp: {resp}");
        let changes: Vec<String> = resp["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(changes, vec!["feature-a"], "resp: {resp}");
        let issues: Vec<String> = resp["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(issues, vec!["fix-thing"], "resp: {resp}");
        let preview = resp["preview"].as_str().unwrap();
        assert!(preview.contains("feature-a"), "preview names the change: {preview}");
        assert!(preview.contains("fix-thing"), "preview names the issue: {preview}");

        // READ-ONLY: the sentinel survives byte-for-byte, HEAD did not move,
        // porcelain status is unchanged, and canon is untouched (no reset).
        assert!(
            sentinel.is_file(),
            "sentinel uncommitted file must survive a read-only dry-run"
        );
        assert_eq!(
            std::fs::read_to_string(&sentinel).unwrap(),
            "in-flight agent work — do not clobber\n",
            "sentinel content must be unchanged by the dry-run"
        );
        let head_after = {
            let out = Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&workspace)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        assert_eq!(head_after, head_before, "dry-run must not move HEAD");
        assert_eq!(
            porcelain(&workspace),
            porcelain_before,
            "dry-run must not change the working-tree status"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("openspec/specs/widget/spec.md")).unwrap(),
            canon_before,
            "dry-run must not reset canon (no `git reset --hard`)"
        );
        // And, per 4.5, no busy marker was acquired.
        assert!(
            !busy_marker::marker_path(&state.paths, &workspace).exists(),
            "dry-run must not acquire the busy marker"
        );
        cancel.cancel();
    }

    /// unconditional-rollback §7.1 — THE end-to-end gate. Drives a REAL
    /// confirmed rollback through `handle_rollback_recovery` against the
    /// ADVERSARIAL state all at once and asserts it SUCCEEDS and produces a
    /// correct, clean PR:
    ///   - a busy marker held by a LIVE in-flight pass (a real spawned child) →
    ///     forcibly reclaimed (the real RealProcessOps SIGKILLs its group);
    ///   - an in-range unit colliding with an active dir → reconciled;
    ///   - a pre-existing agent-branch PR (mockito) → reused + retitled (no 422);
    ///   - a built `target/` with NO `.gitignore` at the target → excluded.
    /// End state asserted: marker released, in-range units active-exactly-once
    /// with canon fold undone, a SINGLE agent-branch PR carrying the rolled-back
    /// source with a rollback title/body, and NO `target/` committed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rollback_confirmed_end_to_end_forceful_reconcile_reuse_pr_no_target() {
        use std::os::unix::process::CommandExt;
        use std::process::Command;

        fn run(path: &Path, args: &[&str]) {
            let st = Command::new("git")
                .args(args)
                .current_dir(path)
                .status()
                .unwrap();
            assert!(st.success(), "git {args:?} failed in {}", path.display());
        }

        let _hook_lock = crate::polling_loop::test_hooks::lock();

        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("workspace");
        run(dir.path(), &["init", "-q", "-b", "main", "workspace"]);
        run(&workspace, &["config", "user.email", "t@e.com"]);
        run(&workspace, &["config", "user.name", "t"]);

        // --- commit 1 (the rollback TARGET): base code + canon, NO .gitignore. ---
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        std::fs::write(workspace.join("src/lib.rs"), "// base\n").unwrap();
        std::fs::create_dir_all(workspace.join("openspec/specs/widget")).unwrap();
        std::fs::write(workspace.join("openspec/specs/widget/spec.md"), "CANON widget v1\n").unwrap();
        run(&workspace, &["add", "-A"]);
        run(&workspace, &["commit", "-q", "-m", "base"]);

        // --- commit 2: ship change feature-a (code + canon fold + dated archive). ---
        std::fs::write(workspace.join("src/a.rs"), "// feature-a impl\n").unwrap();
        std::fs::write(
            workspace.join("openspec/specs/widget/spec.md"),
            "CANON widget v1\nMODIFIED a\n",
        )
        .unwrap();
        let arch_a = workspace.join("openspec/changes/archive/2026-05-01-feature-a");
        std::fs::create_dir_all(&arch_a).unwrap();
        std::fs::write(arch_a.join("proposal.md"), "## Why\nfeature-a\n").unwrap();
        run(&workspace, &["add", "-A"]);
        run(&workspace, &["commit", "-q", "-m", "feature-a: ship change"]);

        // --- commit 3: ship issue fix-thing (code + dated issues archive). ---
        std::fs::write(workspace.join("src/c.rs"), "// fix fix-thing\n").unwrap();
        let arch_i = workspace.join("issues/archive/2026-05-02-fix-thing");
        std::fs::create_dir_all(&arch_i).unwrap();
        std::fs::write(arch_i.join("issue.md"), "## Report\nfix-thing\n").unwrap();
        run(&workspace, &["add", "-A"]);
        run(&workspace, &["commit", "-q", "-m", "fix-thing: ship issue fix"]);

        // --- commit 4: plant the COLLISION — an active openspec/changes/feature-a/
        //     dir of the same slug ALONGSIDE the dated archive entry (the stale
        //     duplicate the production case hits). ---
        std::fs::create_dir_all(workspace.join("openspec/changes/feature-a")).unwrap();
        std::fs::write(
            workspace.join("openspec/changes/feature-a/proposal.md"),
            "stale active feature-a\n",
        )
        .unwrap();
        run(&workspace, &["add", "-A"]);
        run(&workspace, &["commit", "-q", "-m", "plant stale active feature-a"]);

        // Bare origin the workspace fetches/pushes against.
        let origin = dir.path().join("origin.git");
        run(dir.path(), &["clone", "-q", "--bare", "workspace", "origin.git"]);
        run(&workspace, &["remote", "add", "origin", origin.to_str().unwrap()]);
        run(&workspace, &["fetch", "-q", "origin"]);

        // --- adversarial untracked state: a built `target/` (no `.gitignore`
        //     anywhere in the tree). A naive `git add -A` would stage it once
        //     the target's `.gitignore` is gone; the workspace-local exclude
        //     must keep it out. ---
        std::fs::create_dir_all(workspace.join("target/debug")).unwrap();
        std::fs::write(workspace.join("target/debug/autocoder"), "ELF-bytes").unwrap();

        // Config: real github-form URL (so parse_repo_url works) but local_path
        // pins the workspace (so ensure_initialized only fetches). Zero drain
        // timeout so the bounded marker-release wait returns at once, driving
        // the forceful escalation.
        let cfg_path = write_yaml(dir.path(), BASE_YAML);
        let mut cfg = Config::load_from(&cfg_path).unwrap();
        cfg.repositories[0].local_path = Some(workspace.clone());
        cfg.executor.wipe_drain_timeout_secs = 0;
        // Small stale threshold so the live-pid marker is classified stuck.
        cfg.executor.busy_marker_stale_threshold_secs = Some(1);
        let cancel = CancellationToken::new();
        let state = seeded_state(cfg_path, &cfg, cancel.clone());
        let repo = repo_from(&cfg);

        // --- the held busy marker: a REAL live child (`sleep`) in its OWN
        //     process group, recorded in both the marker AND the sidecar so the
        //     forceful reclaim's RealProcessOps SIGKILLs a real, safe-to-kill
        //     process group (not the test runner's). ---
        let mut child = Command::new("sleep")
            .arg("300")
            .process_group(0)
            .spawn()
            .expect("spawn sleep child");
        let child_pid = child.id();
        // Write the marker by hand with the child as holder, comm "sleep",
        // aged past the (1s) threshold so it classifies as stuck.
        {
            let path = busy_marker::marker_path(&state.paths, &workspace);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let marker = busy_marker::BusyMarker {
                repo_url: repo.url.clone(),
                pid: child_pid,
                pgid: child_pid as i32,
                comm: "sleep".into(),
                started_at: chrono::Utc::now() - chrono::Duration::seconds(120),
                stage: busy_marker::Stage::Executor,
                change: "feature-a".into(),
            };
            std::fs::write(&path, serde_json::to_string_pretty(&marker).unwrap()).unwrap();
        }
        busy_marker::write_subprocess_marker(&state.paths, &workspace, child_pid).unwrap();

        // --- mockito: a pre-existing agent-branch PR (#42) is found, then
        //     reused via PATCH (no raw create / 422). ---
        let mut server = mockito::Server::new_async().await;
        let list_mock = server
            .mock("GET", "/repos/owner/repo/pulls")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_body(
                r#"[{"number":42,"html_url":"https://github.com/owner/repo/pull/42"}]"#,
            )
            .expect(1)
            .create_async()
            .await;
        let patch_mock = server
            .mock("PATCH", "/repos/owner/repo/pulls/42")
            .with_status(200)
            .with_body(
                r#"{"number":42,"html_url":"https://github.com/owner/repo/pull/42"}"#,
            )
            .expect(1)
            .create_async()
            .await;
        // A create POST must NEVER fire (that path would 422).
        let create_mock = server
            .mock("POST", "/repos/owner/repo/pulls")
            .with_status(422)
            .with_body(r#"{"message":"a pull request already exists"}"#)
            .expect(0)
            .create_async()
            .await;
        crate::polling_loop::test_hooks::set_github_api_base(Some(server.url()));

        // --- DRIVE THE REAL CONFIRMED ROLLBACK: roll back to the base tip
        //     (the last 3 commits). ---
        let req = json!({
            "action": "rollback_recovery",
            "url": repo.url,
            "count": 3,
            "dry_run": false,
        });
        let resp = handle_rollback_recovery(&req, &state).await;

        // Clean up the override + the child regardless of outcome.
        crate::polling_loop::test_hooks::set_github_api_base(None);
        let _ = child.kill();
        let _ = child.wait();

        // ---- assertions: it SUCCEEDED and produced a clean, correct PR ----
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "rollback must succeed: {resp}");
        assert_eq!(resp["outcome"], serde_json::json!("pr_opened"), "resp: {resp}");
        assert_eq!(
            resp["pr_url"], serde_json::json!("https://github.com/owner/repo/pull/42"),
            "the SINGLE reused PR's URL is returned: {resp}"
        );

        // The pre-existing PR was REUSED (listed + PATCHed), never raw-created.
        list_mock.assert_async().await;
        patch_mock.assert_async().await;
        create_mock.assert_async().await;

        // Marker RELEASED (the busy guard dropped at handler return).
        assert!(
            !busy_marker::marker_path(&state.paths, &workspace).exists(),
            "busy marker must be released after the rollback"
        );

        // Inspect the committed agent-branch tree (what the PR carries).
        let tracked: Vec<String> = {
            let out = Command::new("git")
                .args(["ls-tree", "-r", "--name-only", "agent-q"])
                .current_dir(&workspace)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|s| s.to_string())
                .collect()
        };

        // RECONCILE: feature-a active EXACTLY once, redundant dated archive gone.
        assert!(
            tracked.iter().any(|f| f == "openspec/changes/feature-a/proposal.md"),
            "feature-a active after reconcile: {tracked:?}"
        );
        assert!(
            !tracked
                .iter()
                .any(|f| f.starts_with("openspec/changes/archive/2026-05-01-feature-a")),
            "redundant dated archive entry removed: {tracked:?}"
        );
        // fix-thing issue unarchived to the active lane.
        assert!(
            tracked.iter().any(|f| f == "issues/fix-thing/issue.md"),
            "fix-thing issue active after rollback: {tracked:?}"
        );
        assert!(
            !tracked
                .iter()
                .any(|f| f.starts_with("issues/archive/2026-05-02-fix-thing")),
            "fix-thing archive entry moved out: {tracked:?}"
        );

        // CANON FOLD UNDONE: the in-range change's canon edit is reverted.
        let canon = {
            let out = Command::new("git")
                .args(["show", "agent-q:openspec/specs/widget/spec.md"])
                .current_dir(&workspace)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).to_string()
        };
        assert_eq!(canon, "CANON widget v1\n", "canon fold for feature-a must be undone");

        // CODE DISCARDED: the in-range implementation files are gone.
        assert!(!tracked.iter().any(|f| f == "src/a.rs"), "feature-a code discarded: {tracked:?}");
        assert!(!tracked.iter().any(|f| f == "src/c.rs"), "issue code discarded: {tracked:?}");
        assert!(tracked.iter().any(|f| f == "src/lib.rs"), "base code preserved: {tracked:?}");

        // NO build output committed even though the tree has no `.gitignore`.
        assert!(
            !tracked.iter().any(|f| f.starts_with("target/")),
            "target/ build output must NOT be committed: {tracked:?}"
        );

        cancel.cancel();
    }

    // ================================================================
    // a02: defer / undefer
    // ================================================================

    use std::process::Command as TestCmd;

    fn git_run(path: &Path, args: &[&str]) {
        let st = TestCmd::new("git")
            .args(args)
            .current_dir(path)
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?} failed in {}", path.display());
    }

    /// Build a managed workspace with a bare `origin` it can push to, plus
    /// an `agent-q` branch. Returns (TempDir, workspace path, origin path).
    /// Seeds a change `c01-foo` (dir), a single-file issue `i01-bar.md`,
    /// and a directory issue `i02-baz/`. Markers included to prove fs-move
    /// carries gitignored files.
    fn seed_defer_workspace() -> (TempDir, PathBuf, PathBuf) {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("workspace");
        git_run(dir.path(), &["init", "-q", "-b", "main", "workspace"]);
        git_run(&workspace, &["config", "user.email", "t@e.com"]);
        git_run(&workspace, &["config", "user.name", "t"]);

        // change c01-foo with a gitignored perma-stuck marker
        let chg = workspace.join("openspec/changes/c01-foo");
        std::fs::create_dir_all(&chg).unwrap();
        std::fs::write(chg.join("proposal.md"), "## Why\nfoo\n").unwrap();
        std::fs::write(chg.join(".perma-stuck.json"), "{}\n").unwrap();

        // single-file issue
        let issues = workspace.join("issues");
        std::fs::create_dir_all(&issues).unwrap();
        std::fs::write(issues.join("i01-bar.md"), "## Report\nbar\n").unwrap();

        // directory issue with a marker inside
        let idir = issues.join("i02-baz");
        std::fs::create_dir_all(&idir).unwrap();
        std::fs::write(idir.join("issue.md"), "## Report\nbaz\n").unwrap();
        std::fs::write(idir.join(".perma-stuck.json"), "{}\n").unwrap();

        // gitignore the markers so a tracked-only `git mv` would orphan them
        std::fs::write(workspace.join(".gitignore"), "**/.perma-stuck.json\n").unwrap();

        git_run(&workspace, &["add", "-A"]);
        git_run(&workspace, &["commit", "-q", "-m", "seed"]);
        git_run(&workspace, &["branch", "agent-q"]);

        let origin = dir.path().join("origin.git");
        git_run(dir.path(), &["clone", "-q", "--bare", "workspace", "origin.git"]);
        git_run(&workspace, &["remote", "add", "origin", origin.to_str().unwrap()]);
        git_run(&workspace, &["fetch", "-q", "origin"]);
        (dir, workspace, origin)
    }

    /// 7.2 detection (defer): only-change → Change; only single-file issue
    /// → IssueFile; only dir issue → IssueDir; absent → NotFound; both →
    /// Ambiguous.
    #[test]
    fn detection_defer_classifies_each_form_and_edges() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();

        std::fs::create_dir_all(ws.join("openspec/changes/only-change")).unwrap();
        assert!(matches!(
            locate_for_defer(ws, "only-change"),
            DeferLocate::NeedsMove(DeferKind::Change)
        ));

        std::fs::create_dir_all(ws.join("issues")).unwrap();
        std::fs::write(ws.join("issues/only-file.md"), "x").unwrap();
        assert!(matches!(
            locate_for_defer(ws, "only-file"),
            DeferLocate::NeedsMove(DeferKind::IssueFile)
        ));

        std::fs::create_dir_all(ws.join("issues/only-dir")).unwrap();
        assert!(matches!(
            locate_for_defer(ws, "only-dir"),
            DeferLocate::NeedsMove(DeferKind::IssueDir)
        ));

        assert!(matches!(locate_for_defer(ws, "nonesuch"), DeferLocate::NotFound));

        // both a change AND an issue with the same slug → ambiguous
        std::fs::create_dir_all(ws.join("openspec/changes/dup")).unwrap();
        std::fs::write(ws.join("issues/dup.md"), "x").unwrap();
        assert!(matches!(locate_for_defer(ws, "dup"), DeferLocate::Ambiguous(_)));
    }

    /// 7.3 detection (undefer): present only under deferred-changes / deferred-issues forms.
    #[test]
    fn detection_undefer_classifies_each_form_and_edges() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();

        std::fs::create_dir_all(ws.join("deferred-changes/dc")).unwrap();
        assert!(matches!(
            locate_for_undefer(ws, "dc"),
            DeferLocate::NeedsMove(DeferKind::Change)
        ));

        std::fs::create_dir_all(ws.join("deferred-issues")).unwrap();
        std::fs::write(ws.join("deferred-issues/df.md"), "x").unwrap();
        assert!(matches!(
            locate_for_undefer(ws, "df"),
            DeferLocate::NeedsMove(DeferKind::IssueFile)
        ));

        std::fs::create_dir_all(ws.join("deferred-issues/dd")).unwrap();
        assert!(matches!(
            locate_for_undefer(ws, "dd"),
            DeferLocate::NeedsMove(DeferKind::IssueDir)
        ));

        assert!(matches!(locate_for_undefer(ws, "nope"), DeferLocate::NotFound));
    }

    /// 7.6 idempotency at the detection layer: a defer whose lane location
    /// is absent but deferred is present → AlreadyDone; symmetric for undefer.
    #[test]
    fn detection_idempotency_already_done() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        // Already deferred: deferred-changes present, lane absent.
        std::fs::create_dir_all(ws.join("deferred-changes/c01")).unwrap();
        assert!(matches!(
            locate_for_defer(ws, "c01"),
            DeferLocate::AlreadyDone(DeferKind::Change)
        ));
        // Already active: lane present, deferred absent.
        std::fs::create_dir_all(ws.join("issues/i01")).unwrap();
        assert!(matches!(
            locate_for_undefer(ws, "i01"),
            DeferLocate::AlreadyDone(DeferKind::IssueDir)
        ));
    }

    /// 7.4 (unit-level) fs_move_unit carries gitignored markers (a dir
    /// unit) and preserves single-file form, creating the parent dir.
    #[test]
    fn fs_move_unit_carries_markers_and_creates_parent() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let from = ws.join("issues/i02-baz");
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join("issue.md"), "baz").unwrap();
        std::fs::write(from.join(".perma-stuck.json"), "{}").unwrap();
        let to = ws.join("deferred-issues/i02-baz");
        fs_move_unit(&from, &to).unwrap();
        assert!(!from.exists(), "source removed");
        assert!(to.join("issue.md").is_file());
        assert!(to.join(".perma-stuck.json").is_file(), "gitignored marker travelled");

        // single-file form preserved
        let f_from = ws.join("issues/i01-bar.md");
        std::fs::write(&f_from, "bar").unwrap();
        let f_to = ws.join("deferred-issues/i01-bar.md");
        fs_move_unit(&f_from, &f_to).unwrap();
        assert!(f_to.is_file() && !f_to.is_dir());
        assert!(!f_from.exists());
    }

    /// 7.5 lanes ignore deferred: a unit under deferred-changes/ is not in
    /// `list_pending`; a unit under deferred-issues/ is not in `list_ready`.
    #[test]
    fn lanes_ignore_deferred_units() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let (_td, paths) = crate::testing::test_daemon_paths();

        // deferred change → not pending
        let dchg = ws.join("deferred-changes/c01-foo");
        std::fs::create_dir_all(&dchg).unwrap();
        std::fs::write(dchg.join("proposal.md"), "## Why\n").unwrap();
        let pending = queue::list_pending(&paths, ws).unwrap();
        assert!(
            !pending.iter().any(|s| s == "c01-foo"),
            "deferred change must not be pending: {pending:?}"
        );

        // deferred issues (both forms) → not ready
        std::fs::create_dir_all(ws.join("deferred-issues")).unwrap();
        std::fs::write(ws.join("deferred-issues/i01-bar.md"), "## Report\nbar\n").unwrap();
        let didir = ws.join("deferred-issues/i02-baz");
        std::fs::create_dir_all(&didir).unwrap();
        std::fs::write(didir.join("issue.md"), "## Report\nbaz\n").unwrap();
        let ready = crate::lanes::issues::list_ready(ws).unwrap();
        assert!(
            !ready.iter().any(|s| s == "i01-bar" || s == "i02-baz"),
            "deferred issues must not be ready: {ready:?}"
        );
    }

    /// Build a seeded ControlState + repo whose workspace is the prepared
    /// git dir, with `auto_submit_pr` controllable.
    fn defer_state(
        td: &TempDir,
        workspace: &Path,
        origin: &Path,
        auto_submit_pr: bool,
        cancel: CancellationToken,
    ) -> (ControlState, RepositoryConfig) {
        let cfg_path = write_yaml(td.path(), BASE_YAML);
        let mut cfg = Config::load_from(&cfg_path).unwrap();
        // Keep the URL in `git@github.com:owner/repo.git` form so
        // `parse_repo_url` (used by the no-pr branch_url helper) succeeds.
        // Pushes go to the workspace's `origin` remote (pointed at the bare
        // file repo by `seed_defer_workspace`), NOT the URL, so the move
        // still lands without a real GitHub.
        let _ = origin;
        cfg.repositories[0].local_path = Some(workspace.to_path_buf());
        cfg.repositories[0].auto_submit_pr = auto_submit_pr;
        cfg.repositories[0].agent_branch = "agent-q".to_string();
        cfg.repositories[0].base_branch = "main".to_string();
        let state = seeded_state(cfg_path, &cfg, cancel);
        let repo = repo_from(&cfg);
        (state, repo)
    }

    /// 7.4 + 7.7: deferring a change moves it out of the lane to
    /// `deferred-changes/`, contents (incl. gitignored markers) preserved,
    /// on `agent_branch` (not base), and with `auto_submit_pr=false` yields
    /// the `branch_pushed_no_pr` mechanism.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn defer_change_moves_to_deferred_no_pr() {
        let (td, workspace, origin) = seed_defer_workspace();
        let cancel = CancellationToken::new();
        let (state, repo) = defer_state(&td, &workspace, &origin, false, cancel.clone());

        let req = json!({"action": "defer_unit", "url": repo.url, "slug": "c01-foo"});
        let resp = handle_defer_unit(&req, &state).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(resp["outcome"], "deferred", "resp: {resp}");
        assert_eq!(resp["mechanism"], "branch_pushed_no_pr", "resp: {resp}");
        assert_eq!(resp["branch"], "agent-q", "move lands on the agent branch: {resp}");

        // The move happened on the agent branch's working tree.
        assert!(
            !workspace.join("openspec/changes/c01-foo").exists(),
            "lane location removed"
        );
        let deferred = workspace.join("deferred-changes/c01-foo");
        assert!(deferred.join("proposal.md").is_file(), "moved to deferred-changes");
        assert!(
            deferred.join(".perma-stuck.json").is_file(),
            "gitignored marker travelled with the unit"
        );

        // Current branch is the agent branch, NOT base.
        let cur = TestCmd::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&workspace)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&cur.stdout).trim(),
            "agent-q",
            "the move commit is on the agent branch"
        );
        cancel.cancel();
    }

    /// 7.4: undefer is the exact inverse — it returns a deferred unit to
    /// its lane location. The handler resolves against a clean base tip, so
    /// the deferred state is seeded onto base first.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn undefer_returns_unit_to_lane() {
        let (td, workspace, origin) = seed_defer_workspace();
        // Seed base with the change already deferred (lane absent).
        std::fs::create_dir_all(workspace.join("deferred-changes")).unwrap();
        std::fs::rename(
            workspace.join("openspec/changes/c01-foo"),
            workspace.join("deferred-changes/c01-foo"),
        )
        .unwrap();
        git_run(&workspace, &["add", "-A"]);
        git_run(&workspace, &["commit", "-q", "-m", "pre-deferred c01-foo"]);
        git_run(&workspace, &["push", "-q", "origin", "main"]);

        let cancel = CancellationToken::new();
        let (state, repo) = defer_state(&td, &workspace, &origin, false, cancel.clone());

        let req = json!({"action": "undefer_unit", "url": repo.url, "slug": "c01-foo"});
        let resp = handle_undefer_unit(&req, &state).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(resp["outcome"], "resumed", "resp: {resp}");
        assert_eq!(resp["mechanism"], "branch_pushed_no_pr", "resp: {resp}");
        assert!(
            workspace.join("openspec/changes/c01-foo/proposal.md").is_file(),
            "undefer returns the unit to its lane"
        );
        assert!(
            !workspace.join("deferred-changes/c01-foo").exists(),
            "deferred location emptied by undefer"
        );
        cancel.cancel();
    }

    /// 7.4: deferring a single-file issue preserves its single-file form.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn defer_single_file_issue_preserves_form() {
        let (td, workspace, origin) = seed_defer_workspace();
        let cancel = CancellationToken::new();
        let (state, repo) = defer_state(&td, &workspace, &origin, false, cancel.clone());

        let req = json!({"action": "defer_unit", "url": repo.url, "slug": "i01-bar"});
        let resp = handle_defer_unit(&req, &state).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(resp["kind"], "issue", "resp: {resp}");
        let moved = workspace.join("deferred-issues/i01-bar.md");
        assert!(moved.is_file() && !moved.is_dir(), "single-file form preserved");
        assert!(!workspace.join("issues/i01-bar.md").exists());
        cancel.cancel();
    }

    /// 7.6: deferring an already-deferred slug is a no-op success — no
    /// commit, no PR. The handler resolves the slug against a clean base
    /// tip (reset to `origin/<base>`), so the already-deferred state must
    /// live on base: we commit the unit into `deferred-changes/` (lane
    /// location absent) on base, then assert a defer reports
    /// `already_deferred` with no `mechanism`/move and leaves the tree as-is.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn defer_already_deferred_is_noop_success() {
        let (td, workspace, origin) = seed_defer_workspace();
        // Move the change into deferred-changes/ on base and push to origin
        // so the post-reset base tree reflects the already-deferred state.
        std::fs::create_dir_all(workspace.join("deferred-changes")).unwrap();
        std::fs::rename(
            workspace.join("openspec/changes/c01-foo"),
            workspace.join("deferred-changes/c01-foo"),
        )
        .unwrap();
        git_run(&workspace, &["add", "-A"]);
        git_run(&workspace, &["commit", "-q", "-m", "pre-deferred c01-foo"]);
        git_run(&workspace, &["push", "-q", "origin", "main"]);

        let cancel = CancellationToken::new();
        let (state, repo) = defer_state(&td, &workspace, &origin, false, cancel.clone());

        let req = json!({"action": "defer_unit", "url": repo.url, "slug": "c01-foo"});
        let resp = handle_defer_unit(&req, &state).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(resp["outcome"], "already_deferred", "resp: {resp}");
        // No move was performed: no mechanism field, deferred dir still there.
        assert!(resp.get("mechanism").is_none(), "no-op performs no move: {resp}");
        cancel.cancel();
    }

    /// 7.7: with `auto_submit_pr` true (default), the PR-open path is taken
    /// rather than the branch_pushed_no_pr early return. There is no real
    /// GitHub remote, so the handler reaches `open_triage_pull_request` and
    /// fails THERE — proving the PR path (not the no-pr path) was selected.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn defer_with_auto_submit_pr_takes_pr_path() {
        let (td, workspace, origin) = seed_defer_workspace();
        let cancel = CancellationToken::new();
        let (state, repo) = defer_state(&td, &workspace, &origin, true, cancel.clone());

        // Serialize on the github-api-base test hook: with `auto_submit_pr` true
        // the defer handler reaches `open_triage_pull_request`, which reads the
        // process-wide override. Without this guard a concurrent test that has
        // installed the override (e.g. the confirmed-rollback e2e test, same
        // `owner/repo` path) would receive this test's PR-create POST on its own
        // mockito server, tripping that test's `expect(0)`. See `test_hooks::lock`.
        let _hook = crate::polling_loop::test_hooks::lock();

        let req = json!({"action": "defer_unit", "url": repo.url, "slug": "c01-foo"});
        let resp = handle_defer_unit(&req, &state).await;
        // The move + push succeed; the PR open fails (no GitHub). The error
        // names the PR step — NOT a branch_pushed_no_pr success — proving
        // the auto_submit_pr=true branch was taken.
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        let err = resp["error"].as_str().unwrap_or("");
        assert!(
            err.contains("opening defer PR"),
            "auto_submit_pr=true must reach the PR-open path: {err}"
        );
        assert_ne!(
            resp["mechanism"], "branch_pushed_no_pr",
            "must NOT take the no-pr early return"
        );
        cancel.cancel();
    }

    /// 7.2/error: a not-found slug returns a clear error and performs no move.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn defer_not_found_is_error() {
        let (td, workspace, origin) = seed_defer_workspace();
        let cancel = CancellationToken::new();
        let (state, repo) = defer_state(&td, &workspace, &origin, false, cancel.clone());

        let req = json!({"action": "defer_unit", "url": repo.url, "slug": "nonesuch"});
        let resp = handle_defer_unit(&req, &state).await;
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        assert!(
            resp["error"].as_str().unwrap().contains("no change or issue"),
            "resp: {resp}"
        );
        cancel.cancel();
    }

    /// 7.8 preempt + serialize: deferring while a pass is in flight (busy
    /// marker held by a live process with a recorded change) returns Busy
    /// and does NOT move the unit. Mirrors the rollback handler's
    /// preempt-and-serialize behaviour: a workspace-mutating op cannot
    /// barge in on an ambiguous in-flight holder.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn defer_preempts_and_serializes_against_pass() {
        let (td, workspace, origin) = seed_defer_workspace();
        let cancel = CancellationToken::new();
        let (state, repo) = defer_state(&td, &workspace, &origin, false, cancel.clone());

        // Install an iteration_cancel token so the preempt has a pass to
        // cancel, and HOLD a real busy marker (acquired by THIS live
        // process) so the concurrent acquire classifies SkipFreshInProgress
        // (a live in-flight holder it must not barge in on) → Busy. Holding
        // a real guard is deterministic across platforms (no reliance on
        // /proc/<pid>/comm classification).
        let token = install_iteration_cancel(&state, &repo.url);
        let _held = match busy_marker::try_acquire(&state.paths, &workspace, &repo.url, 600).unwrap() {
            busy_marker::AcquireOutcome::Acquired(g) => g,
            other => panic!(
                "fixture must acquire the marker first, got {}",
                match other {
                    busy_marker::AcquireOutcome::SkipAmbiguous(_) => "Ambiguous",
                    busy_marker::AcquireOutcome::SkipFreshInProgress(_) => "Fresh",
                    busy_marker::AcquireOutcome::Acquired(_) => "Acquired",
                }
            ),
        };
        // Zero the drain timeout so the bounded wait returns immediately.
        let mut cfg2 = (*state.last_config.load_full()).clone();
        cfg2.executor.wipe_drain_timeout_secs = 0;
        state.last_config.store(Arc::new(cfg2));

        let req = json!({"action": "defer_unit", "url": repo.url, "slug": "c01-foo"});
        let resp = handle_defer_unit(&req, &state).await;

        // The preempt fired the iteration cancel (it observed the pass).
        assert!(token.is_cancelled(), "in-flight pass must be cancelled by the preempt");
        // The busy holder is ambiguous → Busy error, NO workspace mutation.
        assert_eq!(resp["ok"], serde_json::Value::Bool(false), "resp: {resp}");
        assert!(
            resp["error"].as_str().unwrap().to_lowercase().contains("busy"),
            "ambiguous in-flight holder must surface a busy error: {resp}"
        );
        // The unit was NOT moved (the op never got past the marker acquire).
        assert!(
            workspace.join("openspec/changes/c01-foo").exists(),
            "no move occurs while a pass holds the workspace"
        );
        assert!(
            !workspace.join("deferred-changes/c01-foo").exists(),
            "deferred location must be empty after a refused (busy) defer"
        );
        drop(_held); // release the held marker (RAII) at end of test
        cancel.cancel();
    }

    /// 7.8 (positive): deferring while a pass is in flight whose marker is
    /// recoverable (dead pid) preempts the pass, acquires the marker, and
    /// completes the move — the marker is held across the whole op. Asserts
    /// the move succeeded AND the iteration cancel fired.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn defer_preempts_recoverable_pass_then_moves() {
        let (td, workspace, origin) = seed_defer_workspace();
        let cancel = CancellationToken::new();
        let (state, repo) = defer_state(&td, &workspace, &origin, false, cancel.clone());

        let token = install_iteration_cancel(&state, &repo.url);
        // Dead-pid marker → recoverable; after preempt the acquire yields
        // Acquired and the move proceeds.
        write_marker(&state.paths, &workspace, 999_999, "claude", 5, "c01-foo");
        busy_marker::write_subprocess_marker(&state.paths, &workspace, 4242).unwrap();
        let mut cfg2 = (*state.last_config.load_full()).clone();
        cfg2.executor.wipe_drain_timeout_secs = 0;
        state.last_config.store(Arc::new(cfg2));

        let req = json!({"action": "defer_unit", "url": repo.url, "slug": "c01-foo"});
        let resp = handle_defer_unit(&req, &state).await;

        assert!(token.is_cancelled(), "the recoverable in-flight pass is cancelled");
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(resp["outcome"], "deferred", "resp: {resp}");
        assert!(
            workspace.join("deferred-changes/c01-foo/proposal.md").is_file(),
            "the move completes after preempting the recoverable pass"
        );
        // The op's busy guard was released on return (marker gone).
        assert!(
            !busy_marker::marker_path(&state.paths, &workspace).exists(),
            "busy marker released after the op completes"
        );
        cancel.cancel();
    }

    /// 9.2 detect-before-preempt: an already-deferred `defer` issued while
    /// a pass is in flight is a READ-ONLY no-op — it does NOT fire the
    /// iteration-cancel token and does NOT acquire the busy marker (so a
    /// typo'd re-defer cannot disrupt active work). We seed the
    /// already-deferred state in the working tree (the no-op path inspects
    /// the live tree without a reset), install an iteration-cancel token,
    /// and HOLD a live busy marker; the handler must leave both untouched.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn defer_already_deferred_does_not_preempt_in_flight_pass() {
        let (td, workspace, origin) = seed_defer_workspace();
        // Move the change into deferred-changes/ in the WORKING TREE so the
        // read-only pre-flight detection classifies it as already-deferred.
        std::fs::create_dir_all(workspace.join("deferred-changes")).unwrap();
        std::fs::rename(
            workspace.join("openspec/changes/c01-foo"),
            workspace.join("deferred-changes/c01-foo"),
        )
        .unwrap();

        let cancel = CancellationToken::new();
        let (state, repo) = defer_state(&td, &workspace, &origin, false, cancel.clone());

        // A pass is in flight: an iteration-cancel token is installed AND a
        // live busy marker is held (acquired by THIS process). If the
        // handler preempted it would fire the token; if it acquired it would
        // need this marker released.
        let token = install_iteration_cancel(&state, &repo.url);
        let held = match busy_marker::try_acquire(&state.paths, &workspace, &repo.url, 600).unwrap() {
            busy_marker::AcquireOutcome::Acquired(g) => g,
            other => panic!(
                "fixture must acquire the marker first, got {}",
                match other {
                    busy_marker::AcquireOutcome::SkipAmbiguous(_) => "Ambiguous",
                    busy_marker::AcquireOutcome::SkipFreshInProgress(_) => "Fresh",
                    busy_marker::AcquireOutcome::Acquired(_) => "Acquired",
                }
            ),
        };

        let req = json!({"action": "defer_unit", "url": repo.url, "slug": "c01-foo"});
        let resp = handle_defer_unit(&req, &state).await;

        // It is a no-op success, NOT a busy error or a move.
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "resp: {resp}");
        assert_eq!(resp["outcome"], "already_deferred", "resp: {resp}");
        assert!(resp.get("mechanism").is_none(), "no-op performs no move: {resp}");
        // No preempt occurred: preempted_change is null AND the in-flight
        // pass's cancel token was NEVER fired.
        assert!(resp["preempted_change"].is_null(), "no preempt for a no-op: {resp}");
        assert!(
            !token.is_cancelled(),
            "an already-deferred no-op must NOT cancel the in-flight pass"
        );
        // The handler acquired no marker of its own: the marker still
        // belongs to the held guard (which we drop here). It exists because
        // WE hold it, not because the handler took it.
        assert!(
            busy_marker::marker_path(&state.paths, &workspace).exists(),
            "the held (fixture) marker is untouched by the no-op handler"
        );
        drop(held);
        // With the fixture guard dropped the marker is gone — proof the
        // handler never acquired a second/replacement marker of its own.
        assert!(
            !busy_marker::marker_path(&state.paths, &workspace).exists(),
            "no handler-acquired marker remains after the fixture guard drops"
        );
        cancel.cancel();
    }
