# Open spec-revision PR parks the change in the queue

## Why

When the spec-revision executor opens a revision PR (on branch
`agent-q-spec-revision-<change>`), it now clears the
`.needs-spec-revision.json` marker (per `revision-clears-needs-spec-revision-marker`).
That is correct behavior — the open PR is itself the blocking signal, so the
marker is redundant once the PR exists.

However, the queue currently has no mechanism to detect that an open
spec-revision PR for a change should park that change. Only the repo-level
`agent-q` branch is checked for open PRs (per `Skip iteration when an open PR
exists for the agent branch`). The spec-revision branch
`agent-q-spec-revision-<change>` is not checked. The consequence:

1. Spec-revision executor opens revision PR, clears marker.
2. Next iteration: queue finds the change unblocked (no marker), runs the gate.
3. Gate re-runs on the unchanged spec, finds the same contradiction, writes the
   marker back.
4. The revision PR and the marker now coexist — the change is stuck in a loop
   where every iteration re-adds the marker the executor just cleared.

This is a spec gap. The canon defined two blocking conditions (`.needs-spec-revision.json`
marker, and a commit the gate confirms needs revision) but omitted the third:
"an open spec-revision PR means the revision is already in flight — do not
re-gate until the PR is resolved."

## What Changes

The blocking-markers gate (run before invoking the executor on any change in
the queue) SHALL also check whether an open PR exists on the branch
`<agent_branch>-spec-revision-<change-slug>`. When such a PR exists, the
change is parked — the gate returns the same "blocked, halt queue walk" result
as a `.needs-spec-revision.json` marker — without re-running the spec gate,
without re-writing the marker.

- **On merge of the spec-revision PR**: the branch is gone, the PR is closed,
  the gate runs normally on the next iteration with the updated spec.
- **On close without merge**: same — branch gone, PR closed, gate re-runs and
  re-adds the marker if the spec still needs revision.
- **On the iteration where the revision PR is opened**: the executor that opened
  the PR runs within the current iteration, which returns before the next
  queue walk. The park takes effect on the NEXT iteration; the within-iteration
  skip is not needed.

The PR query for spec-revision branches reuses the existing `find_pr_by_head`
forge call. Because the spec-revision branch lives on the same fork (or origin)
as `agent-q`, the `head_owner` resolution is identical. The query is per-change
(one call per change currently in the blocking-marker check path), not one broad
scan.

The `status` verb's `currently:` line SHOULD surface the parked-by-spec-revision-pr
state when it is active, alongside the existing marker-present and open-agent-pr
states.

## Impact

- Affected specs: `orchestrator-cli` — the `Spec-needs-revision executor outcome +
  marker` requirement and the `Skip iteration when an open PR exists for the agent
  branch` requirement each gain a corollary covering the spec-revision branch.
- Affected code: `autocoder/src/polling_loop/commits.rs`
  (`handle_blocking_markers_gate` or its caller) — add the open-spec-revision-PR
  check alongside the marker check.
- Forge API budget: one extra `find_pr_by_head` call per change that reaches the
  blocking-marker gate and has no `.needs-spec-revision.json` marker. The call
  is skipped when the marker is already present (marker check short-circuits first).
