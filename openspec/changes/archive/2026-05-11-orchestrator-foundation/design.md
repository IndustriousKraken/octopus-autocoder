## Context

This is the first concrete implementation of the architecture specs. To keep the change reviewable, it covers only the core daemon: load config, initialize per-repo workspaces, run a polling loop per repo, on each iteration walk the queue and produce a monolithic PR. ChatOps escalation, the code reviewer, and the rewind subcommand's multi-repo selector are deliberately deferred to their own changes so each is small enough to audit. The implementation is multi-repo from day one because the production target is multi-repo and the cost of building it that way is small.

## Goals / Non-Goals

**Goals:**
- A `cargo build --release`-able binary whose `--help` lists `run` and `rewind`.
- Real, network-touching implementations of `git push --force-with-lease` and GitHub REST API PR creation. No placeholder print statements.
- A working `ClaudeCliExecutor` wrapping the `claude` CLI as a child process with timeout, stdout/stderr capture, documented outcome mapping.
- A working `workspace-manager` with deterministic per-repo path derivation and startup collision detection.
- A per-repo polling loop with iteration-level error tolerance and graceful SIGINT/SIGTERM shutdown.
- Unit tests for queue engine, config parser, workspace path derivation, git wrappers, polling-loop iterations against `tempfile::TempDir` fixtures, and mockito-tested GitHub PR request shape.
- An optional operator reference at `docs/foundation-smoke-test.md` (with a scaffolding helper at `scripts/scaffold-smoke-sandboxes.sh`) for hand-running the daemon against real GitHub sandboxes. Not a spec task: per project convention, live-service smoke happens organically when the orchestrator is wired to real repos and is not part of verification.

**Non-Goals:**
- ChatOps escalation. If the executor returns `AskUser` in this phase, the orchestrator logs an error, unlocks the change, and the iteration ends. The user re-runs after addressing the question manually. ChatOps is the next change.
- Code reviewer integration. PRs ship without a `## Code Review` section in this phase.
- Rewind multi-repo `--repo` selector. Phase implements basic rewind; the `rewind-and-recovery` change adds the selector logic.
- Per-repo executor configuration overrides. Executor is global; per-repo overrides are a future change.
- Distributed deployment (multiple orchestrator processes coordinating). Single-instance assumption from architecture is preserved.

## Decisions

- **Crate dependencies:** `clap` (derive), `tokio` (full), `serde` + `serde_yaml` + `serde_json`, `anyhow`, `thiserror`, `reqwest` (rustls-tls), `chrono` (UTC date for archive prefix), `async-trait`, `tokio-util` (`CancellationToken`); dev-deps `tempfile`, `mockito`.
- **Config schema (multi-repo from day one):**

  ```yaml
  repositories:
    - url: "git@github.com:owner/repo-a.git"
      local_path: "/tmp/workspaces/repo-a"  # optional, derived if omitted
      base_branch: "main"
      agent_branch: "agent-q"
      poll_interval_sec: 300
    - url: "git@github.com:owner/repo-b.git"
      base_branch: "dev"
      agent_branch: "agent-q"
      poll_interval_sec: 1800
  executor:
    kind: claude_cli
    command: claude
    timeout_secs: 1800
  github:
    token_env: GITHUB_TOKEN
  ```

  No `version` field — the production target is this shape. If a later change introduces an incompatible schema, that change can introduce a `version` field as part of its own delta.
- **Workspace path derivation:** if `local_path` is omitted, derive it as `/tmp/workspaces/<sanitized>` where `<sanitized>` is the URL with `git@<host>:`, `https://`, `ssh://` prefixes stripped, the `.git` suffix stripped, and any character outside `[A-Za-z0-9_-]` replaced with `_`. Deterministic so restarts reuse existing clones.
- **Collision detection:** any two configured repositories that resolve to the same workspace path (whether by explicit `local_path` or derivation) cause a fatal startup error before any polling task is spawned.
- **Executor trait shape:**

  ```rust
  #[async_trait]
  pub trait Executor: Send + Sync {
      async fn run(&self, workspace: &Path, change: &str) -> Result<ExecutorOutcome>;
      async fn resume(&self, handle: ResumeHandle, answer: &str) -> Result<ExecutorOutcome>;
  }

  pub enum ExecutorOutcome {
      Completed,
      AskUser { question: String, resume_handle: ResumeHandle },
      Failed { reason: String },
  }

  #[derive(Serialize, Deserialize)]
  pub struct ResumeHandle(pub serde_json::Value);
  ```

