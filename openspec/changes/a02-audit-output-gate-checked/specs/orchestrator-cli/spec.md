## ADDED Requirements

### Requirement: Spec-writing audits gate-check their output and self-heal before commit
After a spec-writing audit (`security_bug_audit`, `missing_tests_audit`) writes a spec-lane change AND it passes `openspec validate --strict`, the audit SHALL run the `[in]` (change-internal) AND `[canon]` (change-vs-canonical) verifier-gate checks against that change BEFORE committing it, for each gate that is enabled. These authoring-time checks reuse the verifier framework's existing checks unchanged — the same prompts, the same `submit_contradictions` / `submit_canon_contradictions` MCP tools, AND the same opt-in flags (`executor.change_internal_contradiction_check`, `executor.change_canonical_contradiction_check`) that govern the implement-time gates. A verifier gate that is disabled runs at neither authoring nor implement time; an enabled one runs at BOTH — early (here, self-healing) AND at implement time (the unchanged pre-executor gate). These verifier gates are distinct from the issue-lane implementer kick-back (`Issue-flavored implementer prompt verifies against existing canon`), which is an always-on property of the issue implementer — NOT one of these gates AND NOT governed by their flags; disabling a gate does not affect it.

A contradiction finding from `[in]` OR `[canon]` SHALL feed the audit's existing validation-retry loop: the authoring agent is re-invoked with the findings appended to its prompt AND rewrites the unit (delete-and-rewrite), bounded by the same `max_validation_retries` budget that governs `--strict` retries. The agent MAY resolve a finding by aligning the change to canon (reusing canonical vocabulary), by writing a `MODIFIED` delta of the contradicted canonical requirement, OR by converting the unit to an issue. A resolution SHALL NOT dissolve a `[canon]` finding by silently bending the contradicted requirement to fit: a canon-changing resolution SHALL be a legible `MODIFIED` delta whose contract change is stated plainly in the proposal's rationale.

On exhausting the retry budget with the contradiction unresolved, the audit SHALL NOT commit the offending unit AND SHALL resolve that unit to `AuditOutcome::DidNotComplete` — the fail-closed framework's "found a finding it could not persist as a clean unit" disposition — surfaced via the existing audit-failure path. The interactive human handoff for such a residue is provided separately.

#### Scenario: Enabled gates run against the written change before commit
- **WHEN** a spec-writing audit produces a spec-lane change that passes `openspec validate --strict` AND the `[in]` and/or `[canon]` gate is enabled
- **THEN** the audit runs each enabled gate's contradiction check against that change before committing it
- **AND** the checks use the same prompts, MCP submission tools, and opt-in flags as the implement-time gates

#### Scenario: A contradiction feeds the existing retry loop
- **WHEN** the `[canon]` check returns one or more contradictions for the written change AND the retry budget is not exhausted
- **THEN** the audit re-invokes the authoring agent with the findings appended to its prompt AND the agent rewrites the unit (delete-and-rewrite)
- **AND** the rewritten unit is re-checked on the next attempt

#### Scenario: Self-heal by aligning to canon
- **WHEN** the agent resolves a `[canon]` contradiction by reusing the canonical vocabulary so the change no longer conflicts with the existing requirement
- **THEN** the rewritten change passes the `[canon]` check AND is committed
- **AND** no canonical requirement is modified by the change

#### Scenario: Self-heal by converting to an issue
- **WHEN** the agent judges that the finding's correct resolution is a behavior-preserving fix with no contract change AND `features.issues` is enabled
- **THEN** it converts the unit to an `issues/<slug>/` unit (which then runs the issue contract-change check)
- **AND** the original spec-lane change directory is not committed

#### Scenario: A canon-changing resolution is legible, not laundered
- **WHEN** the only correct resolution is to change a canonical contract
- **THEN** the rewritten change carries a `MODIFIED` delta of the contradicted requirement AND states the contract change in the proposal's rationale
- **AND** the audit does NOT make the finding vanish by quietly altering the requirement to match the original change

#### Scenario: Exhausted budget fails closed without committing
- **WHEN** the retry budget is exhausted AND a `[in]` or `[canon]` contradiction remains unresolved
- **THEN** the audit does NOT commit the offending unit
- **AND** it resolves that unit to `AuditOutcome::DidNotComplete` (the found-but-could-not-persist disposition) AND surfaces the failure via the audit-failure path

#### Scenario: A disabled gate is not run at authoring time
- **WHEN** the `[canon]` gate is disabled (`executor.change_canonical_contradiction_check` unset)
- **THEN** the audit does NOT run a `[canon]` check at authoring time
- **AND** the change is committed on `--strict` success as before (the `[canon]` verifier gate likewise does not run at implement time for this change, consistent with the gate being disabled)

#### Scenario: A clean change commits unchanged
- **WHEN** the written change passes `--strict` AND every enabled gate returns no contradictions
- **THEN** the audit commits it exactly as it does today
- **AND** the enabled implement-time gates still run as the backstop and find it clean

### Requirement: Audit-authored issues are checked for hidden contract changes at authoring time
When a spec-writing audit writes an issue-lane unit (`issues/<slug>/`), it SHALL run an authoring-time contract-change check before committing the unit, whenever the `[canon]` gate is enabled (`executor.change_canonical_contradiction_check`). The check is an `agentic_run` session in a read-only sandbox that reads the unit's `issue.md` AND the relevant canonical specs AND judges whether implementing the issue would require changing a canonical contract — the same canon-consistency judgment the `[canon]` gate applies to spec deltas, applied here to an issue that claims (by carrying no spec delta) to change no contract.

If implementing the issue would require a contract change, the audit SHALL re-route the unit to the spec lane within the retry budget (after which the spec-lane gate checks apply); an unresolved case SHALL be rejected — the unit is NOT committed AND resolves to the fail-closed found-but-could-not-persist disposition. This authoring-time check is the early complement to the implement-time issue kick-back ("Issue-flavored implementer prompt verifies against existing canon"): both enforce that an issue carries no contract change, one before the unit is committed and one at run time as the backstop.

#### Scenario: An honest issue passes the contract-change check
- **WHEN** an audit-authored issue's fix preserves observed behavior of already-correctly-specified code AND the `[canon]` gate is enabled
- **THEN** the contract-change check finds no required contract change AND the issue is committed
- **AND** no spec delta is produced for it

#### Scenario: An issue needing a contract change is re-routed to the spec lane
- **WHEN** the contract-change check finds that implementing the issue would require changing a canonical contract AND the retry budget is not exhausted
- **THEN** the audit re-routes the unit to the spec lane AND the spec-lane gate checks then apply to it
- **AND** the issue-lane unit is not committed as an issue

#### Scenario: An unresolved contract-change issue is rejected, not committed
- **WHEN** the contract-change finding cannot be resolved within the retry budget
- **THEN** the unit is NOT committed AND resolves to `AuditOutcome::DidNotComplete` (the found-but-could-not-persist disposition)
- **AND** the failure is surfaced via the audit-failure path

#### Scenario: Disabled canon gate skips the authoring-time issue check
- **WHEN** the `[canon]` gate is disabled
- **THEN** the audit does NOT run the authoring-time contract-change check
- **AND** the issue is committed on its structural validity (an `issue.md` and `tasks.md`, no `specs/`), with the implement-time issue kick-back — a separate always-on mechanism, NOT the `[canon]` gate — remaining as the backstop
