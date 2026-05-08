## ADDED Requirements

### Requirement: CLI-wrapping executor backend (`claude_cli`)
The orchestrator SHALL provide a concrete `Executor` implementation that wraps an external command-line agent tool as a child process. The backend is selected via `executor.kind: claude_cli` in the configuration.

#### Scenario: ClaudeCliExecutor instantiation
- **WHEN** the orchestrator initializes AND `executor.kind` is `claude_cli`
- **THEN** the orchestrator instantiates a `ClaudeCliExecutor` configured from `executor.command` (default `claude`) and `executor.timeout_secs` (default 1800)
- **AND** the executor is wrapped in `Arc<dyn Executor>` and shared across all spawned polling tasks

#### Scenario: Outcome mapping from CLI exit code
- **WHEN** `Executor::run(workspace, change)` is called
- **THEN** the executor spawns the configured command as a tokio child process inside the workspace, providing the change's `proposal.md`/`design.md`/`tasks.md` contents (plus the output of `openspec instructions apply <change>` if the OpenSpec CLI is on PATH) as the prompt
- **AND** on child exit code 0, the call returns `Ok(ExecutorOutcome::Completed)` (the executor does NOT inspect the workspace for diff; the orchestrator handles the no-diff case per the architecture's `git-workflow-manager` "Executor reported Completed but produced no diff" scenario)
- **AND** on non-zero child exit, the call returns `Ok(ExecutorOutcome::Failed { reason })` where `reason` contains the first 200 characters of captured stderr
- **AND** if the configured `executor.timeout_secs` elapses, the child process is killed and the call returns `Ok(ExecutorOutcome::Failed { reason: "timeout" })`

#### Scenario: Resume not supported in this phase
- **WHEN** `Executor::resume(handle, answer)` is called on the foundation `ClaudeCliExecutor`
- **THEN** the call returns `Err(_)` whose text indicates resume is not supported until the `chatops-escalation` change retrofits real resume semantics
- **AND** no child process is spawned and no workspace state is modified
