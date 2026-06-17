## MODIFIED Requirements

### Requirement: submit_canon_contradictions MCP tool returns change-vs-canonical contradictions
The per-execution MCP child SHALL advertise a `submit_canon_contradictions` tool — built on a56's per-role submission framework — whenever `ORCH_MCP_ROLE = canon_contradiction_check`, AND SHALL NOT advertise it for any other role. The tool's payload schema SHALL be `{ contradictions: [{ change_requirement: string, canonical_capability: string, canonical_requirement: string, summary: string }] }` — each finding names the canonical requirement (by capability AND title) that the change's requirement conflicts with, distinguishing it from the `[in]` gate's within-change `submit_contradictions`. The tool relays through a56's `relay_submission` → `record_submission`; a schema-invalid payload is rejected AND surfaced to the agent as a correctable tool error it can retry in the same session.

A session that ends with no stored submission SHALL be consumed as an empty result AT THE TOOL LAYER rather than an error; the open-vs-closed decision is NOT encoded here — the `[canon]` gate's fail-closed policy (per its orchestrator-cli requirement AND the verifier framework) decides the outcome of a no-submission session (it holds the change). (This corrects an earlier description of the caller as "fail-open," which contradicted the `[canon]` gate's fail-closed posture.)

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

#### Scenario: Missing submission consumed as empty; the gate's policy decides
- **WHEN** a `[canon]` session exits with no stored submission for the execution
- **THEN** `consume_submission` returns an empty result AND the tool layer does NOT raise an error
- **AND** the `[canon]` gate's fail-closed policy (orchestrator-cli) decides whether that no-submission session holds the change

### Requirement: submit_contradictions MCP tool returns change-internal contradictions
The per-execution MCP child SHALL advertise a `submit_contradictions` tool — built on a56's per-role submission framework — whenever `ORCH_MCP_ROLE = contradiction_check`, AND SHALL NOT advertise it for any other role. The tool's payload schema SHALL be `{ contradictions: [{ requirement_a: string, requirement_b: string, summary: string }] }`. The tool relays through a56's `relay_submission` → `record_submission`; a schema-invalid payload is rejected AND surfaced to the agent as a correctable tool error it can retry in the same session.

A session that ends with no stored submission SHALL be consumed as an empty result AT THE TOOL LAYER rather than an error; the open-vs-closed decision is NOT encoded here — the `[in]` gate's fail-closed policy (per its orchestrator-cli requirement AND the verifier framework) decides the outcome of a no-submission session (it holds the change). (This corrects an earlier description of the caller as "fail-open," which contradicted the `[in]` gate's fail-closed posture.) A non-empty consumed submission carries the contradictions the caller turns into the `.needs-spec-revision.json` marker.

#### Scenario: Advertised only for the contradiction-check role
- **WHEN** the MCP child starts with `ORCH_MCP_ROLE = contradiction_check`
- **THEN** the `tools/list` response advertises `submit_contradictions` with the contradictions schema alongside the common tools
- **WHEN** the MCP child starts with any other role (`implementer`, `reviewer`, an advisory audit)
- **THEN** `submit_contradictions` is NOT advertised

#### Scenario: Valid submission is consumed by the caller
- **WHEN** the agent calls `submit_contradictions` with a schema-valid payload
- **THEN** the MCP child relays it via `record_submission` (a56)
- **AND** after the session exits the daemon `consume_submission`s the stored payload for the orchestrator-cli caller to act on

#### Scenario: Schema-invalid submission is correctable
- **WHEN** a `submit_contradictions` payload fails the schema (missing field, non-array `contradictions`)
- **THEN** `record_submission` rejects it (a56) AND the agent observes a correctable tool error it can retry in the same session

#### Scenario: Missing submission consumed as empty; the gate's policy decides
- **WHEN** a contradiction-check session exits with no stored submission for the execution
- **THEN** `consume_submission` returns an empty result (no contradictions) AND the tool layer does NOT raise an error
- **AND** the `[in]` gate's fail-closed policy (orchestrator-cli) decides whether that no-submission session holds the change

## ADDED Requirements

### Requirement: submit_rule_violations MCP tool returns global-rule violations
The per-execution MCP child SHALL advertise a `submit_rule_violations` tool — built on a56's per-role submission framework — whenever `ORCH_MCP_ROLE = global_rules_check`, AND SHALL NOT advertise it for any other role. The tool's payload schema SHALL be `{ violations: [{ rule_id: string, summary: string }] }` — each finding names the violated rule by its stable id (per the rule protocol) AND summarizes how the change violates it, distinguishing it from the `[canon]` gate's `submit_canon_contradictions` (which names a canonical requirement). The tool relays through a56's `relay_submission` → `record_submission`; a schema-invalid payload is rejected AND surfaced to the agent as a correctable tool error it can retry in the same session.

A session that ends with no stored submission SHALL be consumed as an empty result at the tool layer; the open-vs-closed decision is NOT encoded here — the `[rules]` gate's fail-closed policy (per its orchestrator-cli requirement) decides the outcome of a no-submission session.

#### Scenario: Advertised only for the global-rules-check role
- **WHEN** the MCP child starts with `ORCH_MCP_ROLE = global_rules_check`
- **THEN** the `tools/list` response advertises `submit_rule_violations` with the rule-violations schema alongside the common tools
- **WHEN** the MCP child starts with any other role (`implementer`, `reviewer`, `contradiction_check`, `canon_contradiction_check`, an advisory audit)
- **THEN** `submit_rule_violations` is NOT advertised

#### Scenario: Valid submission is consumed by the caller
- **WHEN** the agent calls `submit_rule_violations` with a schema-valid payload
- **THEN** the MCP child relays it via `record_submission` (a56)
- **AND** after the session exits the daemon `consume_submission`s the stored payload for the `[rules]`-gate caller to turn into the marker

#### Scenario: Schema-invalid submission is correctable
- **WHEN** a `submit_rule_violations` payload fails the schema (missing `rule_id`, non-array `violations`)
- **THEN** `record_submission` rejects it (a56) AND the agent observes a correctable tool error it can retry in the same session

#### Scenario: Missing submission consumed as empty; the gate's policy decides
- **WHEN** a `[rules]` session exits with no stored submission for the execution
- **THEN** `consume_submission` returns an empty result AND the tool layer does NOT raise an error
- **AND** the `[rules]` gate's fail-closed policy (orchestrator-cli) decides whether that no-submission session holds the change
