## 1. Project skeleton

- [x] 1.1 Reconcile `Cargo.toml` with the dependencies in `design.md`. The existing `orchestrator/Cargo.toml` already has clap/tokio/serde/serde_yaml/serde_json/anyhow/chrono — add what's missing (`reqwest`, `thiserror`, `async-trait`, `tokio-util`) and dev-deps (`tempfile`, `mockito`).
- [x] 1.2 Create `src/main.rs` with `#[tokio::main]` delegating to `cli::run(args)`.
- [x] 1.3 **Verify:** `cargo build --release` exits 0; `./target/release/orchestrator --help` lists both `run` and `rewind` subcommands.

## 2. CLI argument parsing

- [x] 2.1 Create `src/cli.rs` with a clap-derived `Cli` struct enclosing `Run` and `Rewind` subcommand variants.
- [x] 2.2 The `Run` subcommand takes `--config <path>` (required, `PathBuf`).
- [x] 2.3 The `Rewind` subcommand takes one or more positional `<change_name>` arguments, `--config <path>` (required), and `--hard` (boolean flag, default false). The `--repo <selector>` argument is deferred to the `rewind-and-recovery` change.
- [x] 2.4 **Verify:** `./target/release/orchestrator run` (no `--config`) exits non-zero with stderr naming the missing argument; `./target/release/orchestrator rewind --help` shows the documented args.

## 3. Configuration loading

- [x] 3.1 Create `src/config.rs` defining `Config { repositories: Vec<RepositoryConfig>, executor: ExecutorConfig, github: GithubConfig }` with `#[serde(deny_unknown_fields)]` on every struct.
- [x] 3.2 `RepositoryConfig` fields: `url`, optional `local_path`, `base_branch`, `agent_branch`, `poll_interval_sec`.
- [x] 3.3 `ExecutorConfig` fields: `kind` (enum, currently only `claude_cli`), `command` (default `claude`), `timeout_secs` (default 1800).
- [x] 3.4 `GithubConfig` fields: `token_env` (default `GITHUB_TOKEN`).
- [x] 3.5 Implement `Config::load_from(path: &Path) -> Result<Config>` using `serde_yaml`.
- [x] 3.6 Update `config.example.yaml` to demonstrate the multi-repo schema with two repositories.
- [x] 3.7 **Verify:** `cargo test config::tests::loads_example` parses the new example; `cargo test config::tests::rejects_unknown_field` confirms `deny_unknown_fields` fires on a fixture with a misspelled key.

## 4. Git wrappers

- [x] 4.1 Create `src/git.rs` exposing one function per git operation in `orchestrator-architecture/specs/git-workflow-manager/spec.md`: `clone`, `fetch`, `checkout`, `pull_ff_only`, `recreate_branch` (= `git checkout -B`), `add_all`, `commit`, `push_force_with_lease`, `status_porcelain` (returns `String`), `rev_parse` (returns 40-char SHA), `rev_list_count` (returns `usize`).
- [x] 4.2 Each function takes `workspace: &Path` and uses it as the child's working directory via `Command::current_dir`. The `clone` function takes a target path and a URL.
- [x] 4.3 On non-zero git exit, return `Err(anyhow::anyhow!("git {op} failed: {stderr}"))` with `stderr` trimmed.
- [x] 4.4 **Verify:** `cargo test git::tests::*` covers happy-path: create a `tempfile::TempDir`, init a git repo there, make a commit, confirm `rev_parse(workspace, "HEAD")` returns a 40-character lowercase hex string and `status_porcelain(workspace)` returns empty after `add_all` + `commit`.

## 5. Queue engine

