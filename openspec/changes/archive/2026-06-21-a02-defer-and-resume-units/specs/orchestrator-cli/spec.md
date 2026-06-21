## ADDED Requirements

### Requirement: Defer and resume a change or issue without deleting or revising it
The orchestrator SHALL provide a defer operation that sets a work unit aside — out of both work lanes — without deleting OR revising it, AND an inverse undefer operation that resumes it. The motivating case: a change that has gone perma-stuck (repeatedly kicked back) can be set aside intact instead of being forced through the loop OR rolled back; the work is preserved AND reactivated when ready. A defer SHALL preserve the unit's tracked content byte-for-byte AND SHALL NOT clear any marker it carries — it is neither a delete nor a revise. (A unit's GITIGNORED runtime markers — the `.perma-stuck.json` park marker, the `.in-progress` lock — are not committed, so they do not travel through the PR-borne move; a resumed unit therefore re-enters its lane WITHOUT its prior parked/locked state. That is the intended fresh re-attempt — defer sets work aside, it does not freeze a perma-stuck state.)

A unit SHALL be EITHER a change OR an issue, auto-detected by where it lives — the operator names only a slug:

- A CHANGE lives at `openspec/changes/<slug>/` (the changes lane's enumeration root).
- An ISSUE lives at `issues/<slug>.md` (single-file form) OR `issues/<slug>/` (directory form) — the two on-disk forms the issues lane accepts.

The defer operation SHALL MOVE the unit out of its lane into a committed location at the repository root that NEITHER lane enumerates:

- A change: `openspec/changes/<slug>/` → `deferred-changes/<slug>/`.
- An issue: `issues/<slug>.md` → `deferred-issues/<slug>.md`, OR `issues/<slug>/` → `deferred-issues/<slug>/` — the single-file-versus-directory form SHALL be preserved exactly.

The undefer operation SHALL be the exact inverse move, returning the unit to its original lane location in its original form.

The operation SHALL ride the established agent-branch + push + PR flow rather than committing to the base branch directly: it performs the move on the agent branch (recreated at the base tip) AND goes through the SAME push + PR-creation path as any change, honoring the per-repo `auto_submit_pr` setting — a pull request when it is enabled (the default), OR a pushed agent branch with no PR (the `BranchPushedNoPr` outcome) when an installation has set it false. The operation SHALL NOT commit to the base branch directly: a base commit diverges from `origin/<base>`, breaks the per-pass `git pull --ff-only`, is wiped by the dirty-workspace recovery (`git reset --hard origin/<base>` + `git clean -fd`), AND violates the prohibition on base-branch commits outside a PR. The PR body SHALL state what was deferred (or resumed) AND from/to which location.

Like `Code-rollback recovery`, defer AND undefer are workspace-mutating control-socket operations: they SHALL conform to the **Workspace-mutating control-socket operations preempt and serialize against the pass** requirement — preempting any in-flight pass for the repository AND holding the per-repo busy marker for the duration of the move — so the move never races a concurrent agentic session writing the same workspace (the corruption that the unsandboxed daemon git and the workspace-writable agentic child would otherwise cause).

Because defer discards no code AND is fully reversible, it SHALL require only a normal acknowledgement — NOT the two-step confirmation the destructive `Code-rollback recovery` operation requires. This is an explicit contrast: rollback discards code, so it confirms; defer preserves everything, so it does not.

The operation SHALL be idempotent at its edges: deferring a slug already in the deferred area (AND absent from its lane) is a no-op success reporting it is already deferred; undeferring a slug already back in its lane (AND absent from the deferred area) is a no-op success reporting it is already active. A slug present in NEITHER lane is a clear not-found error; a slug naming BOTH a change AND an issue is a clear ambiguous error that names both candidates AND performs no move.

#### Scenario: deferring a change moves it out of the lane via the PR flow
- **WHEN** an operator defers a slug that is a change at `openspec/changes/<slug>/`
- **THEN** the change directory is moved to `deferred-changes/<slug>/` on the agent branch (recreated at the base tip), preserving its contents AND any markers
- **AND** the operation rides the normal push + PR flow (a PR when `auto_submit_pr` is enabled, the default; otherwise a pushed branch with no PR — the `BranchPushedNoPr` outcome), NOT a direct base-branch commit
- **AND** the PR body states the change was deferred AND names the source/destination location

#### Scenario: deferring an issue preserves its on-disk form
- **WHEN** an operator defers a slug that is an issue
- **THEN** a single-file issue moves `issues/<slug>.md` → `deferred-issues/<slug>.md`, AND a directory-form issue moves `issues/<slug>/` → `deferred-issues/<slug>/`
- **AND** the form (single-file versus directory) is preserved exactly, including any sibling or in-directory markers

#### Scenario: undefer returns the unit to its original lane location
- **WHEN** an operator undefers a slug currently in the deferred area
- **THEN** the unit is moved back to its original lane location in its original form (`deferred-changes/<slug>/` → `openspec/changes/<slug>/`, OR `deferred-issues/<slug>(.md|/)` → `issues/<slug>(.md|/)`) via the same agent-branch + push + PR flow
- **AND** the unit re-enters its lane's selection on the next polling iteration (assuming no other marker excludes it)

#### Scenario: a deferred unit is invisible to both lanes
- **WHEN** a change is at `deferred-changes/<slug>/` OR an issue is at `deferred-issues/<slug>(.md|/)`
- **THEN** the changes lane's enumeration (which reads only `openspec/changes/`) does NOT return the deferred change
- **AND** the issues lane's enumeration (which reads only `issues/`) does NOT return the deferred issue
- **AND** neither lane works the deferred unit — it is set aside until undeferred, with no lane-enumeration change required

#### Scenario: defer preempts an in-flight pass and serializes against it
- **WHEN** an operator defers (OR undefers) a unit while a pass is in flight for that repository
- **THEN** the operation preempts the in-flight pass AND acquires the per-repo busy marker before moving the unit, per the preempt-and-serialize invariant
- **AND** the move never runs concurrently with the agentic session, so two writers cannot corrupt the same workspace

#### Scenario: defer requires only a normal acknowledgement, unlike rollback
- **WHEN** an operator invokes defer (OR undefer)
- **THEN** the operation acts on a single acknowledgement — there is no two-step confirmation, contrasting with `Code-rollback recovery`, which requires explicit confirmation because it discards code
- **AND** because defer preserves the unit byte-for-byte AND is reversible by undefer, no code OR spec work is at risk

#### Scenario: deferring an already-deferred unit is a no-op success
- **WHEN** an operator defers a slug that is already in the deferred area (absent from its lane)
- **THEN** the operation reports the unit is already deferred AND performs no move, no commit, AND no PR — a no-op success, not an error
- **AND** it detects the already-deferred state READ-ONLY, BEFORE any preempt or lock — it does NOT preempt an in-flight pass NOR acquire the busy marker for a no-op

#### Scenario: a not-found or ambiguous slug is a clear error
- **WHEN** an operator defers a slug present in NEITHER lane
- **THEN** the operation returns a clear not-found error AND performs no move
- **WHEN** an operator defers a slug that names BOTH a change AND an issue in the same repo
- **THEN** the operation returns a clear ambiguous error naming both candidate locations AND performs no move
