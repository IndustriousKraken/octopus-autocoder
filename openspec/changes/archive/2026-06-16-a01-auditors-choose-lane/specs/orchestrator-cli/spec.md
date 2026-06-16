## ADDED Requirements

### Requirement: Spec-writing bug/gap audits choose their output lane by canon judgment
The `security_bug_audit` AND `missing_tests_audit` audits SHALL choose the output lane for each finding by canon-grounded judgment, never defaulting to the spec lane. Before proposing a fix the audit SHALL read the canonical spec(s) for the capability the finding touches. A finding whose fix changes an observable contract (public API, serialized/wire format, CLI surface, a state machine, OR a new/changed invariant), OR whose correct fix requires changing a canonical requirement, SHALL be written to the spec lane (`openspec/changes/<slug>/`). A finding whose fix preserves the observed behavior of code that is already correctly specified SHALL be written to the issues lane (`issues/<slug>/`, containing `issue.md` AND `tasks.md`, with NO `specs/` directory). The audit SHALL reuse canonical vocabulary AND SHALL prefer a `MODIFIED` delta of an existing requirement over an `ADDED` requirement that introduces a parallel term for a concept canon already names.

Lane choice SHALL be gated by the `features.issues` flag for the repository: when the flag is off, ONLY the spec lane is offered AND the audit behaves as it did before this capability existed. The audits SHALL run under a `WritePolicy` permitting writes under BOTH `openspec/changes/` AND `issues/`, reverting any write outside those two planning lanes. `canon_consolidation_audit` is excluded from lane choice — it exists to evolve canon AND is spec-lane by definition.

#### Scenario: Lane is chosen by contract impact, never defaulted
- **WHEN** a bug/gap audit produces a finding AND `features.issues` is on for the repository
- **THEN** it selects the lane by whether the fix changes an observable contract — spec lane for a contract change, issues lane for a behavior-preserving fix
- **AND** it does NOT route to the spec lane by default

#### Scenario: Canon is read and its vocabulary reused
- **WHEN** the audit frames a fix
- **THEN** it has read the canonical spec(s) for the area of the finding
- **AND** it reuses the canonical vocabulary rather than coining a new term for a concept canon already names
- **AND** it prefers a `MODIFIED` delta of the existing requirement over an `ADDED` requirement introducing a parallel term

#### Scenario: A contract-correcting fix uses a legible MODIFIED delta
- **WHEN** the correct fix requires changing a canonical requirement (canon permits or mandates the defective behavior)
- **THEN** the audit writes a spec-lane change carrying a `MODIFIED` delta of that requirement
- **AND** it states the contract change plainly in the proposal's rationale rather than burying it inside an issue

#### Scenario: With features.issues off only the spec lane is offered
- **WHEN** `features.issues` is off for the repository
- **THEN** the audit writes only `openspec/changes/` units AND the issue lane is not offered
- **AND** its behavior is unchanged from before this capability existed

#### Scenario: A write outside the two planning lanes is reverted
- **WHEN** the agent writes any path outside BOTH `openspec/changes/` AND `issues/` (a source edit, a doc edit, a config change)
- **THEN** the audit's `WritePolicy` post-run check fails AND the framework reverts via `git reset --hard HEAD && git clean -fd`
- **AND** the run is treated as failed: cadence state is NOT advanced, a chatops alert is posted, AND the audit re-runs next iteration

## MODIFIED Requirements

### Requirement: Missing-tests audit
autocoder SHALL register a `missing_tests_audit` audit in the periodic-audit framework. The audit invokes the wrapped agent CLI with a writable planning-lanes sandbox and a missing-tests prompt. Per the `Spec-writing bug/gap audits choose their output lane by canon judgment` requirement, it reads the relevant canonical specs and routes each finding to the spec lane (`openspec/changes/<slug>/`) OR, when `features.issues` is enabled and the fix carries no contract change, the issues lane (`issues/<slug>/`); it commits the produced units to the agent branch and returns the created unit names so the same iteration's queue walk works them. The audit is `requires_head_change = true` and `WritePolicy::PlanningLanes`.

#### Scenario: Invokes the CLI with a writable planning-lanes sandbox
- **WHEN** the audit runs
- **THEN** autocoder spawns the configured `executor.command` with
  a sandbox whose `allowed_tools` includes `Write` and `Edit`
  alongside the read tools
- **AND** the prompt is the embedded
  `prompts/missing-tests-audit.md` template OR the
  operator-supplied override at
  `audits.missing_tests_audit.prompt_path`

#### Scenario: Prompt instructs additive-only output
- **WHEN** the prompt is loaded
- **THEN** the prompt explicitly states:
  - "Do NOT propose deleting existing tests."
  - "Do NOT propose modifying existing tests unless they are
    factually broken (failing or unreachable). When in doubt,
    leave the existing test alone and propose a NEW test."
  - "Suppress trivial gaps: getters, setters, single-line
    constructors, `Default` impls, `From`/`Into` conversions
    with no behavior."
- **AND** the prompt directs the agent to focus on uncovered
  error paths, edge cases, and branches without assertions