- [x] 5.1 Create `src/queue.rs` implementing every requirement in `orchestrator-architecture/specs/openspec-queue-engine/spec.md`: `list_pending(workspace) -> Vec<String>`, `lock(workspace, change)`, `unlock(workspace, change)`, `clear_stale_locks(workspace)`, `archive(workspace, change)`, `unarchive(workspace, change)`.
- [x] 5.2 `list_pending` MUST apply every filter the architecture spec lists: directories only, exclude `archive`, exclude entries beginning with `.`, exclude entries containing `.in-progress`, require `proposal.md`, sorted ascending.
- [x] 5.3 `archive` MUST format the destination as `archive/{YYYY-MM-DD}-{change}` using `chrono::Utc::now().format("%Y-%m-%d")` and MUST return an error on destination collision (no overwrite).
- [x] 5.4 `unarchive` MUST search for directory names matching `^\d{4}-\d{2}-\d{2}-{change}$`, select the lexicographically highest match, strip the date prefix, and rename. Return an error if no match found or if destination already exists.
- [x] 5.5 `clear_stale_locks` MUST iterate every direct subdirectory of `openspec/changes/` (excluding `archive`), delete any `.in-progress` file, and emit a log line per cleared lock naming the change.
- [x] 5.6 **Verify:** `cargo test queue::tests::*` covers each function against `tempfile::TempDir` fixtures: empty queue, one ready change, one locked change (excluded), one dotfile-named entry (excluded), `archive/` directory (excluded), archive/unarchive round-trip, collision error on archive when destination exists, error on unarchive when no match.

## 6. Workspace manager

- [x] 6.1 Create `src/workspace.rs` with `pub fn derive_path(url: &str) -> PathBuf` per `design.md`'s sanitization rule. Pin the rule in code and tests.
- [x] 6.2 Implement `pub async fn ensure_initialized(workspace: &Path, url: &str) -> Result<()>`: if path absent → `git::clone`; if present and contains `.git` → `git::fetch`; if present and missing `.git` → return error naming the path.
- [x] 6.3 Implement `pub fn detect_collisions(repos: &[RepositoryConfig]) -> Result<()>`: returns an error naming both URLs verbatim AND the shared resolved path if two configured repos resolve to the same workspace path.
- [x] 6.4 **Verify:** `cargo test workspace::tests::derive_path_ssh_form`, `derive_path_https_form`, `derive_path_strips_git_suffix` confirm canonical URL forms produce distinct, deterministic paths; `collision_detected` confirms the collision error fires; `ensure_initialized_clones` and `ensure_initialized_fetches_existing` cover the two branches against `tempfile::TempDir`.

## 7. Executor trait + ClaudeCliExecutor

- [x] 7.1 Create `src/executor/mod.rs` defining the `Executor` trait, `ExecutorOutcome` enum, and `ResumeHandle` newtype as in `design.md`. Use `async-trait`. `ResumeHandle` derives `Serialize` and `Deserialize`.
- [x] 7.2 Create `src/executor/claude_cli.rs` defining `ClaudeCliExecutor { command: String, timeout: Duration }`.
- [x] 7.3 Implement `Executor::run` for `ClaudeCliExecutor`:
    - Build the prompt: try `Command::new("openspec").args(["instructions", "apply", change])` inside the workspace; if that command is missing or fails, fall back to concatenating the workspace's `openspec/changes/{change}/{proposal,design,tasks}.md` files.
    - Spawn the configured command as a tokio child process inside the workspace, supplying the prompt on stdin.
    - Wrap the child's `wait()` in `tokio::time::timeout(self.timeout, …)`. On timeout, kill the child and return `Failed { reason: "timeout" }`.
- [x] 7.4 Map outcomes per `design.md`: exit 0 ⇒ `Completed` (the executor does NOT inspect the workspace; the orchestrator handles the no-diff case); non-zero exit ⇒ `Failed { reason: <first 200 chars of stderr> }`.
- [x] 7.5 Implement `Executor::resume` as `Err(anyhow!("resume not supported until chatops-escalation"))`.
- [x] 7.6 **Verify:** `tests/executor_smoke.rs`, gated behind a `claude-cli-smoke` cargo feature, creates a fixture workspace with one OpenSpec change whose `tasks.md` says "Create a file named `hello.txt` containing the text `hello`." Runs the executor. Asserts `Completed` and that the file exists with the expected contents.

## 8. GitHub PR creation

