# orchestrator-cli — delta for a63-code-implements-spec-gate

## ADDED Requirements

### Requirement: Code-implements-spec verification (the [out] gate, advisory)
autocoder SHALL provide an opt-in post-executor check — the `[out]` gate of the verifier framework — that judges whether the executor's implementation satisfies the change's spec delta, requirement by requirement AND scenario by scenario. This is the verifier step the code-reviewer requirement defers to ("Do NOT assess whether the diff implements the spec; that is handled separately by the verifier step"). The gate runs a CLI-wrapped agentic session through the shared `agentic_run` primitive (a56) AFTER the executor implements the change, in a read-only sandbox that reads the spec delta, the diff, AND source on demand, AND returns its verdict via the `submit_verdict` MCP tool.

The gate SHALL be advisory: it annotates AND never auto-acts. It renders the verdict as a `## Spec Verification` section in the PR body (parallel to the reviewer's `## Code Review` block) AND posts a chatops note ONLY when gaps are found. It SHALL NEVER open a revision AND SHALL NEVER block PR creation. Per the a61 framework's advisory posture, a gate failure (session error, a resolved CLI strategy that is not registered, a schema-rejected submission never corrected, OR no submission) logs a WARN carrying the `[out]` label AND omits the section (OR writes "verification unavailable"); it never blocks. A schema-invalid `submit_verdict` call mid-session is a correctable tool error the agent can retry (a56).

The check SHALL be gated by `executor.code_implements_spec_check` (`disabled` default, `enabled` opt-in). The model is configured via `executor.code_implements_spec_check_llm`, which a56's CLI strategy translates into the wrapped CLI's model-selection mechanism. Enabling the check without configuring the model SHALL fail at daemon startup with a fail-fast validation error.

#### Scenario: Default-disabled produces no [out] session
- **WHEN** `executor.code_implements_spec_check` is unset (default `disabled`)
- **AND** the executor implements a change
- **THEN** no `[out]` session is spawned AND PR assembly is unchanged

#### Scenario: Enabled mode verifies the implementation against the spec
- **WHEN** `executor.code_implements_spec_check: enabled` AND the model config is set
- **AND** the executor has implemented a change
- **THEN** the gate runs an `agentic_run` session (a56) in a read-only sandbox (`Read`/`Glob`/`Grep`, `ORCH_MCP_ROLE = code_implements_spec`, the `submit_verdict` MCP tool) with the embedded `prompts/code-implements-spec-check.md` prompt (OR the configured override), carrying the spec-delta files, the unified diff, AND the changed-file list
- **AND** the agent reads source on demand AND returns its verdict by calling `submit_verdict` with `{ verdict, summary, gaps }`

#### Scenario: Implemented verdict renders a clean section, no chatops
- **WHEN** the agent submits `{ verdict: "implemented", ... }`
- **THEN** the PR body's `## Spec Verification` section reports the implementation as complete
- **AND** no chatops note is posted
- **AND** no revision is opened AND PR creation proceeds normally

#### Scenario: Gaps-found verdict annotates and notifies but never acts
- **WHEN** the agent submits `{ verdict: "gaps_found", gaps: [ ... ] }`
- **THEN** the PR body's `## Spec Verification` section lists each gap (`requirement`, optional `scenario`, `status`, `evidence`)
- **AND** a chatops note is posted as an advisory heads-up
- **AND** NO revision is opened AND PR creation is NOT blocked — the operator decides what to do

#### Scenario: Gate failure is advisory, never blocking
- **WHEN** the agentic session fails (spawn error, timeout, unregistered strategy, OR no valid `submit_verdict`)
- **THEN** the gate logs a WARN carrying the `[out]` label
- **AND** omits the `## Spec Verification` section (OR writes "verification unavailable")
- **AND** PR creation proceeds — the gate never blocks

#### Scenario: Enabled without model config fails fast at startup
- **WHEN** `config.yaml` sets `executor.code_implements_spec_check: enabled`
- **AND** `executor.code_implements_spec_check_llm` is unset
- **THEN** daemon startup fails with a named error AND does NOT begin polling
- **AND** the operator sees the error on stderr AND in journalctl
