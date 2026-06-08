# orchestrator-cli — delta for a62-change-vs-canonical-gate

## ADDED Requirements

### Requirement: Change-vs-canonical contradiction pre-flight check (the [canon] gate)
autocoder SHALL provide an opt-in pre-flight check — the `[canon]` gate of the verifier framework — that detects semantic contradictions between a single OpenSpec change's spec deltas AND the project's EXISTING canonical specs, before the executor is invoked. The check runs a CLI-wrapped agentic session through the shared `agentic_run` primitive (a56) in a read-only sandbox that reads the change's spec-delta files AND the canonical specs on demand, AND returns its findings via the `submit_canon_contradictions` MCP tool. On non-empty findings, autocoder SHALL write `.needs-spec-revision.json` with `revision_suggestion` populated from the canon-contradiction narrative, post the existing `AlertCategory::SpecNeedsRevision` chatops alert, AND halt the queue walk for this iteration. The executor SHALL NOT be invoked when contradictions are found. The gate's disposition is identical to the `[in]` gate's; the gates differ only in what they read (deltas-only vs deltas-plus-canon) AND what each finding names.

The check SHALL be gated by `executor.change_canonical_contradiction_check` (`disabled` default, `enabled` opt-in). The model is configured via `executor.change_canonical_contradiction_check_llm` (parallel to the `[in]` gate's block), which a56's CLI strategy translates into the wrapped CLI's model-selection mechanism. Enabling the check without configuring the model SHALL fail at daemon startup with a fail-fast validation error.

Canon access SHALL follow the documentation-audit pattern: the gate reads `openspec/specs/*/spec.md` directly through the sandbox AND additionally uses the `query_canonical_specs` MCP tool when a21's RAG is enabled (focused retrieval for large canon). The gate SHALL function correctly with OR without RAG.

Per the verifier framework (a61), the `[canon]` gate SHALL be fail-open AND SHALL label its diagnostics with the `[canon]` identifier: an agentic-session error (spawn, timeout, OR a resolved CLI strategy that is not registered), a schema-rejected submission the agent never corrects, a session that ends with no submission, OR any other failure log a WARN AND treat the check as "no contradictions found." A schema-invalid `submit_canon_contradictions` call mid-session is a correctable tool error the agent can retry (a56). The daemon does NOT gate work on a failed check.

#### Scenario: Default-disabled produces no [canon] session
- **WHEN** `executor.change_canonical_contradiction_check` is unset (default `disabled`)
- **AND** any change reaches the pre-executor pipeline
- **THEN** no `[canon]` session is spawned
- **AND** the executor is invoked normally (assuming the earlier gates passed)

#### Scenario: Enabled mode checks the deltas against canon
- **WHEN** `executor.change_canonical_contradiction_check: enabled` AND the model config is set
- **AND** a change reaches the pre-executor pipeline
- **THEN** the gate runs an `agentic_run` session (a56) in a read-only sandbox (`Read`/`Glob`/`Grep`, `ORCH_MCP_ROLE = canon_contradiction_check`, the `submit_canon_contradictions` MCP tool) with the embedded `prompts/change-vs-canonical-check.md` prompt (OR the configured override)
- **AND** the agent reads the change's spec-delta files AND the canonical specs on demand AND returns contradictions by calling `submit_canon_contradictions` with `{ contradictions: [{ change_requirement, canonical_capability, canonical_requirement, summary }] }`

#### Scenario: Empty submission proceeds to executor
- **WHEN** the agent calls `submit_canon_contradictions` with an empty `contradictions` array
- **THEN** the pipeline proceeds to the executor
- **AND** no marker is written AND no chatops alert fires

#### Scenario: Non-empty submission writes marker and halts
- **WHEN** the agent submits one or more change-vs-canonical contradictions
- **THEN** the pipeline writes `.needs-spec-revision.json` with `revision_suggestion` text populated from the contradictions narrative (each finding naming the conflicting canonical requirement)
- **AND** the marker's structural arrays (`unarchivable_deltas`, `unimplementable_tasks`) are empty (this case is semantic)
- **AND** the chatops alert under `AlertCategory::SpecNeedsRevision` fires (subject to the throttle)
- **AND** the executor is NOT invoked for this change OR any subsequent change in this iteration

#### Scenario: Runs with and without a21 RAG
- **WHEN** a21's `canonical_rag` is enabled AND the gate runs
- **THEN** the session has `query_canonical_specs` available AND the prompt MAY use it for focused canonical retrieval
- **WHEN** `canonical_rag` is disabled AND the gate runs
- **THEN** the gate reads canon directly via the sandbox's `Read` of `openspec/specs/*/spec.md` AND still produces valid findings

#### Scenario: Session failure fails open
- **WHEN** the agentic session fails (spawn error, timeout, OR the resolved CLI strategy is not registered)
- **THEN** the gate logs a WARN (carrying the `[canon]` label) naming the error
- **AND** treats the check as "no contradictions found" AND proceeds to the executor

#### Scenario: No valid submission fails open
- **WHEN** the session ends with no schema-valid `submit_canon_contradictions` call (never submitted, OR every submission schema-rejected and never corrected)
- **THEN** the gate logs a WARN (carrying the `[canon]` label) with a truncated session-output excerpt
- **AND** treats the check as "no contradictions found" AND proceeds to the executor

#### Scenario: Enabled without model config fails fast at startup
- **WHEN** `config.yaml` sets `executor.change_canonical_contradiction_check: enabled`
- **AND** `executor.change_canonical_contradiction_check_llm` is unset
- **THEN** daemon startup fails with a named error AND does NOT begin polling
- **AND** the operator sees the error on stderr AND in journalctl
