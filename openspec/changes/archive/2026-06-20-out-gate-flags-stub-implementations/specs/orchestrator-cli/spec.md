## MODIFIED Requirements

### Requirement: Code-implements-spec verification (the [out] gate, advisory)
autocoder SHALL provide an opt-in post-executor check — the `[out]` gate of the verifier framework — that judges whether the executor's implementation satisfies the change's spec delta, requirement by requirement AND scenario by scenario. This is the verifier step the code-reviewer requirement defers to ("Do NOT assess whether the diff implements the spec; that is handled separately by the verifier step"). The gate runs a CLI-wrapped agentic session through the shared `agentic_run` primitive (a56) AFTER the executor implements the change, in a read-only sandbox that reads the spec delta, the diff, AND source on demand, AND returns its verdict via the `submit_verdict` MCP tool.

Judging whether the implementation satisfies a requirement SHALL include judging that the required behavior is REALLY implemented, not merely sketched. Per the project's no-stubs standard, where the spec delta calls for working code, a requirement (or scenario) is NOT satisfied — it is a gap — when the code that landed only stubs OR defers the behavior: a placeholder or hardcoded/faked return value, a `todo!()` / `unimplemented!()` / `panic!("not implemented")`, an unconditional early-return that skips the required path, a branch or error path left unwired, a config flag read but never acted on, OR an explicit deferral of the behavior to a later change. A wholly-stubbed requirement SHALL be reported as a `missing` gap; a half-wired one (the behavior exists for some inputs but a required path is stubbed) SHALL be reported as a `partial` gap, each with the stub itself as the evidence. A plausible-looking diff that does not actually do the work the spec requires is NOT a pass, AND the gate SHALL flag it whether or not the spec delta separately says "do not stub."

The gate SHALL be advisory: it annotates AND never auto-acts. It renders the verdict as a `## Spec Verification` section in the PR body (parallel to the reviewer's `## Code Review` block) AND posts a chatops note ONLY when gaps are found. It SHALL NEVER open a revision AND SHALL NEVER block PR creation. Per the gatekeepers-fail-closed standard, the gate fails CLOSED to a VISIBLE state rather than silence: a gate failure (session error, a resolved CLI strategy that is not registered, a schema-rejected submission never corrected, OR no submission) logs a WARN carrying the `[out]` label AND renders an explicit `## Spec Verification: FAILED TO RUN` section naming the cause — making clear the change was NOT verified (NOT a pass) — rather than omitting the section. It still never blocks PR creation. A schema-invalid `submit_verdict` call mid-session is a correctable tool error the agent can retry (a56).

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

#### Scenario: Gate failure renders FAILED TO RUN, never blocking
- **WHEN** the agentic session fails (spawn error, timeout, unregistered strategy, OR no valid `submit_verdict`)
- **THEN** the gate logs a WARN carrying the `[out]` label
- **AND** renders an explicit `## Spec Verification: FAILED TO RUN` section naming the cause (the change is NOT verified — NOT a pass), rather than omitting the section
- **AND** PR creation proceeds — the gate never blocks

#### Scenario: Enabled without model config fails fast at startup
- **WHEN** `config.yaml` sets `executor.code_implements_spec_check: enabled`
- **AND** `executor.code_implements_spec_check_llm` is unset
- **THEN** daemon startup fails with a named error AND does NOT begin polling
- **AND** the operator sees the error on stderr AND in journalctl

#### Scenario: A stubbed or deferred required behavior is reported as a gap
- **WHEN** the spec delta calls for working code for a behavior AND the landed implementation only stubs OR defers it (a placeholder/hardcoded return, `todo!()`/`unimplemented!()`, an unconditional early-return that skips the required path, an unwired branch, OR an explicit deferral to a later change)
- **THEN** the gate reports that requirement (or scenario) as a gap with the stub as concrete evidence — `missing` when wholly stubbed, `partial` when a required path is stubbed
- **AND** it does NOT report the change as fully implemented
