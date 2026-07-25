# hold-covers-pr-creation-failure

## Why

The push-block hold protects completed work against push failure, but two gaps leave completed work unprotected — and each unprotected pass re-bills the full pipeline (pre-executor gates, executor, reviewer) on the next iteration:

1. **PR-creation failure.** When the branch push succeeds but the PR-creation API call then fails (GitHub 5xx, secondary rate limit), nothing is persisted. The next pass sees no open PR, recreates the agent branch from base — locally discarding the completed work — re-implements every carried change, re-reviews, and force-pushes over the remote commits. This repeats every iteration until the PR call succeeds. The throttled `PrCreationFailure` alert fires, but the work is not held.
2. **Issue-bearing passes.** The hold is written only when the pass carried changes. A pass that carried only issue units gets no hold on push failure, even though an issue pass runs a full executor session; the next pass wipes the commit and the promoted-issue reconciliation re-materializes the unit for a complete re-run.

## What Changes

- The push-block hold covers PR-creation failure after a successful push: the marker records which step failed; a matching hold makes the next pass skip branch recreation and the executor entirely and retry only the failed step (for a PR-creation hold, the push retry is a cheap no-op on the already-pushed tip, then PR creation is retried).
- The hold covers passes that carried issue units, recording their slugs alongside change slugs.
- The iteration-level error-tolerance contract states the same preservation rule for the PR-creation category that it already states for the branch-push category.

## Impact

- Affected specs: `orchestrator-cli` (Branch-push failure preserves completed work via a push-block hold; Iteration-level error tolerance), `git-workflow-manager` (Per-pass agent branch)
- Affected code: `autocoder/src/push_block.rs` (marker fields), `autocoder/src/polling_loop/pass.rs` (write the hold on PR-creation failure; include issue-bearing passes)
