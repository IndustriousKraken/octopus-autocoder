## ADDED Requirements

### Requirement: Per-change atomicity in sync-specs rebuild
The `autocoder sync-specs --rebuild` subcommand SHALL treat each archived change as an atomic unit: either the change is successfully re-archived (`openspec archive` exited zero AND the post-condition holds), or the workspace is restored to its pre-change state via rollback. The active path `openspec/changes/<slug>/` SHALL NOT be left containing a directory the rebuild placed there if the change fails to archive. Failed changes SHALL be reported with the openspec output that explains the failure.

#### Scenario: Happy path leaves the change in archive with original date prefix
- **WHEN** `openspec archive <slug> -y` exits zero AND `openspec/changes/<slug>/` no longer exists AND exactly one directory matches `openspec/changes/archive/*-<slug>/` with a date prefix
- **THEN** the rebuild renames the matched archive directory to the change's original name (preserving its historical date prefix) when the names differ
- **AND** records a successful outcome for the change

#### Scenario: Silent skip rolls the workspace back
- **WHEN** `openspec archive <slug> -y` exits zero BUT `openspec/changes/<slug>/` still exists (openspec did not move the directory)
- **THEN** the rebuild moves `openspec/changes/<slug>/` back to `openspec/changes/archive/<original_name>/`
- **AND** records a failed outcome for the change whose `failure_reason` includes openspec's captured stdout AND stderr
- **AND** the operator's `openspec/changes/` directory contains no active-path entry for this slug after the rebuild

#### Scenario: Non-zero exit rolls the workspace back
- **WHEN** `openspec archive <slug> -y` exits non-zero
- **THEN** the rebuild moves `openspec/changes/<slug>/` back to `openspec/changes/archive/<original_name>/`
- **AND** records a failed outcome whose `failure_reason` includes the exit status AND openspec's captured stderr (or stdout when stderr is empty), each truncated to the existing report-size cap

#### Scenario: Data-loss-shaped failure is detected explicitly
- **WHEN** `openspec archive <slug> -y` exits zero AND `openspec/changes/<slug>/` no longer exists BUT NO directory matches `openspec/changes/archive/*-<slug>/`
- **THEN** the rebuild records a failed outcome whose `failure_reason` describes "openspec archive reported success but the change is missing from both the active path and the archive"
- **AND** does NOT attempt a rollback (there is nothing in the active path to roll back)

#### Scenario: Archive-directory collision is detected, not silently picked
- **WHEN** `openspec archive <slug> -y` exits zero AND `openspec/changes/<slug>/` no longer exists AND more than one directory matches `openspec/changes/archive/*-<slug>/`
- **THEN** the rebuild records a failed outcome whose `failure_reason` lists all matching paths and instructs the operator to manually consolidate them
- **AND** does NOT attempt to rename any of the matches (the rebuild cannot tell which one is canonical)

#### Scenario: Rollback failure does not crash the rebuild
- **WHEN** a rollback is required AND the rollback rename itself fails (e.g. destination already exists, filesystem permission)
- **THEN** the rebuild logs at CRITICAL with both the original failure and the rollback failure
- **AND** records a failed outcome whose `failure_reason` concatenates both messages
- **AND** continues processing the next archived change

### Requirement: openspec output is captured regardless of exit code
The rebuild SHALL capture `openspec`'s stdout and stderr for every invocation, not only when the exit code is non-zero. Captured output SHALL be included in the per-change failure report when the post-condition fails on an exit-zero call. This ensures the operator can see the upstream skip-reason without re-running the rebuild under tracing.

#### Scenario: Silent-skip failure reason contains openspec's actual output
- **WHEN** the rebuild reports a change as failed because of post-condition failure on an exit-zero openspec call
- **THEN** the `failure_reason` string contains a non-empty excerpt of openspec's stdout OR stderr
- **AND** the excerpt is bounded by the existing report-size cap so the summary stays readable

### Requirement: Success-path archive directory is observed, not guessed
The rebuild SHALL locate the resulting archive directory after a successful `openspec archive` call by matching `openspec/changes/archive/*-<slug>/` where the prefix matches the date pattern `^\d{4}-\d{2}-\d{2}-`, rather than by constructing a predicted name from today's date. This makes the success path robust to local-timezone differences between openspec and the rebuild, collision suffixes added by openspec, and any future change to openspec's archive-naming format.

#### Scenario: Glob match handles collision suffix
- **WHEN** openspec produces an archive directory named `archive/2026-05-25-<slug>-2/` (a collision suffix because `archive/2026-05-25-<slug>/` already existed from a prior run)
- **THEN** the glob match returns `archive/2026-05-25-<slug>-2/`
- **AND** the rebuild renames that path to the change's original name

#### Scenario: Glob match handles timezone-difference date
- **WHEN** the rebuild's UTC date is `2026-05-25` and openspec uses a different timezone whose date is `2026-05-26`
- **THEN** the glob match returns `archive/2026-05-26-<slug>/` (the actual path openspec created)
- **AND** the rebuild renames that path to the change's original name without relying on `today_dated_name`

#### Scenario: Glob match ignores entries without a date prefix
- **WHEN** an unrelated directory `archive/foo-<slug>/` exists (operator-placed sidecar, nested archive) AND `archive/2026-05-25-<slug>/` also exists
- **THEN** only the date-prefixed match is returned
- **AND** the unrelated directory is not renamed or touched
