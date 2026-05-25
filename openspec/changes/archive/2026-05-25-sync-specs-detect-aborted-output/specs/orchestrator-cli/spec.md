## ADDED Requirements

### Requirement: Detect openspec abort marker in stdout
The `autocoder sync-specs --rebuild` subcommand SHALL inspect every successful (`exit 0`) `openspec archive` invocation's stdout for an abort marker BEFORE running the post-condition check. The marker is any line whose first non-whitespace token is `Aborted.` (with the trailing period). When the marker is present, the rebuild SHALL treat the archive call as failed regardless of the exit code: rollback runs, the change is recorded as failed, and the failure_reason starts with `openspec refused to apply: <reason>` where `<reason>` is the most informative preceding line (typically openspec's diagnostic that immediately precedes the `Aborted.` line). The post-condition check remains in place as a defense-in-depth fallback for cases where openspec's wording changes or the marker is absent.

#### Scenario: Aborted marker on its own line triggers failure path
- **WHEN** `openspec archive <slug> -y` exits 0 AND its stdout contains a line `Aborted. No files were changed.`
- **THEN** the rebuild treats the call as failed
- **AND** `record_failure_with_rollback` is invoked with `original_name`
- **AND** the change directory is moved back to `openspec/changes/archive/<original_name>/`
- **AND** the `ChangeOutcome.failure_reason` starts with `openspec refused to apply:`

#### Scenario: Preceding line is captured as the headline reason
- **WHEN** openspec stdout contains the lines `member-saved-cards MODIFIED failed for header "..." - not found\nAborted. No files were changed.`
- **THEN** the `failure_reason` headline is `openspec refused to apply: member-saved-cards MODIFIED failed for header "..." - not found`
- **AND** the full openspec output (subject to the existing report-size cap) is included after the headline so the operator has the complete context

#### Scenario: Word "aborted" mid-sentence does not trigger detection
- **WHEN** openspec stdout contains the substring `aborted` (lowercase, mid-sentence) but no line whose first non-whitespace token is `Aborted.`
- **THEN** the abort-marker detection returns `None`
- **AND** the rebuild proceeds to the post-condition check as if no marker were present

#### Scenario: Post-condition check remains as fallback
- **WHEN** openspec silently skips a change without emitting the `Aborted.` marker (e.g. a future openspec version changes its wording)
- **THEN** the abort-marker detection returns `None` and the rebuild proceeds to `verify_archive_post_condition`
- **AND** the post-condition check catches the silent skip via the existing `ActivePathStillPresent` path
- **AND** rollback runs through the existing per-change atomicity contract

### Requirement: Rebuild PR body accurately describes rollback behavior
The rebuild's generated PR body SHALL describe failures as rolled back to archive rather than left at the active path, matching the actual behavior introduced by the atomicity contract. The rebuild summary line SHALL include the rolled-back count when greater than zero, so the operator can confirm at a glance that the rollback count matches the failure count. When the counts differ (data-loss-shaped failures, rollback-of-rollback failures), the gap is visible in the summary and explained per-change in the failures list.

#### Scenario: Failed-rebuild PR body header describes rollback
- **WHEN** the rebuild generates a PR body for a run with at least one failed change
- **THEN** the failures-section header reads `**Failed changes** (rolled back to archive — see failure reasons below for the openspec output explaining each):`
- **AND** the header does NOT contain the phrase `left at active path`

#### Scenario: Summary line includes rolled-back count when non-zero
- **WHEN** the rebuild processed N changes, S succeeded, F failed, R rolled back, with R > 0
- **THEN** the summary line reads `Replayed N archived change(s) chronologically; S succeeded, F failed (R rolled back to archive).`

#### Scenario: Summary line omits rolled-back parenthetical when zero
- **WHEN** the rebuild processed N changes with R == 0 (typically because F == 0 too)
- **THEN** the summary line reads `Replayed N archived change(s) chronologically; S succeeded, F failed.` (no parenthetical)

#### Scenario: Rollback gap is visible when R < F
- **WHEN** the rebuild had 5 failed changes but only 4 rollbacks completed (1 rollback-of-rollback failure, or 1 data-loss-shaped failure that doesn't trigger rollback)
- **THEN** the summary line reads `..., 5 failed (4 rolled back to archive).`
- **AND** the failure_reason for the 5th entry contains either `rollback ALSO failed:` (rollback-of-rollback case) or `openspec archive reported success but the change is missing from both the active path and the archive` (data-loss case)
