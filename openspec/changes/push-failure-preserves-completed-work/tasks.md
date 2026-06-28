# Tasks

## 1. Push-block marker (per-workspace, state dir)

- [x] 1.1 Add a push-block marker type written to the daemon STATE directory keyed
  to the workspace (NOT a change directory). Fields: unpushed agent-branch tip
  commit, the change slug(s) the push carried, and the rejection reason. Atomic
  write (temp-then-rename), matching other state files.
- [x] 1.2 On a pass-level `git::push_force_with_lease` failure (`pass.rs`) where the
  pass committed one or more changes, write the marker and post the existing
  throttled `BranchPushFailure` alert naming the reason, the remedy, AND that the
  work is preserved on the agent branch.

## 2. Branch preservation + resume-at-push

- [x] 2.1 In branch initialization (`git-workflow-manager` "Per-pass agent branch"),
  when a push-block marker is present AND the agent branch tip matches the marker's
  recorded tip, SKIP `git checkout -B <agent_branch>` (do not reset) and signal the
  pass to resume at the push step — do NOT run the queue walk / executor. If the
  marker is present but the tip no longer matches, remove the stale marker and
  recreate normally.
- [x] 2.2 Confirm NO code path resets the agent branch or workspace as a consequence
  of a push failure.
- [x] 2.3 While the marker is present and matching, the pass retries the push step
  only (never the executor); a persistent failure refreshes the marker reason and
  re-posts the throttled alert.

## 3. Recovery + happy path

- [x] 3.1 On a SUCCESSFUL push of the preserved branch, remove the push-block marker
  and open the PR (body derived from the carried change slugs); clear the
  `BranchPushFailure` alert state.
- [x] 3.2 Recovery needs no operator command: the operator fixes the cause and the
  next retry succeeds; deleting the marker file abandons the work (next pass
  recreates the branch).

## 4. Tests

- [x] 4.1 A simulated push failure after a change is committed writes a
  workspace-keyed push-block marker (with the slug + tip) and preserves the agent
  branch commits (work not reset).
- [x] 4.2 With a push-block marker present AND matching, the next pass does NOT
  invoke the executor for the carried change; it retries the push step only. A stale
  marker (tip mismatch) is removed and the branch recreated.
- [x] 4.3 A successful push removes the marker and opens the PR.
- [x] 4.4 A push failure never triggers a branch/workspace reset.
- [x] 4.5 The happy-path push+PR flow and alert clearing are unchanged (existing
  pass tests still pass); the perma-stuck counter is untouched by push failures.
