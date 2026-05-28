## ADDED Requirements

### Requirement: OPERATIONS.md, STATE-LAYOUT.md, and TROUBLESHOOTING.md document the alert-state move
`docs/OPERATIONS.md`'s throttled-failure-alerts section SHALL name `<state_dir>/alert-state/<basename>.json` as the canonical path. `docs/STATE-LAYOUT.md` SHALL add `alert-state/` to the state-dir contents table AND remove `.alert-state.json` from any workspace-local-files table that lists it. `docs/TROUBLESHOOTING.md` SHALL gain a "git checkout fails with 'local changes to .alert-state.json'" entry describing the legacy-workspace case AND the migration's automatic handling on next daemon startup. `docs/OPERATIONS.md` SHALL also gain a "Migrations" section enumerating every migration marker the daemon checks at startup AND what each does.

#### Scenario: OPERATIONS.md throttled-alerts section names the new path
- **WHEN** an operator reads `docs/OPERATIONS.md`'s throttled-failure-alerts section
- **THEN** the prose names `<state_dir>/alert-state/<basename>.json` as the storage location
- **AND** does NOT reference a workspace-root `.alert-state.json` path (any pre-spec references are removed or updated)

#### Scenario: STATE-LAYOUT.md state-dir table includes alert-state
- **WHEN** an operator reads `docs/STATE-LAYOUT.md`'s state-dir contents table
- **THEN** an `alert-state/` row appears with the file-naming convention (`<workspace-basename>.json`) AND a one-line description of its purpose
- **AND** `.alert-state.json` no longer appears in any workspace-local-files table

#### Scenario: TROUBLESHOOTING.md helps operators hit by the legacy bug
- **WHEN** an operator reads `docs/TROUBLESHOOTING.md`
- **THEN** a section titled "git checkout fails with 'local changes to .alert-state.json'" describes the symptom
- **AND** the section explains that the daemon's first startup after upgrade migrates the file automatically (per `a16`'s migration)
- **AND** the section gives an immediate-fix recipe for operators stuck before the migration runs (rm the local file; the daemon recreates it at the new location on the next alert)

#### Scenario: OPERATIONS.md Migrations section is authoritative
- **WHEN** an operator reads `docs/OPERATIONS.md`'s Migrations section
- **THEN** the section enumerates every daemon-side migration marker, including the existing `state-paths-out-of-tmp` migration AND the new `alert-state-from-workspace` migration
- **AND** each entry names the marker file's path, when the migration runs, what it migrates, AND how to force a re-scan (remove the marker)