#### Scenario: Audit creates units in the chosen lane
- **WHEN** the audit identifies N coverage gaps (where N is
  capped by `audits.missing_tests_audit.max_proposals_per_run`,
  default `2`)
- **THEN** the audit creates N units, each in the lane it chose:
  a spec-lane unit at `openspec/changes/<slug>/` (a proposal.md,
  tasks.md, and — when the gap implies a capability invariant — a
  `specs/<capability>/spec.md` delta), OR an issue-lane unit at
  `issues/<slug>/` (an `issue.md` and `tasks.md`, no `specs/`
  directory)
- **AND** each created unit is named with a `tests-` prefix
  (e.g. `tests-error-paths-in-queue-engine`) so operators can
  recognize audit-produced units at a glance

#### Scenario: Audit commits created units to agent branch
- **WHEN** the agent finishes creating files
- **THEN** the audit framework's `WritePolicy::PlanningLanes` check
  passes (every modified path is under `openspec/changes/` OR
  `issues/`)
- **AND** the audit stages the produced planning lanes and commits
  (`git commit -m "audit: missing-tests proposals (N unit(s))"`)
- **AND** the audit returns
  `AuditOutcome::SpecsWritten(unit_names)` where `unit_names` is
  the list of newly-created unit directory names across whichever
  lanes were used

#### Scenario: Same iteration's queue walk picks up created units
- **WHEN** the audit returns `SpecsWritten(names)` AND the
  iteration proceeds to lane selection
- **THEN** the lane walker for each produced unit observes its new
  directory (the changes walker for `openspec/changes/<slug>/`, the
  issues walker for `issues/<slug>/`)
- **AND** the iteration works them under the established lane
  precedence (issues over changes), ordered within a lane per the
  existing rule

#### Scenario: Cap on proposals per run
- **WHEN** the prompt would produce more than
  `max_proposals_per_run` units
- **THEN** the prompt instructs the agent to pick the N highest-
  priority gaps (by severity / risk) and emit only those
- **AND** the agent does NOT create more than N units in this
  run; remaining gaps will be re-surfaced on subsequent runs as
  the audit re-evaluates the codebase

#### Scenario: Write outside the planning lanes triggers framework revert
- **WHEN** the agent writes a file outside BOTH `openspec/changes/`
  AND `issues/` (e.g. a `src/foo.rs` modification or a `README.md`
  edit)
- **THEN** the foundation's `WritePolicy::PlanningLanes` post-hoc
  check fails AND the framework reverts via `git reset --hard
  HEAD + git clean -fd`
- **AND** the audit is treated as failed (state NOT updated,
  chatops alert posted, audit re-runs next iteration)
- **AND** no units are committed from this run

#### Scenario: A behavior-preserving gap routes to the issues lane
- **WHEN** `features.issues` is on AND a coverage gap is in code
  that is already correctly specified AND closing it changes no
  observable contract
- **THEN** the audit writes an `issues/<slug>/` unit (`issue.md`
  with acceptance stated against the existing specification, plus
  `tasks.md`, no `specs/`)
- **AND** it does NOT create an `openspec/changes/` unit for that
  finding

#### Scenario: A gap implying a new invariant routes to the spec lane
- **WHEN** closing a gap requires asserting a new or changed
  capability invariant (a contract change)
- **THEN** the audit writes an `openspec/changes/<slug>/` unit with
  the `specs/<capability>/` delta
- **AND** it does NOT route that finding to the issues lane

#### Scenario: Genuine no-findings is declared, not inferred from absence
- **WHEN** the audit's session runs to completion AND the agent positively declares that it examined the code and identified zero meaningful coverage gaps
- **THEN** the audit returns `AuditOutcome::SpecsWritten(vec![])` carrying the agent's examined-summary
- **AND** no commit is made, AND no chatops post is sent for this clean periodic run (per framework behavior for spec-writing audits)

#### Scenario: A run with no terminal declaration is surfaced, never reported as no-findings
- **WHEN** the audit's session ends without the agent positively declaring a survey conclusion (it errored, its exit status was not captured, or it produced output but persisted no valid unit directory)
- **THEN** the audit returns `AuditOutcome::DidNotComplete { .. }`, NOT `SpecsWritten(vec![])`
- **AND** the cadence state is NOT advanced AND a chatops alert is posted

### Requirement: Security & bug audit
autocoder SHALL register a `security_bug_audit` audit in the periodic-audit framework. The audit invokes the wrapped agent CLI with a writable planning-lanes sandbox and a security-and-bug-detection prompt. Per the `Spec-writing bug/gap audits choose their output lane by canon judgment` requirement, it reads the relevant canonical specs and routes each finding to the spec lane (`openspec/changes/<slug>/` describing the proposed fix) OR, when `features.issues` is enabled and the fix carries no contract change, the issues lane (`issues/<slug>/`); it commits the produced units and returns the unit names so the same iteration implements them. The audit is `requires_head_change = true` and `WritePolicy::PlanningLanes`.

The prompt's confidence-filtering and scope guidance below is design intent verified by the drift audit's semantic judgment; it SHALL NOT be pinned by a unit test asserting verbatim substrings of the prompt (per the project-documentation requirement `Tests assert behavior or derivation, never message wording`).

