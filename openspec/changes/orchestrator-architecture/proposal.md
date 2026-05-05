## Why

This project requires an autonomous CI/CD pipeline that processes OpenSpec implementation proposals in sequence across multiple repositories without human supervision. The orchestrator must be a robust Rust server daemon that owns the workflow — queue, git, ChatOps, recovery — while delegating actual code-writing to a swappable executor backend. This keeps the orchestrator small, durable, and free of lock-in to any specific agent CLI or LLM provider.

## What Changes

- An `orchestrator` Rust binary will run as a persistent daemon.
- The queue operates as a "Serial Queue" per repository to handle dependent features safely without git conflicts.
- Implementation execution is performed by a backend-agnostic `Executor` abstraction. Concrete backends (CLI wrappers, MCP-connected agents, future native loops) are introduced by separate implementation changes; the architecture-level spec does not name any specific backend.
- The daemon generates one monolithic Pull Request at the end of each repository's polling pass, combining all processed changes.
- Asynchronous escalation (ChatOps) handles agent ambiguity without blocking the queue.
- Only one orchestrator daemon runs per workspace at a time. On startup, stale `.in-progress` locks are cleared and dirty workspaces are refused (the user must run `rewind` or clean up manually).

## Capabilities

### New Capabilities
- `orchestrator-cli`: Asynchronous daemon entry point, configuration loading, subcommand routing, signal handling.
- `openspec-queue-engine`: Watches `openspec/changes/`, enumerates ready changes, manages archive and lock state.
- `executor`: Backend-agnostic abstraction that runs an implementation given a workspace and change name; reports outcome (`Completed`, `AskUser`, or `Failed`) and supports resuming after a human answer.
- `git-workflow-manager`: Branch creation, serial commits, push, monolithic PR creation via the GitHub REST API.

### Modified Capabilities
- (none)

## Impact

This establishes the foundational blueprint for a production-grade, multi-repo AI CI/CD daemon. By keeping the executor abstract, the orchestrator avoids lock-in to any single agent CLI or LLM provider, stays small enough to actually maintain, and leaves room to evolve the execution backend without rewriting the workflow.
