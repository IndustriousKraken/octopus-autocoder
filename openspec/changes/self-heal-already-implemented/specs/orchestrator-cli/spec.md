## MODIFIED Requirements

### Requirement: Reject archive-only iterations as Failed
autocoder SHALL treat an iteration as Failed (not Completed), revert the staged moves via `git reset --hard`, and leave the change pending for retry when the executor returns Completed AND the resulting working-tree changes consist *only* of file moves whose destination paths start with `openspec/changes/archive/`. The detection is structural — pattern-matching on rename destinations — and does not depend on which command produced the moves. autocoder SHALL treat Completed-with-clean-workspace as Failed by default — UNLESS the change's implementation is already on the base branch, in which case autocoder SHALL self-archive the change rather than fail (see "Self-heal: already-implemented change" scenario).

#### Scenario: Agent archives the change instead of implementing it
- **WHEN** the executor returns `Completed` for a change AND
  `git status --porcelain` reports a non-empty result AND every
  reported entry is a rename (status code `R`) whose target path
  begins with `openspec/changes/archive/`
- **THEN** autocoder reverts the working tree via
  `git reset --hard HEAD` to discard the staged moves
- **AND** autocoder treats the outcome as
  `Failed { reason: "agent appears to have archived without implementing the change" }`
- **AND** autocoder logs a `warn`-level line naming the change
- **AND** the change's `.in-progress` lock is removed via the
  existing Failed-handling code path so the next iteration
  retries

#### Scenario: Legitimate implementation that also moves an archive file
- **WHEN** the executor returns `Completed` AND the working tree
  contains at least one change that is NOT a rename into
  `openspec/changes/archive/` (e.g. modified `src/foo.rs`, added
  `tests/bar.rs`)
- **THEN** autocoder treats the outcome as Completed as before
- **AND** the commit + push + PR steps proceed normally
- **AND** archive-rename entries, if any, are included in the
  commit unchanged

#### Scenario: Workspace is clean (no changes at all)
- **WHEN** the executor returns `Completed` AND `git status
  --porcelain` is empty AND the self-heal criteria below are NOT
  all satisfied
- **THEN** autocoder treats the outcome as
  `Failed { reason: "agent reported Completed without modifying the workspace" }`
- **AND** autocoder logs a `warn`-level line naming the change
- **AND** autocoder does NOT commit, does NOT archive, and does
  NOT push
- **AND** the change's `.in-progress` lock is removed via the
  existing Failed-handling code path so the next iteration
  retries
- **AND** the lazy-archive detection does NOT fire (no staged
  moves to revert)

#### Scenario: Self-heal — already-implemented change
- **WHEN** the executor returns `Completed` AND `git status
  --porcelain` is empty AND `openspec validate <change> --strict`
  exits 0 AND every line in
  `openspec/changes/<change>/tasks.md` that matches the regex
  `^\s*-\s*\[([ x])\]` has `[x]` (and at least one such line
  exists)
- **THEN** autocoder treats the outcome as a self-heal Archive:
  it runs the archive move (renaming
  `openspec/changes/<change>/` to
  `openspec/changes/archive/<YYYY-MM-DD>-<change>/`) on the
  agent branch, commits the move with subject
  `archive: <change>: implementation already in base`, and
  proceeds through the normal push + PR flow
- **AND** the PR body for a self-heal pass includes the
  paragraph `_This PR archives a change whose implementation was
  already present on the base branch. No code diff is included;
  only the openspec archive move._` ahead of any other body
  content
- **AND** autocoder logs an INFO line naming the change and the
  self-heal classification, distinct from the Failed-path log

#### Scenario: Self-heal preconditions unmet
- **WHEN** the executor returns `Completed` AND `git status
  --porcelain` is empty AND any of the self-heal preconditions
  fails: `openspec validate --strict` errors or exits non-zero,
  OR any task in `tasks.md` is still `[ ]`, OR `tasks.md` cannot
  be read
- **THEN** autocoder falls through to the Failed path (as in
  "Workspace is clean (no changes at all)" above), preserving
  the prior behavior for non-self-heal cases
