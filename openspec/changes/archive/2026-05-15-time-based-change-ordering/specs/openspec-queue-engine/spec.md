## MODIFIED Requirements

### Requirement: Enumerate ready changes
The queue engine SHALL list pending OpenSpec changes in the workspace, excluding archived, locked, waiting, perma-stuck, dotfile, and non-directory entries. The returned list SHALL be sorted by ascending `proposal.md` modification time; ties SHALL be broken by ascending entry name for determinism.

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
- **AND** the returned list is sorted ascending by `proposal.md`
  modification time; entries whose `proposal.md` shares the same
  mtime are ordered ascending by entry name as a secondary sort
  key

#### Scenario: Older proposal sorts before newer
- **WHEN** workspace contains two pending changes `older-change`
  and `newer-change` whose `proposal.md` files have distinct
  mtimes (older < newer)
- **THEN** `list_pending` returns `["older-change", "newer-change"]`
  regardless of alphabetical order between the two names

#### Scenario: Tied mtimes fall back to name order
- **WHEN** workspace contains two pending changes `b-change` and
  `a-change` whose `proposal.md` files share the same mtime
- **THEN** `list_pending` returns `["a-change", "b-change"]` —
  alphabetical order breaks the mtime tie deterministically
