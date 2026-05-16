## MODIFIED Requirements

### Requirement: Enumerate ready changes
The queue engine SHALL list pending OpenSpec changes in the workspace, excluding archived, locked, waiting, perma-stuck, dotfile, and non-directory entries. The returned list SHALL be sorted ascending by entry name (UTF-8 byte order, which is also alphabetical for ASCII names).

#### Scenario: Listing the queue
- **WHEN** the queue engine is queried for pending changes in a workspace
- **THEN** it returns the names of every direct subdirectory of `<workspace>/openspec/changes/` that satisfies ALL of the following:
  - the entry is a directory (not a file or symlink)
  - the entry name is not the literal string `archive`
  - the entry name does not begin with `.`
  - the entry does NOT contain a file named `.in-progress`
  - the entry does NOT contain a file named `.question.json`
  - the entry does NOT contain a file named `.perma-stuck.json`
  - the entry contains at least a regular file named `proposal.md`
- **AND** the returned list is sorted ascending by entry name

#### Scenario: Alphabetical order is deterministic across git operations
- **WHEN** the workspace state is altered by any git operation
  (clone, fetch, pull, checkout, reset, merge) that changes
  proposal.md mtimes
- **THEN** `list_pending` returns the same order as before the
  operation (entry names are stable across git operations)
- **AND** operators who require explicit sequencing prepend a
  numeric or alphabetical prefix to change names (e.g.
  `01-rename-foo`, `02-extract-bar`) to control order
