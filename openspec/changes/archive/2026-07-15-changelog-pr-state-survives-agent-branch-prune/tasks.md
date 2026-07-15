## 1. Flow-scoped prune

- [x] 1.1 In `autocoder/src/revisions.rs`, extend `prune_closed_prs` to take a branch predicate and parse each candidate state file's `agent_branch` before deleting: delete only when the branch matches the predicate AND the PR number is absent from the provided open set; preserve and WARN on files whose branch matches neither expectation or whose JSON does not parse.
- [x] 1.2 Update the primary walk's call site (`process_revision_requests_at`) to pass the "equals `repo.agent_branch`" predicate.
- [x] 1.3 In `autocoder/src/changelog_triage.rs`, have the changelog revision walk prune with the "starts with `changelog-`" predicate against its own open changelog-PR set.

## 2. Tests

- [x] 2.1 Unit test: an open changelog PR's state file survives the primary walk's prune while an actually-closed agent-branch PR's state in the same directory is removed.
- [x] 2.2 Unit test: the changelog walk's prune removes state for a closed changelog PR and leaves open changelog PRs and agent-branch state untouched.
- [x] 2.3 Unit test: a state file with an unrecognized branch (or unparseable JSON) is preserved by both walks and a WARN is emitted.
- [x] 2.4 Regression test for the loop shape: with persisted state across two simulated iterations, a single revise comment on a changelog PR dispatches once and the second iteration dispatches nothing; a second comment increments the applied count to 2.
- [x] 2.5 Run the full `cargo test` suite; confirm existing prune tests (`prune_removes_state_for_closed_prs`, garbage-name handling) still pass with the predicate added.
