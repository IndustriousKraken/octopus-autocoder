## Why

Changelog PRs reuse the per-PR revision state directory so closed PRs get pruned — but the prune that runs in the primary (agent-branch) revision walk computes "open PRs" from `list_open_prs_for_head` on `repositories[].agent_branch` only. A changelog PR's head is `changelog-<short-hash>`, so its state file is deleted every polling iteration while the PR is open. The deleted state resets `last_seen_comment_at` to PR creation, so a single `@<bot> revise` comment is re-detected and re-dispatched every iteration, indefinitely — observed in production (coterie PR #100: thirteen stylist re-runs over two hours from one comment, each replying "Total revisions on this PR: 1", until a run happened to fail).

## What Changes

- Pruning becomes flow-scoped: each revision walk prunes only its own flow's state files, identified by the state file's recorded branch — the agent-branch walk prunes state whose branch matches the repo's agent branch; the changelog walk prunes state whose branch carries the `changelog-` prefix, against its own open-changelog-PR set.
- A flow never deletes the other flow's state for an open PR, so a changelog PR's `last_seen_comment_at` and applied-revision count persist across iterations: one revise comment dispatches exactly once, and the reply counter actually counts.
- The requirement's stale state-file path reference (`<workspace>/.autocoder/revisions/`) is aligned with the standard-locations requirement (`<state_dir>/revisions/<repo-sanitized>/`), which is where the code already writes.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `orchestrator-cli`: the "Per-PR state file persists revision count and last-seen timestamp; closed PRs are pruned" requirement becomes flow-scoped (each walk prunes only its own flow's state) and its path reference is corrected.

## Impact

- `autocoder/src/revisions.rs`: `prune_closed_prs` (or its call site in `process_revision_requests_at`) filters candidates by the state file's recorded `agent_branch` before deleting.
- `autocoder/src/changelog_triage.rs`: the changelog revision walk prunes `changelog-*`-branch state against its own open set (it already enumerates open changelog PRs).
- No config, API, or state-shape changes — `agent_branch` is already stored in every state file.