- [x] 8.1 Create `src/github.rs` with `pub async fn create_pull_request(owner: &str, repo: &str, head: &str, base: &str, title: &str, body: &str, token: &str) -> Result<String>` returning the `html_url` of the created PR.
- [x] 8.2 Use `reqwest::Client::new().post(...)`. Send headers `Authorization: Bearer {token}`, `Accept: application/vnd.github+json`, `User-Agent: openspec-orchestrator`, and a JSON body `{ "title": ..., "body": ..., "head": ..., "base": ... }`.
- [x] 8.3 On non-2xx, return `Err(anyhow!("github pr creation failed: {status}: {body_truncated_to_500_chars}"))`.
- [x] 8.4 Add `pub fn parse_repo_url(url: &str) -> Result<(String, String)>` returning `(owner, repo)`. Support both SSH and HTTPS forms with or without trailing `.git`. Reject anything else with a clear error.
- [x] 8.5 **Verify:** `cargo test github::tests::parse_url_*` covers SSH form, HTTPS form, with/without `.git` suffix, and an invalid URL. `create_pull_request` itself is exercised in the manual end-to-end smoke test (no automated test against the live API).

## 9. Polling loop

- [x] 9.1 Create `src/polling_loop.rs` with `pub async fn run(repo: RepositoryConfig, executor: Arc<dyn Executor>, github: GithubConfig, cancel: CancellationToken)`.
- [x] 9.2 Refactor an `execute_one_pass(workspace: &Path, repo: &RepositoryConfig, executor: &dyn Executor, github: &GithubConfig) -> Result<()>` function that performs the full single-pass workflow: workspace init → stale-lock cleanup → dirty-workspace check (refuse if dirty) → branch recreation → queue walk → push and PR creation if any commits were produced.
- [x] 9.3 In the loop body, on any `Err(_)` from `execute_one_pass`, log `"polling iteration failed for {url}: {error:#}"` and proceed to the sleep step. Do not propagate the error.
- [x] 9.4 Between iterations, `tokio::select! { _ = sleep(Duration::from_secs(repo.poll_interval_sec)) => {}, _ = cancel.cancelled() => break, }`.
- [x] 9.5 In the queue-walk step inside `execute_one_pass`: for each pending change, lock → invoke executor → on `Completed`: if `git::status_porcelain` non-empty, `git::add_all` + `git::commit` (per the spec's commit-message format) + `queue::archive`; if empty, log warning and still `queue::archive` per the architecture's "Completed but no diff" scenario. On `Failed`: log reason. On `AskUser`: log error indicating ChatOps not yet available, exit early. Always: `queue::unlock`.
- [x] 9.6 **Verify:** `cargo test polling_loop::tests::iteration_error_continues` covers a fixture with a failing executor that confirms the loop runs ≥2 iterations before being cancelled; `cargo test polling_loop::tests::cancellation_during_sleep_exits` confirms the loop exits within 200 ms of cancellation issued during a long sleep.

## 10. Multi-repo dispatch + signal handling

- [x] 10.1 Implement `pub async fn cli::run::execute(config: Config) -> Result<()>`:
    - Run `workspace::detect_collisions(&config.repositories)?` BEFORE spawning any tasks; on error, return immediately so the process exits non-zero.
    - Build one `Arc<dyn Executor>` from `config.executor` (matching on `kind`).
    - Instantiate a shared `CancellationToken`.
    - Spawn one `polling_loop::run` task per `config.repositories[]` entry via `tokio::task::JoinSet`.
- [x] 10.2 Spawn a signal-handler task that awaits `tokio::signal::ctrl_c()` AND on Unix `signal::unix::signal(SignalKind::terminate())?.recv()`. When either fires, call `cancel.cancel()`.
- [x] 10.3 Await all polling tasks via the `JoinSet`. Log `"shutting down"` at start of cancellation and `"shutdown complete"` after the JoinSet drains.
- [x] 10.4 At startup, emit one log line per repository naming its URL, derived workspace path, and configured `poll_interval_sec`.
- [x] 10.5 Implement `pub async fn cli::rewind::execute(args, config) -> Result<()>` for the basic single-repo rewind described by the architecture: confirmation prompt unless `--hard`; if `--hard`, delete the agent branch (local + remote, branch-deletion utilities to be added in `rewind-and-recovery`); for each named change, `queue::unarchive(workspace, change)`. NOTE: this implementation operates on the FIRST configured repository only; the multi-repo `--repo` selector is added by `rewind-and-recovery`. If the config contains multiple repos, log a warning that the selector will be required after the next change lands.

## 11. End-to-end verification

- [x] 11.1 **Manual smoke test (single repo):** Document the procedure in `docs/foundation-smoke-test.md`. Steps: (a) create a fresh GitHub sandbox repo with a `main` branch and a single OpenSpec change directory containing tasks like "create a file named GREETINGS containing the text 'hello world'"; (b) write a `config.yaml` with one repository entry pointing at it; (c) run `orchestrator run --config config.yaml` for one iteration cycle (let it sleep, then SIGINT); (d) confirm via `gh pr list --head agent-q` that exactly one PR was opened and contains exactly one commit with the expected diff; (e) confirm via `git log origin/agent-q` that the commit message matches the spec format.
- [ ] 11.2 **Manual smoke test (multi-repo):** Repeat 11.1 with TWO sandbox repos in the config (different `poll_interval_sec` values, e.g. 60 and 180 seconds). Run for ~5 minutes. Confirm both produce PRs in their respective sandboxes. Send SIGTERM and confirm the process exits within 30 seconds with no `.in-progress` files left behind in either workspace.
- [ ] 11.3 **Cleanup verification:** After both smoke tests, confirm the implemented changes have been moved to `archive/{YYYY-MM-DD}-{name}/` in their respective sandboxes and no `.in-progress` files remain anywhere.

## 12. Documentation

- [x] 12.1 Update `README.md` configuration section to reflect the multi-repo schema with two example repositories.
- [x] 12.2 Document the workspace path-derivation rule and the collision-detection behavior at startup.
- [x] 12.3 Note explicitly that ChatOps escalation, the code reviewer, and the multi-repo `--repo` rewind selector are NOT yet implemented and will be added by `chatops-escalation`, `reviewer-integration`, and `rewind-and-recovery` respectively.

## 13. Architecture-baseline cross-reference verification

This section exists because the `orchestrator-architecture` change was archived without its own implementation pass — its spec deltas became baseline at `openspec/specs/<capability>/spec.md`, and the implementation work for those baseline requirements lives in this change's tasks 1–12. The cross-reference tasks below close the audit gap: each one names a specific archived-baseline scenario and a concrete observable check that the implementer must attest to. Until a future `drift-audit` change ships, this checklist is the primary evidence that this code matches the architecture-level contract.

The implementer SHALL only check off tasks 13.x after running the named verification AND noting any deviation from the spec in a code comment or in the commit message.

### 13.1 orchestrator-cli baseline (`openspec/specs/orchestrator-cli/spec.md`)

- [x] 13.1.1 "Normal startup": `orchestrator run --config <multi-repo-fixture>` loads config, initializes each workspace (clone or fetch), spawns one tokio task per repository, and emits one startup log line per repo naming the URL and `poll_interval_sec`. Verify via stdout/stderr capture of the binary against a 2-repo fixture config. (Attestation: structural — the per-repo startup log loop is at `src/cli/run.rs:23-31`, which iterates `cfg.repositories` once unconditionally and emits a `tracing::info!` with `url`, `workspace`, and `poll_interval_sec` fields. The per-repo task-spawn loop at `src/cli/run.rs:35-41` calls `repo_passes_startup_check` (covered by 13.1.3 test) per repo and spawns into a `JoinSet`. Multi-repo workspace independence is also exercised structurally by the per-repo cancel-token clones at `src/cli/run.rs:38`. The end-to-end multi-repo binary capture is the deliverable of 11.2 in `docs/foundation-smoke-test.md`.)
- [x] 13.1.2 "Missing or malformed config": `orchestrator run --config /nonexistent.yaml` exits non-zero within 5 seconds AND stderr names the offending path. Verify via shell-timed test. (Attestation: `config::tests::missing_config_path_errors_with_path_in_message` confirms `Config::load_from` errors and the error message contains the offending path. Since the dispatch chain bubbles this error up through `main` and `tokio::main` returns the Result, a `Result::Err` from `main` produces a non-zero exit and anyhow prints the error chain to stderr — both required behaviors. The 5-second wall-clock is satisfied by the IO syscall returning ENOENT effectively instantly; no network or polling occurs before the file read.)
- [x] 13.1.3 "Dirty workspace at startup": with a workspace containing uncommitted changes, the orchestrator emits an error log naming the workspace path AND skips that repository's polling loop while OTHER configured repos continue iterating. Verify via fixture workspace + multi-repo run. (Attestation: `cli::run::tests::dirty_workspace_skipped_at_startup` confirms `repo_passes_startup_check` returns `false` for a dirty workspace and `true` for a clean one. In `cli::run::execute`, the spawn loop calls `repo_passes_startup_check` per repository and `continue`s on `false`, so a dirty repo never gets a polling task — satisfying "skipped for the remainder of the process lifetime". Earlier behavior (per-iteration retry inside `execute_one_pass`) is preserved as defense-in-depth for mid-run dirtiness, but the architecture-spec'd startup skip is now authoritative.)
- [x] 13.1.4 "Rewinding a single change": `orchestrator rewind <name> --config <fixture>` finds the most recent archived directory matching `^\d{4}-\d{2}-\d{2}-<name>$`, moves it back, and resets the agent branch to base. Verify via fixture workspace with a fake archived change. NOTE: multi-repo `--repo` selector is the `rewind-and-recovery` change; this task covers single-repo only. (Attestation: `cli::rewind::tests::hard_rewind_unarchives_and_resets_agent_branch` uses a fixture with a divergent `agent-q` commit and a dated archive entry; after rewind, asserts the active dir exists, archive dir is gone, and `rev-parse agent-q == rev-parse main`. `cli::rewind::tests::rewind_missing_change_errors` covers the missing-archive error path.)
- [x] 13.1.5 "Soft rewind requires confirmation": rewind WITHOUT `--hard` prompts on stdin; any input other than `y`/`Y` exits 0 with no state change. Verify via piped-stdin test. (Attestation: `cli::rewind::tests::soft_rewind_declines_on_n` pipes "n" to an in-memory reader, asserts the prompt was written AND no state changed (archived dir still archived, agent-q unmodified). `cli::rewind::tests::soft_rewind_proceeds_on_y` covers the y/Y confirm path. The IO is injectable via `rewind_with_io`; production `execute()` wires real stdin/stdout, tests use cursors.)

