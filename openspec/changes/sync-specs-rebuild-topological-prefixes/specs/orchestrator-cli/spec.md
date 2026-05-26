## ADDED Requirements

### Requirement: Dependency-aware ordering pre-pass in sync-specs rebuild
Before enumerating archived changes for chronological replay, the `autocoder sync-specs --rebuild` subcommand SHALL scan every archived change's spec deltas, build a dependency graph from `## MODIFIED Requirements` / `## REMOVED Requirements` / `## RENAMED Requirements` blocks to the changes that originally `## ADDED Requirements` those headers, and topologically reorder same-day archives so every ADDING change is processed before any change that operates on its requirement headers. The reordering is persisted as `aNN-` prefixes (two-digit zero-padded, after the date prefix) on the affected archive directory names so subsequent rebuilds see the dependency order encoded in alphabetical sort and no further reordering is needed.

#### Scenario: Same-day MODIFY-before-ADD inversion is automatically fixed
- **WHEN** the archive contains two same-day changes whose alphabetical order has a MODIFYING change sorting before its dependency-providing ADDING change
- **THEN** the pre-pass renames the ADDING change's directory to prefix it with `a01-` (after the date prefix) so it sorts first within the day-group
- **AND** the subsequent chronological-enumeration loop processes the ADDING change first
- **AND** the subsequent MODIFY succeeds against canonical state that now contains the required requirement

#### Scenario: Day with no within-day dependencies produces no renames
- **WHEN** all changes within a date prefix's day-group have no MODIFIED / REMOVED / RENAMED-FROM dependencies on requirements ADDED by other changes in the same day-group
- **THEN** the pre-pass produces zero `RenamePlan` entries for that day-group
- **AND** no archive directories in that day-group are renamed

#### Scenario: Minimum-renames principle
- **WHEN** a day-group requires reordering of K entries
- **THEN** only the K entries whose alphabetical position needs to change SHALL receive `aNN-` prefixes
- **AND** entries already in the correct alphabetical position SHALL NOT be renamed

#### Scenario: Renames are persistent across rebuild runs
- **WHEN** a second rebuild runs against an archive where a prior rebuild already applied `aNN-` prefix renames
- **THEN** the pre-pass produces zero new renames
- **AND** the archive directory names are unchanged

#### Scenario: Stable secondary sort preserves original alphabetical order
- **WHEN** two entries in a day-group have no mutual dependency
- **THEN** their relative order in the topological output matches their relative order in the original alphabetical sort

### Requirement: Rebuild aborts on unresolvable dependency conditions
The pre-pass SHALL detect two graph conditions that cannot be resolved by within-day prefix renames and SHALL abort the rebuild with a structured error before any rename or canonical-spec update is applied. The abort SHALL surface via `RebuildReport.abort_reason: Some(...)` carrying the offending change names and requirement headers, and SHALL post a chatops `❌` notification describing the condition.

#### Scenario: Cycle detection aborts the rebuild
- **WHEN** the dependency graph contains a cycle (e.g. A MODIFIES a requirement ADDED by B, and B MODIFIES a requirement ADDED by A)
- **THEN** the pre-pass returns `Err(RebuildAbortReason::Cycle { changes, requirements })` with both involved change names and both `(capability, requirement)` pairs populated
- **AND** the rebuild aborts without applying any renames
- **AND** the rebuild aborts without modifying any canonical spec files
- **AND** a chatops `❌` notification is posted naming both involved changes

#### Scenario: Cross-day backward dependency aborts the rebuild
- **WHEN** a change archived on day D MODIFIES / REMOVES / RENAMES-FROM a requirement first ADDED by a change archived on day D' where D' > D
- **THEN** the pre-pass returns `Err(RebuildAbortReason::CrossDayBackwardDependency { dependent, dependency, capability, requirement_header })`
- **AND** the rebuild aborts without applying any renames
- **AND** the rebuild aborts without modifying any canonical spec files
- **AND** a chatops `❌` notification is posted naming both involved changes and the date inversion

#### Scenario: Day-group with more than 99 reorderable entries aborts
- **WHEN** a single date prefix's day-group requires `aNN-` prefixes for more than 99 entries
- **THEN** the pre-pass returns `Err(RebuildAbortReason::ScanFailed { error })` whose message states "more than 99 same-day reorderable entries; manual intervention required"
- **AND** the rebuild aborts without applying any partial renames

### Requirement: Chatops notification surfaces the applied renames
When at least one rename is applied during a rebuild, the daemon SHALL post a chatops notification listing the renames before opening the rebuild PR. The notification groups renames by their date-group day, names each `FROM → TO`, and includes a one-line human-readable summary of the dependency that triggered each rename. When no renames are applied, no rename-notification fires (the existing PR-opened notification covers the normal case).

#### Scenario: Successful rebuild with renames posts the `🔀` notification
- **WHEN** `report.prefix_renames` is non-empty after a successful rebuild
- **THEN** the daemon posts a chatops notification whose first line is `🔀 <repo>: rebuild applied dependency-prefix renames in <N> day-group(s)`
- **AND** the body of the notification groups the renames by day
- **AND** each rename is listed in the form `<from> → <to>` with a parenthetical dependency_summary on the next line
- **AND** the notification is posted BEFORE the existing PR-opened notification so operators see the renames first

#### Scenario: Successful rebuild without renames posts no rename-notification
- **WHEN** `report.prefix_renames` is empty after a successful rebuild
- **THEN** no `🔀` notification is posted
- **AND** the existing PR-opened notification fires unchanged

#### Scenario: Notification failure does not block PR creation
- **WHEN** the chatops `post_notification` call fails (network blip, channel renamed, etc.) during the rename-notification post
- **THEN** the daemon logs at ERROR with the underlying error
- **AND** PR creation proceeds normally

### Requirement: PR body lists the renames
When the rebuild's `RebuildReport.prefix_renames` is non-empty, the generated PR body SHALL include a section titled `**Applied dependency-prefix renames**` listing each rename in the same `FROM → TO` form as the chatops notification, grouped by day. The section SHALL appear BEFORE the existing `**Canonical spec files**` section so the operator reviewing the PR diff sees the renames first and can decide whether to keep, edit, or reject them.

#### Scenario: Rename section appears in the PR body
- **WHEN** the rebuild applied at least one rename and successfully produced a PR
- **THEN** the PR body contains a section titled `**Applied dependency-prefix renames**`
- **AND** the section appears before the `**Canonical spec files**` section
- **AND** the section lists every rename grouped by day with dependency summaries
