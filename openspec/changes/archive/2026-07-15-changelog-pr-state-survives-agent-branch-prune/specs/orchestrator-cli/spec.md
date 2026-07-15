## MODIFIED Requirements

### Requirement: Per-PR state file persists revision count and last-seen timestamp; closed PRs are pruned
Each open PR being tracked SHALL have a state file at `<state_dir>/revisions/<repo-sanitized>/<pr_number>.json` (per the standard-locations requirement) containing `pr_number`, `agent_branch`, `last_seen_comment_at`, `revisions_applied`, `revision_cap`, and `cap_decline_posted`. The directory is shared by BOTH revision-tracking flows: agent-branch PRs (the primary revision loop) AND `changelog-<short-hash>` PRs (the changelog revision loop).

Pruning SHALL be flow-scoped. At iteration start, before any comment fetching, each flow SHALL prune only state files belonging to its own flow — identified by the state file's recorded `agent_branch` field — whose PR number is no longer in that flow's own set of open PRs: the primary walk prunes state whose recorded branch matches the repository's `agent_branch` against the open PRs returned by `list_open_prs_for_head`; the changelog walk prunes state whose recorded branch carries the `changelog-` prefix against its own open changelog-PR set. A flow SHALL NEVER delete the other flow's state file while that file's PR is open — deleting an open changelog PR's state resets its `last_seen_comment_at` to PR creation and causes each past `revise` comment to be re-detected and re-dispatched on every subsequent polling iteration (an unbounded revision loop). A state file whose recorded branch matches neither flow is left in place and logged at WARN rather than deleted.

#### Scenario: Closed PRs have their state pruned
- **WHEN** a polling iteration runs AND a previously-tracked PR is no longer in its own flow's open-PRs response
- **THEN** the state file at `<state_dir>/revisions/<repo-sanitized>/<pr_number>.json` is removed
- **AND** no future revision processing references that PR

#### Scenario: Open changelog PR state survives the agent-branch walk's prune
- **WHEN** a changelog PR (head `changelog-<short-hash>`) is open with a state file recording one applied revision
- **AND** the primary revision walk runs its prune (whose open set contains only agent-branch PRs)
- **THEN** the changelog PR's state file is NOT deleted
- **AND** its `last_seen_comment_at` and applied-revision count are unchanged

#### Scenario: One revise comment on a changelog PR dispatches exactly once
- **WHEN** an operator posts a single `@<bot> revise <text>` comment on an open changelog PR
- **AND** several polling iterations pass with no further comments
- **THEN** the stylist re-run is dispatched exactly once (on the first iteration that sees the comment)
- **AND** subsequent iterations fetch comments only after the persisted `last_seen_comment_at` and find nothing new
- **AND** a second revise comment produces a reply whose applied-revision count is `2`

#### Scenario: Closed changelog PR state is pruned by the changelog walk
- **WHEN** a changelog PR is closed or merged
- **AND** the changelog revision walk next runs
- **THEN** that PR's state file is removed by the changelog walk's own prune

#### Scenario: Unrecognized-branch state is preserved and logged
- **WHEN** the revisions directory contains a state file whose recorded branch matches neither the repository's agent branch nor the `changelog-` prefix
- **THEN** neither walk deletes it
- **AND** a WARN names the file and its unrecognized branch

#### Scenario: New PR initializes state lazily
- **WHEN** a polling iteration sees an open PR that has no existing state file AND the PR has new comments
- **THEN** a fresh `RevisionState` is initialized with `last_seen_comment_at = pr.created_at`, `revisions_applied = 0`, `cap_decline_posted = false`, and the resolved `revision_cap`
- **AND** the state is written to disk after any comment processing

#### Scenario: State writes are atomic
- **WHEN** the daemon writes a `RevisionState` file
- **THEN** the write uses temp-file-then-rename (matching the daemon's other state-file writes)
- **AND** an interrupted write does NOT leave a partial canonical file on disk
