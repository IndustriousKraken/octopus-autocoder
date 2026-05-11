## Why

The architecture-level specs in `orchestrator-architecture` define the daemon abstractly. We need a concrete first implementation that brings the orchestrator end-to-end against one or more repositories. Rather than implement a single-repo demo and graduate to multi-repo later, this change builds the production-shaped multi-repo daemon from day one — the implementation difference between "one repository" and "many repositories" is small enough (a `repositories: []` array, N tokio tasks instead of 1, a workspace-path collision check) that the staged migration would be artificial scaffolding that costs more than it saves.

## What Changes

- Initialize the Rust project (`Cargo.toml`, `src/main.rs`) with the dependencies named in `design.md`.
- Implement `src/cli.rs` with `run` and `rewind` subcommands using `clap`.
- Implement `src/config.rs` with a multi-repository configuration schema (`repositories: []`, plus `executor`, `github` blocks).
- Implement `src/git.rs` covering every git operation referenced in `git-workflow-manager/spec.md`.
- Implement `src/queue.rs` covering enumerate, lock, unlock, archive, unarchive, and stale-lock cleanup.
- Implement `src/workspace.rs` with deterministic per-repo workspace path derivation, idempotent clone-or-fetch, and startup-time collision detection.
- Define the `Executor` trait, `ExecutorOutcome` enum, and `ResumeHandle` newtype in `src/executor/mod.rs`.
- Implement `src/executor/claude_cli.rs` as the first concrete `Executor` backend, wrapping the `claude` CLI as a child process with timeout and explicit outcome mapping.
- Implement `src/github.rs` with real GitHub REST API PR creation, including SSH-and-HTTPS URL parsing for owner/repo extraction.
- Implement `src/polling_loop.rs` with one tokio task per repository: a sleep-then-iterate cycle wrapping the architecture's single-pass workflow.
- Wire it all together in `cli::run::execute`: validate config, detect workspace collisions, spawn one polling task per `repositories[]` entry, await SIGINT/SIGTERM via a shared `CancellationToken`, drain in-flight iterations on shutdown.

## Capabilities

### New Capabilities
- `workspace-manager`: deterministic per-repo workspace paths under `/tmp/workspaces/`, idempotent clone-or-fetch, startup-time cross-repo collision detection.

### Modified Capabilities
<!-- None. This change introduces real ADDED Requirements to the architecture-level `executor` and `orchestrator-cli` capabilities (the first concrete executor backend; the polling-loop semantics that fill in the architecture's per-repo task abstraction). It does not modify the architecture-level requirements; it extends them. -->

## Impact

After this change lands, the orchestrator runs as a multi-repository daemon: one tokio task per configured repo, each iterating its serial queue, producing monolithic PRs with `claude-cli` as the executor backend. ChatOps escalation, the code reviewer, and the rewind subcommand's multi-repo selector are added by subsequent changes.
