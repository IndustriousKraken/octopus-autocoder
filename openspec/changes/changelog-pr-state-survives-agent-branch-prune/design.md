## Context

`process_revision_requests_at` (`autocoder/src/revisions.rs:578`) builds its open set from `find_pr_by_head(..., repo.agent_branch)` (`revisions.rs:614-619`) and hands it to `prune_closed_prs` (`revisions.rs:628`), which deletes every `<pr>.json` not in the set (`revisions.rs:267-303`). Changelog PRs (head `changelog-<short-hash>`) store their revision state in the same directory precisely to reuse "prune-on-close" (`changelog_triage.rs:841-842`) — but they are never in the agent-branch open set, so their state is deleted every iteration while open. The changelog revision handler then re-initializes state with `last_seen_comment_at = pr.created_at` (`changelog_triage.rs:843-860`) and re-processes every revise comment since PR creation. Production evidence: coterie PR #100 — one revise comment, thirteen stylist re-runs at polling cadence over two hours, every reply reading "Total revisions on this PR: 1", terminated only by an incidental out-of-scope-diff failure.

## Goals / Non-Goals

**Goals:**
- An open PR's revision state is deleted only by the flow that owns it, and only when that PR is actually closed.
- One revise comment → one dispatch; the applied-revision counter is truthful.

**Non-Goals:**
- A revision cap for changelog PRs (the loop was not a cap problem — the counter never reached 2; with truthful state the existing `u32::MAX` cap stands until someone wants otherwise).
- Unifying the two walks into one — they poll different PR sets with different dispatch logic; only the prune scoping is shared ground.
- Migrating state-file locations (the path reference in canon is corrected to match where the code already writes; no files move).

## Decisions

- **Scope by the state file's recorded `agent_branch`, not by extra bookkeeping.** Every `RevisionState` already records its PR's head branch (`agent_branch` field, stamped at initialization from `pr.head.ref_`). `prune_closed_prs` gains a branch predicate: the primary walk passes "matches `repo.agent_branch`", the changelog walk passes "starts with `changelog-`". No new state, no migration; files written by old builds already carry the field.
- **Files matching neither predicate are preserved and WARN-logged.** Fail-safe: an unrecognized branch means a newer flow or a corrupt field; deleting on ignorance is how this bug happened. The WARN gives operators a janitorial signal.
- **The changelog walk prunes its own flow.** It already enumerates open changelog PRs to process their comments; passing that set through the same branch-scoped prune preserves prune-on-close for changelog state (the original reason the directory is shared).
- **Read-back of `agent_branch` requires parsing candidate files before deletion.** The prune currently deletes on filename alone. It now reads each candidate's JSON to get the branch; unparseable files are treated like unrecognized branches (preserve + WARN) rather than deleted, consistent with fail-safe pruning.

## Risks / Trade-offs

- [Truly orphaned unparseable files now accumulate instead of being deleted] → Bounded by the WARN making them visible; a corrupt state file is rare and small. Deleting unparseable state is exactly the failure mode being fixed, so preservation is the correct default.
- [Prune now reads N small JSON files per iteration instead of listing names] → N is the number of open-ish PRs per repo; negligible IO.
