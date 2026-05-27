## ADDED Requirements

### Requirement: `.alert-state.json` SHALL NOT appear in the managed workspace post-migration
The workspace directory of a managed repository SHALL NOT contain a file named `.alert-state.json` after the daemon's first-startup migration (per `a16`) completes. This requirement codifies the architectural intent that daemon bookkeeping lives in `<state_dir>/`, never in the managed repository's workspace. The requirement catches future code drift that might re-introduce a workspace-rooted `.alert-state.json` AND surfaces such drift via a workspace-init invariant check.

#### Scenario: Workspace-init invariant check
- **WHEN** the daemon's workspace-init step runs against a workspace
- **AND** the file `<workspace>/.alert-state.json` exists AND the migration marker `<state_dir>/alert-state/.migration-from-workspace-done` is also present (indicating the migration completed previously)
- **THEN** the daemon logs WARN naming the unexpected file path AND the likely cause (code drift OR fresh clone of a repo whose history transiently included the file)
- **AND** removes the workspace file (since the migration marker says the migration completed, any file appearing now is a regression OR a transient — either way the operator-visible behavior is the file going away)

#### Scenario: Pre-migration workspaces are not affected by this requirement
- **WHEN** the daemon's workspace-init step runs against a workspace AND the migration marker is NOT present
- **THEN** this requirement does NOT trigger any action
- **AND** the migration logic (per `a16`'s orchestrator-cli requirement) handles the workspace file at startup time

#### Scenario: Other workspace-local bookkeeping files are unaffected
- **WHEN** this requirement evaluates a workspace
- **THEN** files OTHER than `.alert-state.json` are not inspected — `.audit-state.json`, `.failure-state.json`, `.perma-stuck.json`, `.needs-spec-revision.json`, `.in-progress*`, AND any change-directory marker files (`.question.json`, `.answer.json`) are out of scope for this spec
- **AND** future specs MAY extend the same architectural principle to those files; this spec deliberately scopes to alert-state only to keep the migration risk bounded
