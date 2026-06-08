# executor — delta for a62-change-vs-canonical-gate

## ADDED Requirements

### Requirement: submit_canon_contradictions MCP tool returns change-vs-canonical contradictions
The per-execution MCP child SHALL advertise a `submit_canon_contradictions` tool — built on a56's per-role submission framework — whenever `ORCH_MCP_ROLE = canon_contradiction_check`, AND SHALL NOT advertise it for any other role. The tool's payload schema SHALL be `{ contradictions: [{ change_requirement: string, canonical_capability: string, canonical_requirement: string, summary: string }] }` — each finding names the canonical requirement (by capability AND title) that the change's requirement conflicts with, distinguishing it from the `[in]` gate's within-change `submit_contradictions`. The tool relays through a56's `relay_submission` → `record_submission`; a schema-invalid payload is rejected AND surfaced to the agent as a correctable tool error it can retry in the same session.

Because the `[canon]` gate is fail-open (per the orchestrator-cli requirement AND the a61 framework), a session that ends with no stored submission SHALL be consumed as an empty result rather than an error — the fail-open WARN-and-proceed decision lives in the orchestrator-cli caller.

#### Scenario: Advertised only for the canon-check role
- **WHEN** the MCP child starts with `ORCH_MCP_ROLE = canon_contradiction_check`
- **THEN** the `tools/list` response advertises `submit_canon_contradictions` with the canon-contradictions schema alongside the common tools
- **WHEN** the MCP child starts with any other role (`implementer`, `reviewer`, `contradiction_check`, an advisory audit)
- **THEN** `submit_canon_contradictions` is NOT advertised

#### Scenario: Valid submission is consumed by the caller
- **WHEN** the agent calls `submit_canon_contradictions` with a schema-valid payload
- **THEN** the MCP child relays it via `record_submission` (a56)
- **AND** after the session exits the daemon `consume_submission`s the stored payload for the orchestrator-cli caller to turn into the marker

#### Scenario: Schema-invalid submission is correctable
- **WHEN** a `submit_canon_contradictions` payload fails the schema (missing `canonical_requirement`, non-array `contradictions`)
- **THEN** `record_submission` rejects it (a56) AND the agent observes a correctable tool error it can retry in the same session

#### Scenario: Missing submission consumed as empty, not an error
- **WHEN** a `[canon]` session exits with no stored submission for the execution
- **THEN** `consume_submission` returns an empty result
- **AND** the tool layer does NOT raise an error — the orchestrator-cli caller's fail-open policy decides the outcome
