## ADDED Requirements

### Requirement: Canon and the archive are autocoder-only; executor sessions are guarded from both
Folding a change's deltas into canonical specs under `openspec/specs/`, AND archiving a change (running `openspec archive`, OR creating/moving an entry under `openspec/changes/archive/` or `issues/archive/`), are autocoder's responsibilities — performed by the daemon ONLY after a change is implemented AND merged. NO executor session — the changes-lane implementer, the spec-revision (`send it`) executor, a spec-writing audit, audit triage, OR a proposer — SHALL modify canon OR archive a change as part of its run. A session that does so bypasses the post-executor gate AND double-applies on merge.

This invariant SHALL be enforced in depth across three layers:

1. **Teach (prompt).** Each spec-writing/implementing/revision session's prompt SHALL direct the agent to the repository's `OCTOPUS.md` for the workflow conventions AND these constraints, when that file is present; absence is a graceful no-op (the session proceeds; the sandbox AND revert layers still apply).
2. **Prevent (sandbox).** Each such session's sandbox SHALL deny executing `openspec archive` AND SHALL deny writes under `openspec/specs/`. The session's OWN planning artifact remains writable — the change's `openspec/changes/<slug>/` directory (including its `specs/` delta subdirectory) OR the issue's `issues/<slug>` unit — as do the implementer's code edits; only canon AND the archive are off-limits.
3. **Catch (revert).** After the session AND before its work is committed, any modification to `openspec/specs/` AND any created/modified entry under `openspec/changes/archive/` or `issues/archive/` that the session produced SHALL be reverted, AND the violation SHALL be surfaced (an operator-visible alert), mirroring the audit planning-lane post-hoc revert. The revert is fail-closed: it does not depend on the prompt or sandbox having held.

A change's own spec-delta directory (`openspec/changes/<slug>/specs/`) is NOT canon AND is explicitly writable by the proposer, audits, AND the spec-revision executor; the distinction the guard draws is between a change's deltas (writable) AND the canonical specs the archive later folds them into (autocoder-only).

#### Scenario: A session that edits canon has the edit reverted and surfaced
- **WHEN** an executor session modifies a file under `openspec/specs/` during its run
- **THEN** that modification is reverted before the session's work is committed
- **AND** an operator-visible alert names the session AND the attempted canon edit

#### Scenario: A session that archives a change has the archive reverted and surfaced
- **WHEN** an executor session runs `openspec archive` OR creates an entry under `openspec/changes/archive/` (or `issues/archive/`) during its run
- **THEN** the sandbox denies the `openspec archive` execution, AND any archive entry the session created is reverted before commit
- **AND** the violation is surfaced to the operator

#### Scenario: The change's own delta directory stays writable
- **WHEN** the proposer, a spec-writing audit, OR the spec-revision executor writes under the change's `openspec/changes/<slug>/specs/` directory
- **THEN** the write is permitted (it is the change's delta, not canon)
- **AND** the implementer's code edits outside `openspec/` are likewise permitted

#### Scenario: The session prompt points at OCTOPUS.md when present
- **WHEN** a spec-writing/implementing/revision session starts AND an `OCTOPUS.md` exists at the repository root
- **THEN** the session prompt directs the agent to read it for the workflow conventions AND the canon/archive constraints
- **WHEN** no `OCTOPUS.md` is present
- **THEN** the session proceeds AND the sandbox AND revert layers still enforce the invariant