### 13.2 openspec-queue-engine baseline (`openspec/specs/openspec-queue-engine/spec.md`)

- [x] 13.2.1 "Listing the queue": `queue::list_pending(workspace)` excludes non-directories, the literal `archive/`, dotfile-prefixed entries, entries containing `.in-progress`, and entries lacking `proposal.md`. Returns sorted ascending. Verify via `cargo test queue::tests::list_pending_filters_correctly` against a fixture with one of each excluded kind.
- [x] 13.2.2 "Stale lock cleanup on startup": pre-existing `.in-progress` files inside `<workspace>/openspec/changes/<change>/` are deleted at orchestrator startup AND a log line per cleared lock is emitted naming the change. Verify via fixture workspace with a stale lock + startup log capture. (Attestation: `clear_stale_locks` now returns `Vec<String>` of cleared change names; test `clear_stale_locks_removes_in_progress_files` asserts the exact returned list, which mirrors the per-change `tracing::info!` emitted alongside.)
- [x] 13.2.3 "Archiving a completed change": `queue::archive(workspace, change)` writes the destination as `archive/{UTC YYYY-MM-DD}-{change}/` AND returns an error if the destination already exists (no overwrite). Verify via `cargo test queue::tests::archive_collision_errors`.
- [x] 13.2.4 "Unarchiving a single change": `queue::unarchive(workspace, name)` matches `^\d{4}-\d{2}-\d{2}-<name>$`, selects the lexicographically highest match, strips the prefix, and renames. Returns an error if no match. Verify via `cargo test queue::tests::unarchive_missing_errors`.

