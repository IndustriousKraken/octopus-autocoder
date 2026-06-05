# orchestrator-cli — delta for a009-issues-lane-curated

## ADDED Requirements

### Requirement: Issues lane for corrections
The daemon SHALL provide a second work lane, `issues/`, for corrections — fixes to code that is already correctly specified (bug fixes, behavior-preserving refactors) that carry NO spec delta. An issue SHALL be a directory `issues/<slug>/` containing `issue.md` (the report and diagnosis AND the acceptance criteria stated against the EXISTING specification) AND `tasks.md` (the fix steps), with NO `specs/` directory — that absence is the contract that an issue changes no spec. The lane SHALL be gated by a `features.issues` flag, off by default. The curated entry path is a maintainer committing `issues/<slug>/` directly (repository write is the allowlist; no public surface). On completion the issue directory SHALL move to `issues/archive/`, mirroring `changes/archive/`, AND no canonical spec SHALL be modified (the issues lane leaves an audit trail only).

#### Scenario: An enabled lane works a committed issue
- **WHEN** `features.issues` is on AND an `issues/<slug>/` with `issue.md` and `tasks.md` is present
- **THEN** the issue is selected and worked
- **AND** no spec delta is required for it

#### Scenario: An issue carrying a specs directory is rejected
- **WHEN** an `issues/<slug>/` contains a `specs/` directory
- **THEN** it is rejected as malformed, because an issue carries no spec delta

#### Scenario: Completion archives without touching canon
- **WHEN** an issue's fix completes
- **THEN** `issues/<slug>/` moves to `issues/archive/`
- **AND** no canonical spec file is modified

#### Scenario: The lane is disabled by default
- **WHEN** `features.issues` is unset
- **THEN** the issues lane is inactive AND `issues/<slug>/` directories are not worked

### Requirement: Independent lane walkers over shared utilities
The changes lane AND the issues lane SHALL be driven by separate walkers, each with its own control flow AND its own state file; lane-specific behavior SHALL live in each walker, NOT in shared branching keyed on a lane flag. Shared leaf functionality — the busy-marker, PR opening, archiving, chatops notification, queue-state I/O, AND workspace handling — SHALL be composed from stateless shared utilities that both walkers call. A fault in one walker SHALL NOT corrupt the other lane's control flow or state: each walker reads AND writes only its own lane's state.

#### Scenario: Each walker owns its state
- **WHEN** both lanes have ready work for a repository
- **THEN** each walker reads and writes only its own lane's state file

#### Scenario: Shared leaf operations are stateless utilities
- **WHEN** either walker opens a PR, archives a unit, or posts a chatops notification
- **THEN** it calls the shared stateless utility rather than a lane-private copy

#### Scenario: One definition per shared primitive
- **WHEN** the codebase is searched after this change
- **THEN** the busy-marker, PR-open, archive, chatops-notify, queue-state, and workspace primitives each have a single definition composed by both walkers

### Requirement: Lane precedence — issues over changes over audits
Within the existing per-repo serializer (the busy-marker — one unit of work per repository at a time), each iteration SHALL select the highest-precedence READY unit in the order issues > changes > audits, extending the established changes-over-audits precedence. Within a lane, selection SHALL be alphabetical. Issue-precedence SHALL be strict: a ready issue beats a ready change. Anti-starvation is provided by the promotion gate (issues enter the lane only after maintainer approval), NOT by a scheduling fairness rule. Because issues run first, a change may later find its work already done; a plain failure for such a change is acceptable, AND no rebase-precheck is performed.

#### Scenario: A ready issue beats a ready change
- **WHEN** both a ready issue and a ready change exist for a repository
- **THEN** the issue is selected first

#### Scenario: A ready change beats a ready audit
- **WHEN** a ready change and a ready audit exist AND no issue is ready
- **THEN** the change is selected before the audit

#### Scenario: Alphabetical within a lane
- **WHEN** two issues are ready for a repository
- **THEN** they are selected in alphabetical order
