## MODIFIED Requirements

### Requirement: Verifier-gate framework
autocoder's change-lifecycle consistency checks SHALL be organized as a verifier-gate framework of the following named gates positioned around the executor run:

- the `[in]` gate — change-internal consistency, run BEFORE the executor;
- the `[canon]` gate — change-vs-canonical consistency, run BEFORE the executor;
- the `[rules]` gate — change-vs-global-rules consistency, run BEFORE the executor;
- the `[out]` gate — code-implements-spec, run AFTER the executor.

Each gate SHALL be individually opt-in AND SHALL own its disposition, but NO gate treats an inability to run as a pass (the gatekeepers-fail-closed standard). The pre-executor gates (`[in]`, `[canon]`, `[rules]`) FAIL CLOSED: a gate's own failure (transport, parse, unregistered strategy, no submission) does NOT proceed as "no findings" — it holds the change in an explicit failed-to-run state (the change was NOT evaluated), surfaces a distinct "gate FAILED TO RUN — change held" alert, AND halts the iteration; an operator clears the hold (after fixing the gate) to retry. The `[out]` gate is advisory — it never auto-acts (no revision, no block) — AND fails to a VISIBLE state: on its own failure it renders an explicit "FAILED TO RUN" section rather than silently omitting one. Each gate's diagnostics (log lines AND any operator surface it writes) SHALL carry the gate's stable identifier so a finding — OR a held/failed-to-run state — is attributable to the gate that produced it.

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

### Requirement: Gate dispositions are enforced by a default-deny verdict ledger rendered in the PR
The verifier gates' fail-closed disposition SHALL be enforced **structurally** — by a per-change gate-verdict ledger whose default is non-passing — NOT by per-path inspection of a gate's result. Inspection requires every code path (every result arm, every error, every future early-return) to be classified correctly; a single missed path inherits whatever the fall-through is, which is how a gate silently fails open. A default-deny ledger removes that class of bug: "open" requires an affirmative, completed `PASS`, so a crash, an unhandled path, or a runner that never ran leaves the change held by construction.

For each change under gate evaluation, every gate slot (`[in]`, `[canon]`, `[rules]`, `[out]`) SHALL have a verdict in the ledger, INITIALIZED to `PENDING` (a non-passing state). A verdict SHALL become `PASS` ONLY by an explicit, completed clean result. The verdict set is: `PENDING` (default — a runner that never recorded a verdict; treated as held), `PASS` (ran, clean), `FAIL` (ran, findings), `FAILED_TO_RUN` (ran, could not produce a verdict), `DISABLED` (gate not configured; non-blocking).

There SHALL be no skip/absent code path among the slots the ledger carries: the ledger holds one slot per REALIZED gate, AND every such slot — whether its gate is enabled OR disabled — SHALL run a runner that affirmatively writes a verdict. A disabled gate's runner is a STUB that writes `DISABLED`; stamping that verdict is NOT invoking the gate — no check runs speculatively, consistent with the framework's posture toward inactive gates. A gate that no change has realized is ABSENT per the `Verifier-gate framework` requirement: it has no ledger slot AND no runner until it is realized, at which point it gains both. This keeps "unrealized" (no installed gate, no slot) distinct from "disabled" (installed, configured off, slot records `DISABLED`), AND eliminates the disabled-vs-failed ambiguity at the structural level — "disabled" is an explicit recorded verdict, never an absence that a reader must remember to treat as a pass.

The executor SHALL be invoked ONLY when every BLOCKING gate (`[in]`, `[canon]`, `[rules]`) is `PASS` or `DISABLED`. A blocking gate that is `PENDING`, `FAIL`, or `FAILED_TO_RUN` SHALL hold the change. Because the default is `PENDING`, any failure to affirmatively record `PASS` holds the change without the holding code having to anticipate the specific failure. (`[rules]` is a pre-executor blocking gate, like `[in]` and `[canon]`; `[out]` is advisory and non-blocking.)

The ledger SHALL be rendered into the PR body as a compliance record: per gate, its identifier, the model that ran it, AND its verdict (with a one-line summary for `FAIL` / `FAILED_TO_RUN`). A `PASS` is therefore VISIBLE in the PR — the operator can see which gate ran, with which model, and that it passed — rather than inferred from the silent absence of an alert. The agentic reviewer's verdict SHALL likewise appear in the PR record.

#### Scenario: A blocking gate left PENDING holds the change
- **WHEN** a blocking gate's runner does not record a verdict (it crashes, an unhandled path is taken, or it never runs) so the ledger entry remains `PENDING`
- **THEN** the change is HELD (the executor is NOT invoked) — `PENDING` is non-passing by construction
- **AND** no code path needs to anticipate the specific failure for the hold to occur

#### Scenario: A disabled gate records DISABLED via a stub
- **WHEN** a gate is realized but not configured (disabled)
- **THEN** its slot's stub runner records `DISABLED` (a non-blocking verdict), NOT an absence
- **AND** the executor proceeds (a disabled gate does not hold the change)
- **AND** the gate's own check is NOT invoked (stamping `DISABLED` is not running the gate)

#### Scenario: An unrealized gate has no ledger slot
- **WHEN** a gate named in the framework has not been realized by any change
- **THEN** the ledger carries no slot for it AND invokes no runner — not even a stub — consistent with the `Verifier-gate framework` requirement treating it as absent
- **AND** when a later change realizes the gate, it gains both a ledger slot AND a runner that affirmatively writes its verdict

#### Scenario: The executor runs only when blocking gates are PASS or DISABLED
- **WHEN** the gate ledger for a change is evaluated before the executor
- **THEN** the executor is invoked ONLY if every blocking gate (`[in]`, `[canon]`, `[rules]`) is `PASS` or `DISABLED`
- **AND** any blocking gate that is `PENDING`, `FAIL`, or `FAILED_TO_RUN` holds the change