#### Scenario: Prompt steers the agent toward high-confidence, in-scope findings
- **WHEN** the security-bug audit prompt is loaded
- **THEN** it instructs the agent to report only findings it is
  reasonably confident about and to err toward NOT reporting when
  uncertain, because a false positive becomes wasted implementer
  work downstream
- **AND** it instructs the agent not to propose stylistic
  "best-practice" changes that do not address a concrete security
  issue or bug
- **AND** it scopes findings to concrete in-scope categories
  (injection, auth/authz mistakes, hard-coded secrets, unsafe
  deserialization, missing input validation at trust boundaries,
  race conditions, resource leaks, off-by-one, wrong operator,
  mishandled None/null, missing error propagation) and excludes
  out-of-scope categories (code style, naming, architectural
  opinions, performance unless measurable, anything the project
  has explicitly accepted)

#### Scenario: Created units use fix- or secure- prefix
- **WHEN** the audit creates a unit for a proposed fix
- **THEN** the unit directory name uses `fix-` prefix for bug
  fixes (e.g. `fix-off-by-one-in-queue-walker`) AND `secure-`
  prefix for security hardening (e.g.
  `secure-sanitize-user-paths`), in whichever lane it lands
- **AND** the operator can recognize audit-produced security/bug
  units by their prefix at a glance

#### Scenario: Each proposed unit includes its fix specification
- **WHEN** the audit creates a unit
- **THEN** a spec-lane change SHALL contain:
  - `proposal.md` naming the issue, citing the source location,
    and explaining the fix.
  - `tasks.md` listing the implementation steps.
  - When the fix implies a capability invariant (e.g. "every
    operation X SHALL validate Y"), a `specs/<capability>/spec.md`
    delta MODIFYING the relevant requirement OR adding a new
    requirement.
- **AND** an issue-lane unit SHALL contain `issue.md` (the issue,
  the source location, AND acceptance criteria stated against the
  EXISTING specification) and `tasks.md`, with NO `specs/`
  directory
- **AND** for a spec-lane change, validation via `openspec validate
  <name> --strict` passes before the audit commits it

#### Scenario: Validation failure rejects the spec-lane change without committing
- **WHEN** the agent produces a spec-lane change that fails
  `openspec validate --strict`
- **THEN** the audit deletes the offending change directory AND
  records a WARN log entry naming the validation error
- **AND** the audit does NOT chatops-alert per-change validation
  failures (the audit-run log is sufficient operator signal)
- **AND** if every proposed unit fails validation, the audit
  returns `AuditOutcome::SpecsWritten(vec![])` and no commit
  is made

#### Scenario: Per-run proposal cap
- **WHEN** the agent would produce more than
  `max_proposals_per_run` (default `2`) units
- **THEN** the prompt instructs the agent to pick the
  highest-severity issues and emit only those
- **AND** the cap is enforced post-hoc: if the agent produces
  more, the audit keeps the first N (in directory-listing order
  after the post-run snapshot) and deletes the rest with a WARN
  log

#### Scenario: Write outside the planning lanes triggers framework revert
- **WHEN** the agent writes a file outside BOTH `openspec/changes/`
  AND `issues/` (attempts to fix the bug directly, edits a source
  file, etc.)
- **THEN** the foundation's `WritePolicy::PlanningLanes` post-hoc
  check fails AND the framework reverts via
  `git reset --hard HEAD + git clean -fd`
- **AND** the audit is treated as failed; chatops alert posted;
  the audit re-runs next iteration

#### Scenario: A behavior-preserving fix routes to the issues lane
- **WHEN** `features.issues` is on AND the finding is a defect in
  code that is already correctly specified AND the fix changes no
  observable contract
- **THEN** the audit writes an `issues/<slug>/` unit (`issue.md`
  with acceptance stated against the existing specification, plus
  `tasks.md`, no `specs/`)
- **AND** it does NOT create an `openspec/changes/` unit for that
  finding

#### Scenario: A contract-changing fix routes to the spec lane
- **WHEN** the fix requires new or changed observable behavior, OR
  canon itself permits/mandates the defective behavior and must be
  corrected
- **THEN** the audit writes an `openspec/changes/<slug>/` unit with
  the appropriate delta
- **AND** it does NOT route that finding to the issues lane

#### Scenario: Genuine no-findings is declared, not inferred from absence
- **WHEN** the audit's session runs to completion AND the agent positively declares that it examined the code and identified zero confident security or bug issues
- **THEN** the audit returns `AuditOutcome::SpecsWritten(vec![])` carrying the agent's examined-summary
- **AND** no commit is made, no chatops post is sent, AND the iteration proceeds normally

#### Scenario: A run with no terminal declaration is surfaced, never reported as no-findings
- **WHEN** the audit's session ends without the agent positively declaring a survey conclusion (it errored, its exit status was not captured, or it identified an issue it could not persist as a unit directory)
- **THEN** the audit returns `AuditOutcome::DidNotComplete { .. }`, NOT `SpecsWritten(vec![])`
- **AND** the cadence state is NOT advanced AND a chatops alert is posted so the inability to run is visible
