# executor — delta for a63-code-implements-spec-gate

## ADDED Requirements

### Requirement: submit_verdict MCP tool returns the code-implements-spec verdict
The per-execution MCP child SHALL advertise a `submit_verdict` tool — the last of a56's reserved per-role submission tools, built on the same framework — whenever `ORCH_MCP_ROLE = code_implements_spec`, AND SHALL NOT advertise it for any other role. The tool's payload schema SHALL be `{ verdict: "implemented" | "gaps_found", summary: string, gaps: [{ requirement: string, scenario: string|null, status: "missing" | "partial", evidence: string }] }`. The schema SHALL enforce the `verdict` enum AND SHALL require a non-empty `gaps` array whenever `verdict: gaps_found`. The tool relays through a56's `relay_submission` → `record_submission`; a schema-invalid payload is rejected AND surfaced to the agent as a correctable tool error it can retry in the same session.

Because the `[out]` gate is advisory (per the orchestrator-cli requirement AND the a61 framework), a session that ends with no stored submission SHALL be consumed as an empty result rather than an error — the caller omits the `## Spec Verification` section AND logs a WARN; it never blocks. A consumed `gaps_found` verdict drives the advisory annotation AND the chatops heads-up, never a revision.

#### Scenario: Advertised only for the code-implements-spec role
- **WHEN** the MCP child starts with `ORCH_MCP_ROLE = code_implements_spec`
- **THEN** the `tools/list` response advertises `submit_verdict` with the verdict schema alongside the common tools
- **WHEN** the MCP child starts with any other role (`implementer`, `reviewer`, a contradiction gate, an advisory audit)
- **THEN** `submit_verdict` is NOT advertised

#### Scenario: Valid verdict is consumed by the caller
- **WHEN** the agent calls `submit_verdict` with a schema-valid payload
- **THEN** the MCP child relays it via `record_submission` (a56)
- **AND** after the session exits the daemon `consume_submission`s the payload for the orchestrator-cli caller to render the advisory section

#### Scenario: gaps_found requires a non-empty gaps array
- **WHEN** a `submit_verdict` payload has `verdict: "gaps_found"` AND an empty `gaps` array, OR a `verdict` outside the enum
- **THEN** `record_submission` rejects it (a56) AND the agent observes a correctable tool error it can retry in the same session

#### Scenario: Missing submission consumed as empty, never blocks
- **WHEN** a `[out]` session exits with no stored submission for the execution
- **THEN** `consume_submission` returns an empty result
- **AND** the tool layer does NOT raise an error — the orchestrator-cli caller omits the advisory section AND proceeds (the gate never blocks)
