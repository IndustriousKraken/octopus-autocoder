## Context

The project requires automated processing of OpenSpec proposals across multiple repositories, in serial per repository. To keep the orchestrator small and durable, code-writing is delegated to a swappable executor backend, and the orchestrator owns only the workflow concerns: queue state, git operations, ChatOps escalation, and recovery. This design intentionally avoids reimplementing agentic primitives (LLM loops, tool dispatch, context management) that mature external tools already provide well.

## Goals / Non-Goals

**Goals:**
- One Rust binary that runs as a daemon and orchestrates the AI software factory.
- A serial queue per repository, reading from `openspec/changes/`.
- A backend-agnostic `Executor` trait that the orchestrator calls. Concrete implementations are provided by separate implementation changes.
- Local git operations via `std::process::Command`; remote operations (PR creation) via the GitHub REST API using `reqwest`.
- Asynchronous Slack escalation when the executor reports an `AskUser` outcome.
- Single-instance daemon assumption with explicit stale-lock cleanup on startup.

**Non-Goals:**
- No web dashboard.
- No automatic git merge-conflict resolution.
- The architecture-level specs do NOT name any specific executor backend. Concrete backends (claude-cli wrappers, OpenCode, custom MCP, future native loops) are introduced by separate implementation changes.
- The orchestrator does NOT implement an MCP server itself. If MCP tooling is desired, it is configured into whichever executor backend supports it.
- No support for multiple daemon instances against a single workspace. The single-instance assumption is load-bearing for the lock-cleanup-on-startup decision.

## Decisions

- **Language & Runtime:** Rust with `tokio`. Required for concurrent polling loops across repositories while sharing in-process state safely.
- **Executor abstraction:** A Rust trait with two async methods — `run(workspace, change) -> Result<ExecutorOutcome>` and `resume(handle, answer) -> Result<ExecutorOutcome>` — where `ExecutorOutcome` is an enum of `Completed`, `AskUser { question, resume_handle }`, or `Failed { reason }`. The orchestrator code is generic over this trait. Concrete backends are introduced by phase-specific changes; if multi-backend support proves over-engineered, the trait is small enough to inline with no architectural rewrite.
- **Git Branching Strategy:** Per-pass feature branch. Each polling pass branches from the configured `base_branch`, applies all ready changes serially with one commit per change, and opens one monolithic PR at the end of the pass. The agent branch is recreated each pass (`git checkout -B`); prior state is overwritten by design.
- **Queue State as Filesystem:** The file system (`openspec/changes/`) is the source of truth. `.in-progress` locks and ChatOps state files (`.question.json`, `.answer.json`) live alongside the proposals so they survive restarts.
- **Single-instance assumption + explicit startup contract:** Only one orchestrator daemon runs per workspace. On startup, the orchestrator (a) deletes any pre-existing `.in-progress` files (conclusively stale, since the daemon is the only writer and it just started), and (b) inspects the workspace for uncommitted git state. If `git status --porcelain` returns non-empty, the orchestrator logs an error and skips that repository's polling loop for the lifetime of the process. This resolves the prior contradiction between "preserve locks across crashes" and "auto-cleanup on startup": locks are cleaned, but a dirty workspace is treated as a hard error rather than silently retried, preserving the user's ability to inspect what was lost.
- **Verifiable acceptance:** Every requirement in the architecture-level specs names at least one externally-observable side effect — an HTTP call, a file path, an exit code, a branch ref, a `git status` output — so an implementation cannot satisfy a scenario with a placeholder print statement.

## Risks / Trade-offs

- **Risk:** The executor abstraction may be over-engineered if only one backend is ever implemented.
  - **Mitigation:** The trait is intentionally minimal (two methods, one enum). If multi-backend support never materializes, the trait can be inlined with a small refactor; nothing is lost by starting with it.
- **Risk:** Executor backends hang indefinitely.
  - **Mitigation:** The orchestrator wraps every `Executor::run` call in a configurable `tokio::time::timeout`. On timeout, the orchestrator treats the call as `Failed { reason: "timeout" }`, unlocks the change, and does NOT archive it.
- **Risk:** A dirty workspace at startup blocks the daemon.
  - **Mitigation:** This is intentional. The recovery path is the `rewind` subcommand, which the user invokes manually after auditing what was lost.
- **Risk:** Force-recreating the agent branch each pass discards prior commits that may have been worth salvaging.
  - **Mitigation:** Pushes use `--force-with-lease`, not `--force`, so concurrent human edits to the remote agent branch are detected. Salvaging prior agent work is out of scope; this is a "redo from scratch" workflow by design.
