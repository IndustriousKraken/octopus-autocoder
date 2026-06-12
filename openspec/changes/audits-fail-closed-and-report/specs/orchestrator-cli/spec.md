## ADDED Requirements

### Requirement: Audit runs fail closed to a non-passing did-not-complete outcome
Every audit run SHALL initialize its outcome to an explicit non-passing "did not complete" state, conforming to the project-documentation `gatekeepers-fail-closed` standard. That initial state SHALL be overwritten ONLY by an evidenced terminal verdict: a session that demonstrably ran to completion AND either produced its expected artifact OR positively declared a survey conclusion. A run that cannot produce such evidence SHALL resolve to a surfaced did-not-complete outcome — never to a passing `NoFindings` / empty `SpecsWritten` result.

The audit framework SHALL expose a `DidNotComplete { audit_type, cause, examined_summary }` outcome variant. `cause` distinguishes at least: a session error (timeout, non-zero exit, **OR an exit status that was not captured**); a session that ended without declaring any terminal verdict; and a session that declared findings it could not persist. The scheduler SHALL treat `DidNotComplete` like the existing audit-failure path — it SHALL NOT advance the audit's cadence state AND it SHALL surface the failure (chatops alert when a backend is configured) — and SHALL keep it distinct from `NoFindings`, `SpecsWritten`, and `WorkspaceUnavailable`.

