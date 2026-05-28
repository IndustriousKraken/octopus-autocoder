## MODIFIED Requirements

### Requirement: Perma-stuck change detection
autocoder SHALL track consecutive failures per change in a per-repo `.failure-state.json` file at the workspace root. After the executor returns `Failed` for a change (or the daemon transforms a Completed-with-empty-workspace outcome to Failed), the counter for that change SHALL be incremented. After the executor returns `Archived` (including via self-heal), the counter for that change SHALL be cleared. When a change's counter reaches the configured `executor.perma_stuck_after_failures` threshold (default 2), autocoder SHALL write a `.perma-stuck.json` marker into the change directory, post a chatops alert, AND exclude the change from subsequent polling iterations until the marker is removed manually.

A `.perma-stuck.json` marker SHALL ALSO block the queue walk for subsequent pending changes in the same repository, per the same-repo blocking policy that already applies to `.in-progress*` AND `.needs-spec-revision.json` markers. The block is downgradeable per the `Ignore-for-queue marker downgrades blocking-marker behavior` requirement: an operator who stamps `.ignore-for-queue.json` alongside the perma-stuck marker tells the daemon "I know this one's broken; skip it AND proceed with the rest." The change stays excluded from `list_pending` (perma-stuck markers always exclude); the ignore-marker only releases the sibling-blocking effect.

#### Scenario: Failure increments the counter
- **WHEN** `handle_outcome` produces a `Failed` result for a change (whether the executor returned Failed or the daemon transformed a Completed-with-empty-workspace via the no-op-completion or self-heal logic into Failed)
- **THEN** autocoder reads `.failure-state.json` from the workspace root, increments the entry for that change (or creates it with `count: 1` if absent), sets `last_reason` and `last_failed_at`, and writes the file back atomically (write-temp-then-rename)
- **AND** transient daemon-side errors that prevent the executor from running (workspace init failure, openspec preflight failure, GitHub API transport error) do NOT increment the counter — only outcomes where the executor itself ran and Failed (or was forced to Failed by post-execution classification) count

#### Scenario: Archive clears the counter
- **WHEN** `handle_outcome` produces an `Archived` result for a change (including via the self-heal path from `self-heal-already-implemented`)
- **THEN** autocoder removes that change's entry from `.failure-state.json` and writes the file back atomically
- **AND** the next failure of any change starts fresh from `count: 1`

#### Scenario: Threshold reached → mark perma-stuck
- **WHEN** incrementing the counter results in `count >= executor.perma_stuck_after_failures` (default 2)
- **THEN** autocoder writes a `.perma-stuck.json` marker file inside the change directory containing the change name, consecutive_failures count, last_reason, marked_stuck_at timestamp, and the operator_action message
- **AND** autocoder posts a chatops alert via the configured backend with subject "change perma-stuck" and a body naming the repo, change, count, and last reason. The alert is subject to the existing 24h throttle so repeat-mark events do not spam
- **AND** autocoder logs an ERROR line naming the change and the marker file path
- **AND** when no chatops backend is configured, the ERROR log is the operator's only signal — the marker is still written and the change is still excluded from `list_pending` going forward

#### Scenario: Operator clears the marker
- **WHEN** the operator deletes `.perma-stuck.json` from a change directory (manually or via `@<bot> clear-perma-stuck`)
- **THEN** the next polling iteration sees the change in `list_pending` again and runs the executor against it
- **AND** the counter starts fresh at 0 (or whatever `.failure-state.json` records for that change after the removal — implementations MAY also clear the change's entry in `.failure-state.json` at marker-removal time; either is acceptable as long as the operator's "retry" signal does reset behavior)
- **AND** if a `.ignore-for-queue.json` marker accompanied the perma-stuck marker, `clear-perma-stuck` removes BOTH files (full resolution)

#### Scenario: Threshold is one
- **WHEN** `executor.perma_stuck_after_failures` is set to `1`
- **THEN** the very first Failed outcome for a change marks perma-stuck (no retry at all)

#### Scenario: Default threshold
- **WHEN** `executor.perma_stuck_after_failures` is unset
- **THEN** autocoder uses `2` as the threshold value

