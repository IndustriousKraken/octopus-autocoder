# Push failure preserves completed work instead of trashing it

## Why

A change can be fully implemented — the executor's diff committed and the change
archived on the agent branch — and then have its **branch push** rejected by the
remote (e.g. branch protection, a stale lease, or any transient/permanent git
push error). Today that completed work is thrown away and re-done from scratch
every polling pass, forever:

1. The per-change queue step returns `Archived` (success), which **resets** the
   change's perma-stuck failure counter (`failure_state::clear`).
2. The push happens AFTER, at the pass level. On failure it emits a throttled
   `BranchPushFailure` alert and returns `Err`.
3. Canon (`orchestrator-cli` "Perma-stuck counter covers all per-change errors")
   **deliberately** declares branch-push failures unattributable, so they
   increment no counter — the change can never go perma-stuck.
4. Next pass, `git checkout -B <agent_branch>` (`git-workflow-manager` "Per-pass
   agent branch", "overwritten without warning — by design") **discards the
   commits**, the change is pending again, and it is re-implemented.

The result is an unbounded loop that re-runs a completed implementation every
pass — each run is real, billable agent token spend (an implementation can cost
tens of dollars), discarded the instant the next pass starts. Re-running
implementation can **never** fix a push failure, because the work was already
done correctly; only the transport failed. Deleting finished work to re-do it is
the defect.

## What Changes

A branch-push failure becomes a **recoverable, work-preserving hold**, never a
trigger to re-implement or to reset the branch. Because the changes are already
archived + committed on the agent branch by the time the push runs, the hold is
tracked by a **per-workspace push-block marker in the state directory** — NOT a
per-change marker (the change directories no longer exist at the active path):

- **Preservation.** The agent branch and its commits are RETAINED on a push
  failure — never reset. The next pass SHALL NOT recreate the agent branch while a
  push-block marker is present AND the branch tip still matches the marker (the
  marker, written only on a real push failure, is the anchor — so a stale
  post-merge branch is never falsely preserved); it resumes at the push/PR step
  rather than re-running the executor.
- **Push-block marker + alert.** On a push failure carrying committed changes,
  autocoder writes a push-block marker (workspace-keyed, in the state dir)
  recording the unpushed tip commit, the change slug(s), and the rejection reason;
  and posts the existing throttled `BranchPushFailure` alert naming the reason, the
  remedy, AND that the work is preserved on `<agent_branch>`.
- **Cheap push-only retry.** Each subsequent pass retries the push step ONLY —
  never the executor. A failed `git push` costs no tokens, so a persistent failure
  just re-attempts the cheap push and re-posts the throttled alert until it
  succeeds. No retryable/non-retryable classification, no hold state machine.
- **Recovery.** The operator fixes the cause (grants the token scope, lifts branch
  protection) and the next pass's retry succeeds automatically — no operator
  command needed. To abandon the work instead, the operator deletes the marker file
  directly (the next pass recreates the branch).
- **No destructive reset on push failure, ever.** A push failure SHALL never
  cause a branch or workspace wipe. The only destructive reset remains an explicit
  operator action (`rewind` / `wipe-repo`).
- **Not the perma-stuck counter.** A push failure is a transport failure, not an
  execution failure, so it uses this dedicated push-block hold and does NOT touch
  the perma-stuck counter (whose "branch push is unattributable" rule is left
  intact).

## Impact

- Ends the implement→trash→re-implement doom loop and the associated token burn.
- Modifies `orchestrator-cli`: "Iteration-level error tolerance"; ADDS
  "Branch-push failure preserves completed work via a push-block hold".
- Modifies `git-workflow-manager`: "Per-pass agent branch".
- The perma-stuck counter requirement is unchanged.
- No change to the happy path (a successful push still archives + opens the PR and
  clears alert state exactly as today).
