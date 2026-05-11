# executor Specification

## Purpose
TBD - created by archiving change orchestrator-architecture. Update Purpose after archive.
## Requirements
### Requirement: Backend-agnostic execution contract
autocoder SHALL invoke implementations through a trait-shaped abstraction that takes a workspace path and an OpenSpec change name and returns an outcome enum. The architecture-level spec does NOT name a concrete backend; concrete implementations (CLI wrappers, MCP-connected agents, future native loops) are introduced by separate implementation changes.

#### Scenario: Successful implementation
- **WHEN** autocoder calls `Executor::run(workspace, change_name)` with a valid workspace path and an unarchived change name
- **AND** the underlying backend reports successful completion of the implementation
- **THEN** the call returns `Ok(ExecutorOutcome::Completed)`
- **AND** the workspace working tree contains modifications attributable to the executor, verifiable via `git status --porcelain` returning a non-empty result inside the workspace

#### Scenario: Agent requires clarification
- **WHEN** the underlying backend signals ambiguity through any backend-specific mechanism (tool call, exit code, structured output, etc.)
- **THEN** the call returns `Ok(ExecutorOutcome::AskUser { question, resume_handle })` where `question` is a non-empty human-readable string and `resume_handle` is a value implementing `serde::Serialize` and `serde::Deserialize` so it can be persisted to `.question.json` and restored after a daemon restart
- **AND** no commits are produced on the agent branch as a side effect of the halted implementation
- **AND** autocoder (NOT the executor) is responsible for writing `.question.json` and posting the question to ChatOps

#### Scenario: Backend failure
- **WHEN** the underlying backend terminates abnormally (non-zero exit, crash, malformed output, network error, or an enclosing timeout fires)
- **THEN** the call returns `Ok(ExecutorOutcome::Failed { reason })` with a non-empty `reason` string OR `Err(_)` for unrecoverable infrastructure errors that prevent the executor from determining outcome
- **AND** autocoder unlocks the change (removes `.in-progress`) and does NOT archive it

### Requirement: Resume after ask-user
The executor SHALL support resuming a previously halted implementation when a human answer becomes available.

#### Scenario: Resuming with an answer
- **WHEN** autocoder calls `Executor::resume(resume_handle, answer)` with a `resume_handle` previously returned from `run` and a non-empty `answer` string
- **THEN** the call returns one of `Ok(ExecutorOutcome::Completed)`, `Ok(ExecutorOutcome::AskUser { ... })`, `Ok(ExecutorOutcome::Failed { ... })`, or `Err(_)`, with the same observable side-effect contracts as `run`
- **AND** autocoder MUST consume (delete or mark answered) the prior `.question.json` before invoking `resume`, so the executor cannot observe a stale escalation

#### Scenario: Resume after daemon restart
- **WHEN** autocoder restarts and finds a `.question.json` file alongside a corresponding `.answer.json` in `<workspace>/openspec/changes/<change>/`
- **THEN** autocoder deserializes the stored `resume_handle` from `.question.json` and calls `Executor::resume(handle, answer)` to continue execution
- **AND** the executor backend MUST tolerate a `resume_handle` that was serialized by a prior process invocation

### Requirement: CLI-wrapping executor backend (`claude_cli`)
autocoder SHALL provide a concrete `Executor` implementation that wraps an external command-line agent tool as a child process. The backend is selected via `executor.kind: claude_cli` in the configuration.

#### Scenario: ClaudeCliExecutor instantiation
- **WHEN** autocoder initializes AND `executor.kind` is `claude_cli`
- **THEN** autocoder instantiates a `ClaudeCliExecutor` configured from `executor.command` (default `claude`) and `executor.timeout_secs` (default 1800)
- **AND** the executor is wrapped in `Arc<dyn Executor>` and shared across all spawned polling tasks

#### Scenario: Outcome mapping from CLI exit code
- **WHEN** `Executor::run(workspace, change)` is called
- **THEN** the executor spawns the configured command as a tokio child process inside the workspace, providing the change's `proposal.md`/`design.md`/`tasks.md` contents (plus the output of `openspec instructions apply <change>` if the OpenSpec CLI is on PATH) as the prompt
- **AND** on child exit code 0, the call returns `Ok(ExecutorOutcome::Completed)` (the executor does NOT inspect the workspace for diff; autocoder handles the no-diff case per the architecture's `git-workflow-manager` "Executor reported Completed but produced no diff" scenario)
- **AND** on non-zero child exit, the call returns `Ok(ExecutorOutcome::Failed { reason })` where `reason` contains the first 200 characters of captured stderr
- **AND** if the configured `executor.timeout_secs` elapses, the child process is killed and the call returns `Ok(ExecutorOutcome::Failed { reason: "timeout" })`

#### Scenario: Resume not supported in this phase
- **WHEN** `Executor::resume(handle, answer)` is called on the foundation `ClaudeCliExecutor`
- **THEN** the call returns `Err(_)` whose text indicates resume is not supported until the `chatops-escalation` change retrofits real resume semantics
- **AND** no child process is spawned and no workspace state is modified