#### Scenario: Perma-stuck marker blocks subsequent pending changes by default
- **WHEN** a repository has a change with `.perma-stuck.json` AND a subsequent change in `list_pending` (no markers)
- **AND** the perma-stuck change does NOT also have `.ignore-for-queue.json`
- **THEN** the polling iteration's queue walk halts before processing the subsequent change
- **AND** an INFO log line names the blocking change AND the marker file path
- **AND** the operator's options are: (a) fix the perma-stuck change AND run `@<bot> clear-perma-stuck`, OR (b) run `@<bot> ignore-and-continue` to skip the broken change AND let siblings proceed

## ADDED Requirements

### Requirement: Ignore-for-queue marker downgrades blocking-marker behavior without unblocking the change itself
autocoder SHALL recognize a per-change `.ignore-for-queue.json` marker file at `<workspace>/openspec/changes/<change>/.ignore-for-queue.json`. The marker downgrades any sibling operator-action marker (`.perma-stuck.json`, `.needs-spec-revision.json`) on the same change from "blocks subsequent queue processing" to "still excludes this change from `list_pending`, but doesn't block siblings." The marker is the operator's explicit "I know this change is broken; skip it AND proceed with the rest" signal.

The marker SHALL be writable via the `@<bot> ignore-and-continue` chatops verb (writes the file AND commits/pushes the change directory's update) AND removable via the `@<bot> clear-ignore` verb (removes the file AND commits/pushes the removal). The file is intentionally git-tracked, consistent with `.perma-stuck.json` AND `.needs-spec-revision.json`.

Removal of the underlying blocking marker (e.g. via `@<bot> clear-perma-stuck`) SHALL also remove the `.ignore-for-queue.json` marker — when the underlying marker is gone, the ignore-marker has nothing to downgrade AND becomes vestigial.

#### Scenario: Operator stamps ignore-for-queue; queue resumes for siblings
- **WHEN** a repository has change A with `.perma-stuck.json` AND change B pending (no markers) AND change A also has `.ignore-for-queue.json`
- **THEN** the polling iteration's queue walk processes change B (the ignore-marker downgrades A's blocking effect)
- **AND** change A remains excluded from `list_pending` (perma-stuck marker still applies to A's own status)
- **AND** the iteration's chatops `🚀 starting work on B` fires normally

#### Scenario: `@<bot> ignore-and-continue` writes the marker
- **WHEN** the operator runs `@<bot> ignore-and-continue <repo-substring> <change-slug>`
- **AND** the named change has at least one of `{.perma-stuck.json, .needs-spec-revision.json}`
- **THEN** the daemon writes `.ignore-for-queue.json` inside the change directory containing the change name, marked_at timestamp, marked_by operator identifier, AND the operator_action note
- **AND** the daemon commits the file AND pushes the commit to the agent branch (subject `chore: ignore-for-queue on <change> (operator <id>)`)
- **AND** the chatops reply confirms: `✓ Marked <change> as ignored for queue. Subsequent changes will process; <change> stays excluded until the underlying marker is cleared.`

#### Scenario: `@<bot> ignore-and-continue` rejects when no underlying marker exists
- **WHEN** the operator runs `@<bot> ignore-and-continue <repo> <change>` AND the named change has NEITHER `.perma-stuck.json` NOR `.needs-spec-revision.json`
- **THEN** the daemon refuses: `✗ <change> has no operator-action marker (perma-stuck OR needs-spec-revision). Ignore is a no-op; rejecting to prevent confusion.`
- **AND** no file is written

#### Scenario: `@<bot> clear-ignore` removes the marker, queue resumes blocking
- **WHEN** the operator runs `@<bot> clear-ignore <repo-substring> <change-slug>`
- **AND** the named change has `.ignore-for-queue.json`
- **THEN** the daemon removes the file AND commits/pushes the removal (`chore: clear ignore-for-queue on <change>`)
- **AND** subsequent polling iterations resume blocking the queue on the original marker (if still present)
- **AND** the chatops reply confirms: `✓ Cleared ignore-for-queue on <change>. Queue resumes blocking on <original-marker>.`

#### Scenario: `clear-perma-stuck` removes ignore-for-queue too
- **WHEN** the operator runs `@<bot> clear-perma-stuck <repo> <change>` AND the change has BOTH `.perma-stuck.json` AND `.ignore-for-queue.json`
- **THEN** BOTH files are removed by the same operation
- **AND** the chatops reply notes both removals: `✓ Cleared .perma-stuck.json AND .ignore-for-queue.json for <change>.`
- **AND** the change re-enters `list_pending` on the next iteration (per the existing clear-perma-stuck behavior)
