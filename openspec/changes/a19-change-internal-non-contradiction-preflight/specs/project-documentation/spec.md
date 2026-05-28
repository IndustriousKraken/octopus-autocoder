## ADDED Requirements

### Requirement: CONFIG.md and OPERATIONS.md document the contradiction-check fields and cost model
`docs/CONFIG.md`'s `executor:` table SHALL include rows for the three new fields (`change_internal_contradiction_check`, `change_internal_contradiction_check_prompt_path`, `change_internal_contradiction_check_llm`). `docs/OPERATIONS.md` SHALL include a "Pre-flight checks" section enumerating the layered pre-executor checks (validate → archivability → contradiction) AND noting the contradiction check's opt-in posture, LLM cost, AND fail-open behavior.

#### Scenario: CONFIG.md documents all three new fields
- **WHEN** an operator reads `docs/CONFIG.md`'s `executor:` table
- **THEN** rows for `change_internal_contradiction_check` (default `disabled`), `change_internal_contradiction_check_prompt_path` (default `null`, embedded template), AND `change_internal_contradiction_check_llm` (required when the check is enabled) appear with brief descriptions
- **AND** each row cross-links to OPERATIONS.md's pre-flight-checks section for the full operational discussion

#### Scenario: OPERATIONS.md enumerates the pre-flight layers
- **WHEN** an operator reads `docs/OPERATIONS.md`'s pre-flight-checks section
- **THEN** the section enumerates the three layered checks: `openspec validate --strict` (well-formedness, free), `a17`'s archivability check (mechanical, free), AND `a19`'s contradiction check (LLM, opt-in, small per-change cost)
- **AND** each layer's purpose is named AND the failure mode (marker + chatops alert + executor-skip) is described
- **AND** the contradiction check's opt-in posture is explained: operators trading a small per-change LLM cost for the catch of semantic self-contradictions enable it; default-off operators see no behavior change

#### Scenario: OPERATIONS.md describes the fail-open posture
- **WHEN** an operator reads the contradiction-check description in OPERATIONS.md
- **THEN** the section notes that LLM failures (transport, parse, etc.) fail OPEN — the executor proceeds, the operator sees a WARN in journalctl
- **AND** the section explains why: a failed check should not block work; operators decide whether to investigate based on the WARN cadence
