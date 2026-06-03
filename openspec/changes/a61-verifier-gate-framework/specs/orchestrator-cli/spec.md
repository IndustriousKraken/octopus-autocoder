# orchestrator-cli — delta for a61-verifier-gate-framework

## ADDED Requirements

### Requirement: Verifier-gate framework
autocoder's change-lifecycle consistency checks SHALL be organized as a verifier-gate framework of exactly three named gates positioned around the executor run:

- the `[in]` gate — change-internal consistency, run BEFORE the executor;
- the `[canon]` gate — change-vs-canonical consistency, run BEFORE the executor;
- the `[out]` gate — code-implements-spec, run AFTER the executor.

Each gate SHALL be individually opt-in AND SHALL own its disposition: the pre-executor gates (`[in]`, `[canon]`) are fail-open — a gate's own failure (transport, parse, unregistered strategy, no submission) logs a WARN AND never blocks the iteration; the `[out]` gate is advisory — it annotates operator surfaces AND never auto-acts (no revision, no block). Each gate's diagnostics (log lines AND any operator surface it writes) SHALL carry the gate's stable identifier so a finding is attributable to the gate that produced it.

The `[in]` gate IS the existing change-internal contradiction pre-flight check (its own requirement defines its behavior, opt-in gating, fail-open posture, marker, AND alert); this framework reframes that check under the `[in]` identifier WITHOUT changing what it decides, its config key, OR its alert category. The `[canon]` AND `[out]` gates are realized by subsequent changes; until a gate is realized the framework treats it as absent AND invokes nothing for it. This change introduces ONLY the shared gate vocabulary, lifecycle positions, posture rules, AND labeling — it does NOT add a new gate.

#### Scenario: The `[in]` gate runs the contradiction check, labeled
- **WHEN** the `[in]` gate runs for a change
- **THEN** it executes the change-internal contradiction pre-flight check unchanged in what it decides (same opt-in gating, fail-open posture, marker, AND alert category)
- **AND** its emitted log / diagnostic lines carry the `[in]` gate identifier so the finding is attributable to that gate

#### Scenario: An unrealized gate is inert
- **WHEN** the `[canon]` OR `[out]` gate has not been realized by a subsequent change
- **THEN** resolving that gate yields "no installed gate"
- **AND** the framework invokes nothing for it — no gate is run speculatively

#### Scenario: Gate disposition follows the gate's lifecycle position
- **WHEN** a pre-executor gate (`[in]` or `[canon]`) fails for its own reasons
- **THEN** the framework treats it as fail-open: it logs a WARN AND does NOT block the iteration
- **WHEN** the `[out]` gate produces findings
- **THEN** the framework treats them as advisory: they annotate operator surfaces AND do NOT auto-trigger a revision or block