- **`ClaudeCliExecutor` outcome mapping:** trusts the underlying CLI's exit code as the success signal; does NOT inspect the workspace itself. The orchestrator handles a no-diff `Completed` per the architecture's `git-workflow-manager` "Executor reported Completed but produced no diff" scenario (warn + still archive).
  - Exit code 0 ⇒ `Completed`.
  - Non-zero ⇒ `Failed { reason: <first 200 chars of stderr> }`.
  - `tokio::time::timeout` fires ⇒ kill child, return `Failed { reason: "timeout" }`.
  - `AskUser` is NOT detected in this change. The ChatOps change retrofits detection.
  - `resume` returns `Err(anyhow!("resume not supported until chatops-escalation"))`.
- **Polling-loop error policy:** any error from a polling iteration is logged with the repo URL and the failed step, then the loop sleeps and continues. The loop only terminates on shutdown signal. Rationale: a single transient failure must not kill the daemon.
- **Signal handling:** `tokio::signal::ctrl_c` plus `signal::unix::signal(SignalKind::terminate())` trigger a shared `tokio_util::sync::CancellationToken`. Each polling task checks the token between iterations (via `tokio::select!` against its sleep). Mid-iteration cancellation is NOT performed — once an iteration starts, it runs to completion, accepting up to one `executor.timeout_secs` worth of shutdown delay in the worst case.
- **Iteration overrun:** if an iteration exceeds `poll_interval_sec`, the next iteration begins immediately without negative sleep. No two iterations within the same task run in parallel.
- **One task per repo, no shared workspace:** the architecture's serial-queue invariant per repo is preserved by giving each tokio task sole ownership of a workspace path; the collision detector enforces non-overlap.
- **GitHub PR creation:** `reqwest::Client` POSTs to `https://api.github.com/repos/{owner}/{repo}/pulls` with `Authorization: Bearer <token>`, `Accept: application/vnd.github+json`, `User-Agent: openspec-orchestrator`. Owner and repo parsed from `repository.url`, supporting SSH (`git@github.com:owner/repo.git`) and HTTPS (`https://github.com/owner/repo.git`) forms with or without `.git` suffix.

## Risks / Trade-offs

- **Risk:** A confused or lazy agent that exits 0 without producing a diff has its change archived (with a warning log) rather than retried.
  - **Mitigation:** Every no-diff archive emits a warning naming the change, so the situation is auditable. The reviewer-agent change can later add a "PR contains no commits attributable to archived change X" policy. Accepted risk consistent with the architecture spec.
- **Risk:** Persistent failures (misconfigured token, perpetually broken executor, network outage) are masked by polling-loop error tolerance.
  - **Mitigation:** Every iteration's error is logged with the repo URL and the failed step. No automatic alerting in this change; that is a later observability concern.
- **Risk:** Mid-iteration shutdown delay frustrates operators trying to redeploy quickly.
  - **Mitigation:** Worst case is one `executor.timeout_secs` (default 30 minutes). Lower the timeout if faster shutdown is required. `SIGKILL` remains an escape hatch.
- **Risk:** Workspace path derivation produces user-surprising paths.
  - **Mitigation:** The derived path is logged at startup ("watching repo X at workspace path Y"). Users can override with explicit `local_path`.
- **Risk:** This change is large enough that a single implementer agent produces uneven quality across modules.
  - **Mitigation:** The tasks.md is sectioned with explicit `Verify:` criteria per section. Each module has unit tests against fixture workspaces; section 13 cross-references every baseline Requirement to a concrete attestation. The reviewer-agent change (when it lands) will provide an additional quality backstop.
- **Risk:** Phase has no `AskUser` detection, so a Claude run that emits a clarification request as final text surfaces as a no-diff `Completed`.
  - **Mitigation:** Documented non-goal. ChatOps change retrofits detection.