#### Scenario: The PR body renders the gate ledger as a compliance record
- **WHEN** a change reaches PR creation
- **THEN** the PR body contains a gate-verdict section listing, per gate, its identifier, the model that ran it, AND its verdict
- **AND** a `PASS` is visible there (not inferred from silence), so an operator can judge whether a verdict came from a model they trust

## ADDED Requirements

### Requirement: Global-rules pre-flight check (the [rules] gate)
autocoder SHALL provide an opt-in pre-flight check — the `[rules]` gate of the verifier framework — that detects whether a single OpenSpec change's spec deltas VIOLATE any rule in the global rule corpus, before the executor is invoked. It is the corpus-parameterized sibling of the `[canon]` gate: the SAME machinery — a CLI-wrapped agentic session through the shared `agentic_run` primitive (a56) in a read-only sandbox — but the comparison corpus is the global rule corpus (per `Global rules are authored as minimal prose, not contract language`) instead of the project's canonical specs, AND each finding names the violated rule (by its stable id) rather than a canonical requirement. The session returns its findings via the `submit_rule_violations` MCP tool. On non-empty findings, autocoder SHALL write `.needs-spec-revision.json` with `revision_suggestion` populated from the rule-violation narrative (naming each violated rule), post the existing `AlertCategory::SpecNeedsRevision` chatops alert, AND halt the queue walk for this iteration. The executor SHALL NOT be invoked when violations are found. The marker is the same semantic-finding shape the `[canon]` gate writes (empty `unimplementable_tasks`, no `gate_error`), so the interactive revision flow handles it identically.

The check SHALL be gated by `executor.global_rules_check` (`disabled` default, `enabled` opt-in). The model is configured via `executor.global_rules_check_llm` (parallel to the `[canon]` gate's block). The corpus location is configured via `executor.global_rules.corpus` (a path OR repo the daemon can read; the spec-box has its own copy). Enabling the check without BOTH a configured model AND a resolvable corpus SHALL fail at daemon startup with a fail-fast validation error.

The gate reads the rule corpus per the rule protocol: at small scale it feeds all rules to the session; the protocol's retrieval hook governs selection once the corpus outgrows the context window. The gate SHALL function with the corpus flat OR grouped into registers.

Per the verifier framework, the `[rules]` gate SHALL FAIL CLOSED (gatekeepers-fail-closed standard) AND SHALL label its diagnostics with the `[rules]` identifier: an agentic-session error (spawn, timeout, OR a resolved CLI strategy that is not registered), a schema-rejected submission the agent never corrects, a session that ends with no submission, OR any other could-not-run failure SHALL NOT be treated as "no violations found." It SHALL log a WARN AND hold the change in an explicit failed-to-run state — write `.needs-spec-revision.json` with a structured `gate_error` population, post the distinct "gate FAILED TO RUN — change held" alert, AND halt the iteration; an operator clears the marker (after fixing the gate) to retry. A schema-invalid `submit_rule_violations` call mid-session is a correctable tool error the agent can retry (a56). A successful session that returns an empty array is a clean result AND proceeds to the executor.

Because it is a verifier gate, the `[rules]` gate runs server-side pre-executor (the enforcement guarantee that no change in any repo violates the global rules); its normative behavior, enabling, and fail-closed posture are fully defined by that server-side path and do NOT depend on any local runner. Where a local gate runner (the `verify` subcommand) is available, the same check runs against the same corpus as an accelerator.

#### Scenario: Default-disabled produces no [rules] session
- **WHEN** `executor.global_rules_check` is unset (default `disabled`)
- **AND** any change reaches the pre-executor pipeline
- **THEN** no `[rules]` session is spawned
- **AND** the executor is invoked normally (assuming the earlier gates passed)

#### Scenario: Enabled mode checks the deltas against the rule corpus
- **WHEN** `executor.global_rules_check: enabled` AND the model AND corpus are configured
- **AND** a change reaches the pre-executor pipeline
- **THEN** the gate runs an `agentic_run` read-only session (`Read`/`Glob`/`Grep`, `ORCH_MCP_ROLE = global_rules_check`, the `submit_rule_violations` MCP tool) that reads the change's spec-delta files AND the global rule corpus
- **AND** the agent returns violations by calling `submit_rule_violations` with `{ violations: [{ rule_id, summary }] }`

#### Scenario: A violation writes the marker and halts
- **WHEN** the agent submits one or more rule violations
- **THEN** the pipeline writes `.needs-spec-revision.json` with `revision_suggestion` naming each violated rule by id
- **AND** the `AlertCategory::SpecNeedsRevision` alert fires (subject to the throttle)
- **AND** the executor is NOT invoked for this change OR any subsequent change in this iteration

#### Scenario: Empty submission proceeds to executor
- **WHEN** the agent calls `submit_rule_violations` with an empty `violations` array
- **THEN** the pipeline proceeds to the executor
- **AND** no marker is written AND no alert fires

#### Scenario: Session or submission failure holds the change (fail closed)
- **WHEN** the agentic session fails (spawn error, timeout, unregistered strategy) OR ends with no schema-valid `submit_rule_violations` call
- **THEN** the gate logs a WARN (carrying the `[rules]` label) naming the cause
- **AND** writes `.needs-spec-revision.json` with a structured `gate_error`, posts the "gate FAILED TO RUN — change held" alert, AND does NOT proceed to the executor

#### Scenario: Enabled without model or corpus fails fast at startup
- **WHEN** `executor.global_rules_check: enabled` AND either `executor.global_rules_check_llm` OR `executor.global_rules.corpus` is unset/unresolvable
- **THEN** daemon startup fails with a named error AND does NOT begin polling
