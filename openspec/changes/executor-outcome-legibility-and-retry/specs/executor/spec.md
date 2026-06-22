## MODIFIED Requirements

### Requirement: Backend-agnostic execution contract
The orchestrator SHALL invoke implementations through a trait-shaped abstraction that takes a workspace path and an OpenSpec change name and returns an outcome enum. The architecture-level spec does NOT name a concrete backend; concrete implementations (CLI wrappers, MCP-connected agents, future native loops) are introduced by separate implementation changes.

When a backend terminates abnormally, the non-empty `reason` SHALL be ASSEMBLED from the evidence the backend captured — the agent's final message, the captured standard-error stream, and the process exit status or terminating signal — surfaced RAW and each truncated to a bounded budget. The orchestrator SHALL NOT parse, match, or classify provider-specific error text to construct the reason; it surfaces the captured evidence for a human to interpret, which keeps the contract provider-agnostic across every wrapped CLI and model.

#### Scenario: Successful implementation
- **WHEN** the orchestrator calls `Executor::run(workspace, change_name)` with a valid workspace path and an unarchived change name
- **AND** the underlying backend reports successful completion of the implementation
- **THEN** the call returns `Ok(ExecutorOutcome::Completed)`
- **AND** the workspace working tree contains modifications attributable to the executor, verifiable via `git status --porcelain` returning a non-empty result inside the workspace

#### Scenario: Agent requires clarification
- **WHEN** the underlying backend signals ambiguity through any backend-specific mechanism (tool call, exit code, structured output, etc.)
- **THEN** the call returns `Ok(ExecutorOutcome::AskUser { question, resume_handle })` where `question` is a non-empty human-readable string and `resume_handle` is a value implementing `serde::Serialize` and `serde::Deserialize` so it can be persisted to `.question.json` and restored after a daemon restart
- **AND** no commits are produced on the agent branch as a side effect of the halted implementation
- **AND** the orchestrator (NOT the executor) is responsible for writing `.question.json` and posting the question to ChatOps

#### Scenario: Backend failure
- **WHEN** the underlying backend terminates abnormally (non-zero exit, crash, malformed output, network error, or an enclosing timeout fires)
- **THEN** the call returns `Ok(ExecutorOutcome::Failed { reason })` with a non-empty `reason` string OR `Err(_)` for unrecoverable infrastructure errors that prevent the executor from determining outcome
- **AND** the `reason` is assembled from the captured evidence — the agent's final message (if non-empty), the captured standard-error (if non-empty), and the process exit status or terminating signal — in that priority order, each truncated to a bounded budget, surfaced RAW without parsing or error-classification
- **AND** the orchestrator unlocks the change (removes `.in-progress`) and does NOT archive it

#### Scenario: Failure reason includes the captured final message and standard-error
- **WHEN** a backend fails AND captured a final message and/or standard-error output
- **THEN** the assembled `reason` includes that captured text, truncated to a bounded budget, so the operator sees the actual cause (e.g. an upstream-API message such as an overload notice, or a panic trace) rather than only an exit code

#### Scenario: Failure reason surfaces the exit status when no output was captured
- **WHEN** a backend terminates abnormally AND both the agent's final message and the captured standard-error are empty (e.g. the process was killed by a signal before emitting anything)
- **THEN** the assembled `reason` surfaces the process exit status or terminating signal, so an empty-output death is still legible rather than blank

### Requirement: CliStrategy trait with the claude implementation
The agentic-run primitive SHALL select its CLI invocation through a `CliStrategy` trait so a model's provider can determine the CLI without role code changing. The trait SHALL do two jobs: build the invocation (binary, flags, the allowed-tools/sandbox-settings format, AND the MCP-config-file format) AND translate a resolved `(provider, model, api_base_url, api_key)` into that CLI's model-selection mechanism. A role's strategy SHALL be resolved from the model's provider via the model registry's `provider → default CLI` rule.

This change SHALL implement the `claude` strategy AND reproduce today's invocation exactly: `--settings <sandbox-file>`, `--allowedTools <combined>`, `--permission-mode acceptEdits`, AND — in streaming mode — `--verbose --output-format stream-json`, with MCP delivered via `.mcp.json`. The `claude` strategy SHALL select the model via `ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_MODEL` ONLY when a model is configured; when no model is configured it SHALL set none of them, preserving the executor's current CLI-default behavior. A role whose provider resolves to a CLI with no registered strategy SHALL return a clear error naming that CLI; this change registers only the `claude` strategy, so any non-`claude` resolution errors until that CLI's strategy is added (the `opencode` strategy is added by a later change).

The trait SHALL also expose an OPTIONAL, defaulted retry hint `is_retryable(&AgenticRunOutcome) -> Option<bool>` whose default body returns `None`. A strategy MAY override it to encode its own provider's retry signals; `None` means the strategy expresses no opinion, AND the orchestrator's bounded no-committable-result retry rule (see orchestrator-cli) applies. This keeps any provider-specific retry knowledge encapsulated in the strategy that owns it, so the core retry path stays provider-agnostic.

#### Scenario: Claude strategy with no model preserves CLI-default behavior
- **WHEN** the `claude` strategy builds an invocation with `model: None` (the executor's current state)
- **THEN** none of `ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_MODEL` is set
- **AND** the invocation is byte-identical to the pre-refactor executor command

#### Scenario: Claude strategy with a model sets the selection env
- **WHEN** the `claude` strategy builds an invocation with a resolved model `(anthropic, claude-opus-4-8, base, key)`
- **THEN** `ANTHROPIC_BASE_URL`, `ANTHROPIC_AUTH_TOKEN`, AND `ANTHROPIC_MODEL` are set from the resolved tuple

#### Scenario: A CLI with no registered strategy returns a clear error
- **WHEN** a role's model resolves (via the registry rule) to a CLI that has no registered strategy (e.g. `opencode`, before its strategy is added)
- **THEN** strategy resolution returns an error naming the CLI
- **AND** no subprocess is spawned

#### Scenario: Default retry hint expresses no opinion
- **WHEN** a `CliStrategy` does not override `is_retryable` (the `claude` strategy)
- **THEN** `is_retryable(outcome)` returns `None` for every outcome
- **AND** the orchestrator falls back to its bounded no-committable-result retry rule rather than treating `None` as "never retry"
