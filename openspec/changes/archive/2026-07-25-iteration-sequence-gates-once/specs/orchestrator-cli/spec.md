# orchestrator-cli delta: iteration-sequence-gates-once

## MODIFIED Requirements

### Requirement: Verifier-gate framework
autocoder's change-lifecycle consistency checks SHALL be organized as a verifier-gate framework of the following named gates positioned around the executor run:

- the `[in]` gate — change-internal consistency, run BEFORE the executor;
- the `[canon]` gate — change-vs-canonical consistency, run BEFORE the executor;
- the `[rules]` gate — change-vs-global-rules consistency, run BEFORE the executor;
- the `[out]` gate — code-implements-spec, run AFTER the executor.

Each gate SHALL be individually opt-in AND SHALL own its disposition, but NO gate treats an inability to run as a pass (the gatekeepers-fail-closed standard). The pre-executor gates (`[in]`, `[canon]`, `[rules]`) FAIL CLOSED: a gate's own failure (transport, parse, unregistered strategy, no submission) does NOT proceed as "no findings" — it holds the change in an explicit failed-to-run state (the change was NOT evaluated), surfaces a distinct "gate FAILED TO RUN — change held" alert, AND halts the iteration; an operator clears the hold (after fixing the gate) to retry. The `[out]` gate is advisory — it never auto-acts (no revision, no block) — AND fails to a VISIBLE state: on its own failure it renders an explicit "FAILED TO RUN" section rather than silently omitting one. Each gate's diagnostics (log lines AND any operator surface it writes) SHALL carry the gate's stable identifier so a finding — OR a held/failed-to-run state — is attributable to the gate that produced it.

Pre-executor gate evaluation SHALL be SEQUENCE-scoped, not invocation-scoped. When every enabled pre-executor gate passes for a change and the executor then requests an iteration, the recorded pass covers the continuation iterations of that sequence: at gate-pass time the daemon SHALL record a content hash of the change directory's gate inputs (every regular file under the change's directory, excluding daemon bookkeeping markers) in a state-dir gate-pass record keyed to the workspace and change. A continuation pickup — one whose iteration-pending marker is present — whose recomputed hash equals the record SHALL NOT re-spawn the pre-executor gate sessions: for gate-invocation purposes it does not re-enter the pre-executor pipeline; the sequence's recorded verdicts stand and the verdict ledger renders them annotated as carried forward. No verdict is manufactured — the gates judged exactly these bytes when the sequence began, and the record only extends that judgment across its own sequence. Any hash difference, a missing or unreadable record, or any error computing the hash SHALL run the gates in full: the skip fails toward RUNNING, never toward passing. The record is removed whenever the sequence terminates (wherever the iteration-pending marker is dropped — `Completed`, `Failed`, or `SpecNeedsRevision`), so a fresh sequence always re-gates, and it is replaced whenever the gates run in full and pass again. The per-gate requirements define each gate's behavior WHEN it runs; this framework requirement owns WHEN the gates are invoked.

The `[in]` gate IS the existing change-internal contradiction pre-flight check (its own requirement defines its behavior, opt-in gating, fail-closed posture, marker, AND alert); this framework reframes that check under the `[in]` identifier. The `[canon]`, `[rules]`, AND `[out]` gates are realized by their own requirements; until a gate is realized the framework treats it as absent AND invokes nothing for it.

#### Scenario: The `[in]` gate runs the contradiction check, labeled
- **WHEN** the `[in]` gate runs for a change
- **THEN** it executes the change-internal contradiction pre-flight check (same opt-in gating, fail-closed posture, marker, AND alert category)
- **AND** its emitted log / diagnostic lines carry the `[in]` gate identifier so the finding is attributable to that gate

#### Scenario: The `[rules]` gate runs the global-rules check, labeled
- **WHEN** the `[rules]` gate runs for a change
- **THEN** it executes the global-rules pre-flight check against the global rule corpus (pre-executor, opt-in, fail-closed)
- **AND** its emitted log / diagnostic lines carry the `[rules]` gate identifier so a violation is attributable to that gate

#### Scenario: An unrealized gate is inert
- **WHEN** a gate named in the framework has not been realized by any change
- **THEN** resolving that gate yields "no installed gate"
- **AND** the framework invokes nothing for it — no gate is run speculatively

#### Scenario: Gate disposition follows the gate's lifecycle position
- **WHEN** a pre-executor gate (`[in]`, `[canon]`, or `[rules]`) fails for its own reasons (transport, parse, unregistered strategy, no submission)
- **THEN** the framework treats it as fail-CLOSED: it holds the change in an explicit failed-to-run state, surfaces it, AND does NOT proceed to the executor as if the gate passed
- **WHEN** the `[out]` gate fails for its own reasons
- **THEN** the framework renders an explicit "FAILED TO RUN" section (advisory, never blocking) rather than omitting one
- **WHEN** the `[out]` gate produces findings
- **THEN** the framework treats them as advisory: they annotate operator surfaces AND do NOT auto-trigger a revision or block

#### Scenario: A continuation iteration with unchanged inputs does not re-spawn gate sessions
- **WHEN** a change is picked up as a continuation (its iteration-pending marker is present) AND a gate-pass record exists for it AND the recomputed inputs hash equals the recorded hash
- **THEN** no pre-executor gate session is spawned for this pickup
- **AND** the verdict ledger renders the sequence's recorded verdicts annotated as carried forward

#### Scenario: A mid-sequence edit re-gates in full
- **WHEN** a change is picked up as a continuation AND any regular file under its directory changed since the record was written (the recomputed hash differs)
- **THEN** the pre-executor gates run in full, exactly as for a fresh pickup
- **AND** a new record is written only if they all pass

#### Scenario: Any doubt runs the gates
- **WHEN** a change is picked up as a continuation AND the gate-pass record is missing or unreadable, OR the inputs hash cannot be computed
- **THEN** the pre-executor gates run in full (the skip fails toward running, never toward passing)

#### Scenario: A fresh sequence always gates
- **WHEN** a change is picked up with no iteration-pending marker
- **THEN** the pre-executor gates run regardless of any leftover gate-pass record
- **AND** the record is replaced when they pass AND removed wherever the sequence terminates