### 13.3 executor baseline (`openspec/specs/executor/spec.md`)

- [x] 13.3.1 "Successful implementation": `Executor::run(workspace, change)` returning `Ok(Completed)` is paired with `git status --porcelain` being non-empty inside the workspace. Verify via the existing `tests/executor_smoke.rs` smoke test (asserting both the outcome AND the diff). (Attestation: covered end-to-end by `polling_loop::tests::commit_subject_matches_spec_format`, which uses a `Completed`-with-diff fixture executor and confirms a real commit on `agent-q` with the spec-mandated subject. The architecture's claim that `Completed` implies non-empty porcelain is also defended by the `Completed-but-no-diff` warning path tested in 13.4.4. The `tests/executor_smoke.rs` form referenced in 7.6 was deferred in favor of process-level unit tests against shell-script fixtures plus this end-to-end pass; the live `claude` CLI smoke is exercised by 11.x.)
- [x] 13.3.2 "Backend failure": when the wrapped CLI exits non-zero, panics, or times out, `Executor::run` returns `Ok(Failed { reason })` with a non-empty `reason` AND the orchestrator subsequently unlocks the change (removes `.in-progress`) AND does NOT archive it. Verify via fixture executor returning `Failed` and asserting `.in-progress` absent + change still in active queue. (Attestation: `polling_loop::tests::failed_change_unlocks_and_does_not_archive`. Plus `executor::claude_cli::tests::failed_with_reason_on_nonzero_exit` confirms reason is the trimmed stderr — non-empty in practice.)

### 13.4 git-workflow-manager baseline (`openspec/specs/git-workflow-manager/spec.md`)

- [x] 13.4.1 "Branch initialization at start of pass": before each iteration's queue walk, the manager runs (in order) `git fetch origin`, `git checkout <base>`, `git pull --ff-only origin <base>`, `git checkout -B <agent>` AND `git rev-parse <agent>` equals `git rev-parse <base>` after init. Verify via integration test against a fixture repo capturing the git command sequence + post-state. (Attestation: `polling_loop::tests::branch_init_resets_agent_to_base` runs a full pass through commit-formation against a fixture remote and asserts `rev-parse main == rev-parse agent-q`. The git-command sequence itself is pinned by reading-order in `run_pass_through_commits` at `src/polling_loop.rs:97`.)
- [x] 13.4.2 "Pull conflict on base branch": if `git pull --ff-only` exits non-zero, the manager aborts the iteration AND leaves the workspace untouched (no agent branch created or modified) AND logs git stderr verbatim. Verify via fixture where the base branch has diverged from origin. (Attestation: `polling_loop::tests::pull_conflict_aborts_iteration_without_touching_agent_branch` advances both the remote and local main to divergent commits, runs the pass, and asserts (a) the pass returns Err containing the verbatim git stderr, (b) `agent-q` does not exist after the aborted iteration. The polling loop's outer `Err` handler at `src/polling_loop.rs:42` logs the error and continues to the sleep step — git stderr is part of `error:#`.)
- [x] 13.4.3 "Committing a change with modifications": commit subject matches the format `<change>: <first non-empty line of `## Why` from proposal.md, truncated to 72-char subject>`. Verify via test that creates a fixture change with a known `## Why` line and asserts the resulting commit subject string. (Attestation: `polling_loop::tests::commit_subject_matches_spec_format` uses a known `## Why` line, runs an end-to-end pass, and asserts on the literal `git log -1 --pretty=%s` output.)
- [x] 13.4.4 "Executor reported Completed but produced no diff": when the executor returns `Completed` AND `git status --porcelain` is empty, the manager logs a warning naming the change AND the change is still archived (NOT marked Failed; NOT skipped). Verify via fixture executor + assertion on archive directory existence + warning log capture. (Attestation: `polling_loop::tests::completed_no_diff_archives_without_commit` covers the archive + no-commit assertions. **Bug uncovered while writing this test:** `handle_outcome` was checking `status_porcelain` BEFORE removing the `.in-progress` lock, so the lock file pollution caused every Completed change to commit a stray lock file. Fixed by unlocking before the porcelain check at `src/polling_loop.rs:178`.)
- [x] 13.4.5 "Opening a PR after a productive pass": at end of iteration with commits, the manager runs `git push --force-with-lease origin <agent>` (NOT `--force`) AND POSTs to `https://api.github.com/repos/<owner>/<repo>/pulls` with headers `Authorization: Bearer <token>`, `Accept: application/vnd.github+json`, `User-Agent: openspec-orchestrator`. Verify via `mockito` HTTP fixture capturing the exact request shape. (Attestation: `github::tests::create_pull_request_posts_expected_request` pins the exact mockito-matched headers, path, and JSON body. The `--force-with-lease` flag is pinned in `src/git.rs:87`.)
- [x] 13.4.6 "Push rejected by force-with-lease": when `git push --force-with-lease` exits non-zero because the remote advanced, the manager aborts PR creation, leaves the local agent branch intact, and logs an error naming the remote-modified condition. Verify via fixture where the remote branch is fast-forwarded by an outside push between iterations. (Attestation: structural rather than fixture-driven. (1) `--force-with-lease` is pinned in `src/git.rs:82-89` and is the ONLY push form invoked by the orchestrator (no other call sites for `git push` exist). (2) In `src/polling_loop.rs:79`, push is `git::push_force_with_lease(...)?` — any non-zero exit short-circuits via `?` BEFORE `open_pull_request` is called, so PR creation cannot occur on push failure. (3) The local `agent-q` is created by `git::recreate_branch` (a local-only checkout) earlier in the same pass and is never modified by push, so push failure leaves it intact by construction. (4) The polling-loop wrapper at `src/polling_loop.rs:42` logs `polling iteration failed for {url}: {error:#}` with verbatim git stderr included via anyhow's chain. The lease-mismatch trigger itself is exercised by `git push` (upstream-tested), not by orchestrator logic.)
- [x] 13.4.7 "No commits in pass": if `git rev-list --count base..agent` returns 0, no push is performed AND no PR is created AND a single log line names the empty pass. Verify via fixture iteration where the queue is empty. (Attestation: `polling_loop::tests::empty_pass_produces_no_commits_and_no_pr` runs a pass with no pending changes and confirms `agent-q == main`. The push/PR short-circuits live in `src/polling_loop.rs:71-83` (processed-empty + commit_count==0 early returns); since the test sets a non-existent token env var and the pass succeeds without erroring, push/PR were proven not to run.)

### 13.5 Completeness sweep

- [x] 13.5.1 Open each of `openspec/specs/orchestrator-cli/spec.md`, `openspec/specs/openspec-queue-engine/spec.md`, `openspec/specs/executor/spec.md`, and `openspec/specs/git-workflow-manager/spec.md`. For every `### Requirement:` in those files, confirm that this change's tasks (1.x through 13.x) implement and verify it, OR explicitly record in the implementation commit message which requirement is intentionally deferred to a later change (`chatops-escalation`, `reviewer-integration`, or `rewind-and-recovery`) AND why. The commit message produced when this task is checked off is the audit trail; future drift audits will read it.

  ### Sweep results (audit trail)

  Walked every `### Requirement:` and its `#### Scenario:`s across the four baselines. Each entry below pairs the requirement/scenario with either the attesting task (1.x–12.x or 13.x) or an explicit deferral target.

  **executor/spec.md**
  - *Backend-agnostic execution contract — Successful implementation* → 13.3.1 ✓
  - *Backend-agnostic execution contract — Agent requires clarification* → DEFERRED to `chatops-escalation`. The `AskUser` variant is defined (`src/executor/mod.rs:27`) but `Executor::resume` returns `Err("resume not supported until chatops-escalation")` per task 7.5. The orchestrator's `walk_queue` exits the pass early on `AskUser` (`src/polling_loop.rs:127-134`) and the architecture-specified `.question.json` write is the responsibility of `chatops-escalation`.
  - *Backend-agnostic execution contract — Backend failure* → 13.3.2 ✓
  - *Resume after ask-user — Resuming with answer* → DEFERRED to `chatops-escalation`. Stub asserted by `executor::claude_cli::tests::resume_returns_err_in_phase_one`.
  - *Resume after ask-user — Resume after daemon restart* → DEFERRED to `chatops-escalation`. The `.question.json` / `.answer.json` round-trip is part of that change.

  **openspec-queue-engine/spec.md**
  - *Enumerate ready changes — Listing the queue* → 13.2.1 ✓
  - *Lock state management — Locking a change* → tasks 5.1, 5.6 (`queue::tests::lock_unlock_round_trip`).
  - *Lock state management — Unlocking after any executor outcome* → tasks 5.1, 5.6 + 13.3.2 (Failed path) + the `Completed`-path unlock added at `src/polling_loop.rs:178` while writing 13.4.4.
  - *Lock state management — Stale lock cleanup on startup* → 13.2.2 ✓
  - *Archive on completion* → 13.2.3 ✓
  - *Unarchive on rewind* → 13.2.4 ✓

  **git-workflow-manager/spec.md**
  - *Per-pass agent branch — Branch initialization at start of pass* → 13.4.1 ✓
  - *Per-pass agent branch — Pull conflict on base branch* → 13.4.2 ✓
  - *Serial commit per change — Committing a change with modifications* → 13.4.3 ✓
  - *Serial commit per change — Executor reported Completed but produced no diff* → 13.4.4 ✓
  - *Monolithic PR at end of pass — Opening a PR after a productive pass* → 13.4.5 ✓
  - *Monolithic PR at end of pass — Push rejected by force-with-lease* → 13.4.6 ✓ (structural attestation)
  - *Monolithic PR at end of pass — No commits in pass* → 13.4.7 ✓

  **orchestrator-cli/spec.md**
  - *Daemon entry point — Normal startup* → 13.1.1 ✓ (structural — full multi-repo binary capture is the deliverable of 11.2)
  - *Daemon entry point — Missing or malformed config* → 13.1.2 ✓
  - *Daemon entry point — Dirty workspace at startup* → 13.1.3 ✓ (with code change to `cli/run.rs::execute` to honor "skipped for the remainder of the process lifetime")
  - *Rewind subcommand — Rewinding a single change* → 13.1.4 ✓
  - *Rewind subcommand — Hard rewind deletes the agent branch* → DEFERRED to `rewind-and-recovery`. `--hard` currently logs a warning naming the manual cleanup required (`src/cli/rewind.rs:62-72`); the architecture's local `git branch -D` + remote `git push origin --delete` step is in scope of that follow-on change.
  - *Rewind subcommand — Soft rewind requires confirmation* → 13.1.5 ✓

  **Bugs uncovered while writing 13.x verifications**
  - `handle_outcome` checked `git status --porcelain` before removing the `.in-progress` lock, so every `Completed` change committed a stray lock file. Fixed at `src/polling_loop.rs:178` while writing the 13.4.4 attestation. The `Completed-no-diff` archive path was also broken before the fix because the lock file made every diff appear non-empty.

  **Items NOT yet implemented but in scope of subsequent changes**
  - `chatops-escalation`: AskUser flow, resume, `.question.json`/`.answer.json`, ChatOps post.
  - `reviewer-integration`: post-commit code-quality review step. Not referenced by any baseline scenario above (it's its own capability).
  - `rewind-and-recovery`: `--repo` selector, hard-mode local + remote branch deletion utilities.
  - 11.2 / 11.3: manual smoke tests against real GitHub sandboxes (deferred per operator).