For a specs-writing audit, "no findings" SHALL be backed by the agent's positive declaration that it examined the code and reached that conclusion; the mere absence of new change directories SHALL NOT by itself be reported as "no findings." A specs-writing audit's terminal outcome — its written-proposals result OR its did-not-complete result — SHALL carry an `examined_summary` (the agent's account of what it looked at) so that even a clean run is accompanied by evidence the audit actually ran, and so the on-demand completion notification can report it.

#### Scenario: Outcome is non-passing until an evidenced verdict overwrites it
- **WHEN** an audit run begins
- **THEN** its outcome is initialized to a non-passing did-not-complete state
- **AND** only a session that ran to completion AND produced its expected artifact OR positively declared a survey conclusion may overwrite that state with a passing outcome

#### Scenario: Uncaptured exit status is a failure, not a pass
- **WHEN** an audit's wrapped session ends AND no exit status was captured (e.g. the process was signal-killed)
- **THEN** the audit resolves to `DidNotComplete { cause: <session-errored>, .. }`
- **AND** the scheduler does NOT advance the cadence state AND surfaces the failure
- **AND** the outcome is NOT `NoFindings` or empty `SpecsWritten`

#### Scenario: Findings that cannot be persisted are surfaced, not dropped
- **WHEN** a specs-writing audit's agent declares it found one or more issues but no valid change directory was persisted for them
- **THEN** the audit resolves to `DidNotComplete { cause: <found-but-could-not-persist>, .. }`
- **AND** a chatops alert is posted AND the cadence state is NOT advanced
- **AND** the run is NOT reported as "0 findings"

#### Scenario: A specs-writing outcome carries an examined summary
- **WHEN** a specs-writing audit reaches a terminal outcome (proposals written, no findings, or did-not-complete)
- **THEN** the outcome carries an `examined_summary` describing what the audit looked at, available to the on-demand completion notification and its conclusion

### Requirement: On-demand audit triggers carry their chat origin and receive a terminal completion notification
When an audit is triggered on demand — via the chatops `audit` verb or the CLI `audit run` subcommand against a running daemon — the originating chat context (channel and thread identifiers, when present) SHALL be carried through the `queue_audit` control-socket action and onto the queued entry, so the daemon can reply to the operator who asked. After the queued audit reaches a terminal outcome, the scheduler SHALL post a terminal completion notification to that origin. A cadence-driven (not operator-triggered) run carries no origin and SHALL NOT emit this completion notification (its existing findings / failure notifications are unchanged).

#### Scenario: queue_audit carries the originating chat context
- **WHEN** an operator triggers `@<bot> audit <type> <repo>` from a chat thread
- **THEN** the submitted `queue_audit` action includes the originating channel and thread identifiers
- **AND** the queued entry retains that origin until the audit runs

#### Scenario: A completed on-demand audit replies to the operator's thread
- **WHEN** an operator-triggered audit reaches a terminal outcome
- **THEN** the scheduler posts a completion notification to the originating thread reporting the terminal result (findings, no-findings with the examined summary, OR a did-not-complete failure with its cause)

#### Scenario: A cadence-driven run emits no completion notification
- **WHEN** an audit runs because its cadence came due (no operator trigger, no origin)
- **THEN** no on-demand completion notification is posted
- **AND** the audit's existing findings / failure notification behavior is unchanged

### Requirement: On-demand audit-run queue survives pass-skip, early-return, and daemon restart
A queued on-demand audit SHALL be removed from the `pending_audit_runs` queue ONLY after the audit has actually run. When the polling pass that would run it is skipped (busy marker), returns early before the audit phase (workspace-init failure), or is bounded out (`max_audits_per_iteration: 0`), the queued entry SHALL remain for a later iteration rather than being discarded. The queue SHALL additionally be persisted such that a daemon restart between the enqueue acknowledgement and the run does not lose the queued audit.

#### Scenario: A busy-skipped pass does not lose the queued audit
- **WHEN** an audit is queued AND the next pass skips because a busy marker is held
- **THEN** the queued entry is still present for the following iteration
- **AND** the audit runs once a non-skipped pass reaches the audit phase

#### Scenario: A workspace-init failure does not lose the queued audit
- **WHEN** an audit is queued AND the next pass returns early because `ensure_initialized` failed
- **THEN** the queued entry is still present for the following iteration

#### Scenario: An audit bound of zero defers rather than discards
- **WHEN** an audit is queued AND `max_audits_per_iteration` is `0`
- **THEN** the audit phase runs no audits this iteration AND the queued entry is retained for a later iteration

#### Scenario: A restart between enqueue and run preserves the queued audit
- **WHEN** an audit is queued AND the daemon restarts before the audit has run
- **THEN** the queued audit is restored on startup AND runs on a subsequent iteration

## MODIFIED Requirements

### Requirement: Missing-tests audit
autocoder SHALL register a `missing_tests_audit` audit in the periodic-audit framework. The audit invokes the wrapped agent CLI with an OpenSpec-only sandbox and a missing-tests prompt; it creates new OpenSpec change directories under `openspec/changes/`, commits them to the agent branch, and returns the created change names so the same iteration's queue walk implements them. The audit is `requires_head_change = true` and `WritePolicy::OpenSpecOnly`.

#### Scenario: Invokes the CLI with an OpenSpec-only sandbox
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

#### Scenario: Audit creates new OpenSpec changes
- **WHEN** the audit identifies N coverage gaps (where N is
  capped by `audits.missing_tests_audit.max_proposals_per_run`,
  default `2`)
- **THEN** the audit creates N change directories at
  `openspec/changes/<change_name>/` where each contains a
  proposal.md, tasks.md, and (when the gap implies a capability
  invariant) a `specs/<capability>/spec.md` delta
- **AND** each created change is named with a `tests-` prefix
  (e.g. `tests-error-paths-in-queue-engine`) so operators can
  recognize audit-produced changes at a glance

#### Scenario: Audit commits created changes to agent branch
- **WHEN** the agent finishes creating files
- **THEN** the audit framework's WritePolicy::OpenSpecOnly check
  passes (every modified path is under `openspec/changes/`)
- **AND** the audit runs `git add openspec/changes/ && git commit
  -m "audit: missing-tests proposals (N change(s))"`
- **AND** the audit returns
  `AuditOutcome::SpecsWritten(change_names)` where
  `change_names` is the list of newly-created change directory
  names

#### Scenario: Same iteration's queue walk picks up created changes
- **WHEN** the audit returns `SpecsWritten(names)` AND the
  iteration proceeds to `list_pending`
- **THEN** `list_pending` observes the new directories (they have
  `proposal.md`, no `.in-progress`, no `.question.json`)
- **AND** the iteration's `walk_queue` includes them in its
  archive cap, ordered by their `proposal.md` mtime
  (per the existing time-based ordering)

#### Scenario: Cap on proposals per run
- **WHEN** the prompt would produce more than
  `max_proposals_per_run` changes
- **THEN** the prompt instructs the agent to pick the N highest-
  priority gaps (by severity / risk) and emit only those
- **AND** the agent does NOT create more than N changes in this
  run; remaining gaps will be re-surfaced on subsequent runs as
  the audit re-evaluates the codebase

#### Scenario: Write outside openspec/changes triggers framework revert
- **WHEN** the agent writes a file outside `openspec/changes/`
  (e.g. a `src/foo.rs` modification or a `README.md` edit)
- **THEN** the foundation's `WritePolicy::OpenSpecOnly` post-hoc
  check fails AND the framework reverts via `git reset --hard
  HEAD + git clean -fd`
- **AND** the audit is treated as failed (state NOT updated,
  chatops alert posted, audit re-runs next iteration)
- **AND** no OpenSpec changes are committed from this run

#### Scenario: Genuine no-findings is declared, not inferred from absence
- **WHEN** the audit's session runs to completion AND the agent positively declares that it examined the code and identified zero meaningful coverage gaps
- **THEN** the audit returns `AuditOutcome::SpecsWritten(vec![])` carrying the agent's examined-summary
- **AND** no commit is made, AND no chatops post is sent for this clean periodic run (per framework behavior for spec-writing audits)

#### Scenario: A run with no terminal declaration is surfaced, never reported as no-findings
- **WHEN** the audit's session ends without the agent positively declaring a survey conclusion (it errored, its exit status was not captured, or it produced output but persisted no valid change directory)
- **THEN** the audit returns `AuditOutcome::DidNotComplete { .. }`, NOT `SpecsWritten(vec![])`
- **AND** the cadence state is NOT advanced AND a chatops alert is posted

### Requirement: Security & bug audit
autocoder SHALL register a `security_bug_audit` audit in the periodic-audit framework. The audit invokes the wrapped agent CLI with an OpenSpec-only sandbox and a security-and-bug-detection prompt; it creates new OpenSpec change directories under `openspec/changes/` describing proposed fixes, commits them, and returns the change names so the same iteration implements them. The audit is `requires_head_change = true` and `WritePolicy::OpenSpecOnly`.

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

#### Scenario: Created changes use fix- or secure- prefix
- **WHEN** the audit creates a change for a proposed fix
- **THEN** the change directory name uses `fix-` prefix for bug
  fixes (e.g. `fix-off-by-one-in-queue-walker`) AND `secure-`
  prefix for security hardening (e.g.
  `secure-sanitize-user-paths`)
- **AND** the operator can recognize audit-produced security/bug
  changes by their prefix at a glance

#### Scenario: Each proposed change includes a fix specification
- **WHEN** the audit creates a change
- **THEN** the change SHALL contain:
  - `proposal.md` naming the issue, citing the source location,
    and explaining the fix.
  - `tasks.md` listing the implementation steps.
  - When the fix implies a capability invariant (e.g. "every
    operation X SHALL validate Y"), a `specs/<capability>/spec.md`
    delta MODIFYING the relevant requirement OR adding a new
    requirement.
- **AND** validation via `openspec validate <name> --strict`
  passes before the audit commits the change

#### Scenario: Validation failure rejects the change without committing
- **WHEN** the agent produces a change that fails `openspec
  validate --strict`
- **THEN** the audit deletes the offending change directory AND
  records a WARN log entry naming the validation error
- **AND** the audit does NOT chatops-alert per-change validation
  failures (the audit-run log is sufficient operator signal)
- **AND** if every proposed change fails validation, the audit
  returns `AuditOutcome::SpecsWritten(vec![])` and no commit
  is made

#### Scenario: Per-run proposal cap
- **WHEN** the agent would produce more than
  `max_proposals_per_run` (default `2`) changes
- **THEN** the prompt instructs the agent to pick the
  highest-severity issues and emit only those
- **AND** the cap is enforced post-hoc: if the agent produces
  more, the audit keeps the first N (in directory-listing order
  after the post-run snapshot) and deletes the rest with a WARN
  log

#### Scenario: Write outside openspec/changes triggers framework revert
- **WHEN** the agent writes a file outside `openspec/changes/`
  (attempts to fix the bug directly, edits a source file, etc.)
- **THEN** the foundation's `WritePolicy::OpenSpecOnly` post-hoc
  check fails AND the framework reverts via
  `git reset --hard HEAD + git clean -fd`
- **AND** the audit is treated as failed; chatops alert posted;
  the audit re-runs next iteration

#### Scenario: Genuine no-findings is declared, not inferred from absence
- **WHEN** the audit's session runs to completion AND the agent positively declares that it examined the code and identified zero confident security or bug issues
- **THEN** the audit returns `AuditOutcome::SpecsWritten(vec![])` carrying the agent's examined-summary
- **AND** no commit is made, no chatops post is sent, AND the iteration proceeds normally

#### Scenario: A run with no terminal declaration is surfaced, never reported as no-findings
- **WHEN** the audit's session ends without the agent positively declaring a survey conclusion (it errored, its exit status was not captured, or it identified an issue it could not persist as a change directory)
- **THEN** the audit returns `AuditOutcome::DidNotComplete { .. }`, NOT `SpecsWritten(vec![])`
- **AND** the cadence state is NOT advanced AND a chatops alert is posted so the inability to run is visible
