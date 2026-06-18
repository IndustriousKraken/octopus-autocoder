# Issues lane parks a non-progressing issue instead of retrying it forever

## Why

The changes lane gives up on a unit it cannot land: after
`executor.perma_stuck_after_failures` consecutive failures it writes a
`.perma-stuck.json` marker, excludes the change from the queue, and alerts
the operator. The issues lane has no such gate. Its walker records a
per-issue failure counter (`lanes::state::record_failure`), but the function
that reads it (`lanes::state::failure_count`) is dead — nothing consults it,
and `issues::list_ready` re-selects every well-formed, unlocked,
non-archived issue every pass. An issue that cannot complete is therefore
re-attempted indefinitely: a full executor session every iteration, forever.

Two outcomes make this worse than a slow leak. A `Failed` issue and an
`Escalated` issue (the agent asked a question — the issues lane does not
escalate) post NOTHING to chatops, so the only operator-visible signal is the
`starting issue` notice repeating. And `Escalated` does not even record a
failure, so a counter-only gate would still loop on it. The result observed
in the field: one issue re-started every iteration for hours, silent except
for the start notice.

## What Changes

- A new `orchestrator-cli` requirement gives the issues lane the same
  give-up semantics the changes lane already has, owned by the issues
  walker and its own state (per the independent-lane-walkers requirement):
  - A retryable failure increments the per-issue consecutive-failure
    counter; on reaching `executor.perma_stuck_after_failures` the issue is
    PARKED — a `.perma-stuck.json` marker is written into `issues/<slug>/`
    and the unit is excluded from selection until an operator removes it.
  - An outcome that retrying cannot resolve — the agent escalating a
    question, OR kicking the fix back to the changes lane — parks the issue
    immediately (one attempt, not the full threshold), which also stops the
    kick-back notice from re-posting every pass.
  - A daemon-shutdown abort never counts toward the threshold.
  - Completion clears both the counter and the marker.
  - Parking posts an operator-visible chatops alert naming the issue, the
    attempt count, and the last reason — the lane is never silently stuck
    and never silently abandoned.
- `lanes::state::failure_count` is wired into the walker (its dead-code
  allowance removed); `issues::list_ready` skips a parked issue exactly as
  it already skips a locked one; the marker file reuses the
  `.perma-stuck.json` name already registered in `.git/info/exclude`, so it
  is gitignored at any depth and survives branch reset and `git clean`.

## Impact

- Affected specs: `orchestrator-cli` (ADD the issues-lane give-up
  requirement).
- Affected code: `lanes/issues.rs` (marker helpers + `list_ready` skip),
  `lanes/walker.rs` (park on threshold / on escalate / on kick-back, thread
  the threshold), `lanes/state.rs` (consume `failure_count`),
  `polling_loop/commits.rs` (thread `perma_stuck_threshold` into the issues
  lane).
- Reuses the existing `executor.perma_stuck_after_failures` threshold — no
  new config. The operator unparks an issue the same way as a change:
  delete the `.perma-stuck.json` marker.
