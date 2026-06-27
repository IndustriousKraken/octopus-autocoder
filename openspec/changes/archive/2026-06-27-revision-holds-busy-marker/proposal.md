# The spec-revision executor holds the per-repo busy marker so status reflects it

## Why

When an operator runs `@<bot> send it`, the daemon drains a spec-revision
executor request (`process_pending_revision_execute`, `loop_drive.rs:519`) and
runs a bounded converge loop: edit the change's spec deltas → re-run the `[in]` /
`[canon]` gates → on a remaining contradiction, refresh `.needs-spec-revision.json`
and try again, up to `revision_converge_attempts + 1` passes (each edit bounded by
`executor.agentic_session_timeout_secs`, which operators often set high for large
changes).

That whole operation runs WITHOUT stamping the per-repo busy marker — the marker
is acquired only by the normal pass (`pass.rs:29`) and rebuild. But the `status`
verb computes its `currently:` line from exactly that marker (per
`Status reply always shows live workspace snapshot`). So a `status` issued while a
revision is actively editing and re-gating reports `currently: idle` — a false
negative. This was observed in production: a revision was running (high server
timeout), yet status said `idle`, even though the revision was actively rewriting
`.needs-spec-revision.json` between converge passes.

Marker freshness cannot stand in for this: a `.needs-spec-revision.json` "marked
N minutes ago" is ambiguous — it is rewritten mid-convergence on every re-gate
that still finds a contradiction, so a fresh marker could mean "revising right now"
OR "the attempt finished and re-held the change." The daemon needs an explicit
in-flight signal, and the per-repo busy marker is exactly that signal — it already
drives the `currently:` line and carries stale-detection/recovery semantics.

## What Changes

- The spec-revision executor SHALL acquire/stamp the per-repo busy marker for the
  DURATION of a `send it` revision, recording the change slug being revised, and
  release it when the revision completes (clean re-gate → PR, budget exhausted,
  scope/edit violation, gate could-not-run, or error).
- Because the live `status` reply already renders a stamped marker whose `change`
  is non-empty as `working on <change> (started <age> ago)`, an in-flight revision
  is then reflected in `currently:` instead of `idle` — with NO change to the
  status requirement's branching.
- The revision additionally participates in the same per-repo concurrency control
  as a normal pass (per `Per-repo busy marker prevents concurrent work`): a pass
  and a revision do not run concurrently on one workspace.

## Impact

- Affected specs: `orchestrator-cli` — one ADDED requirement. It does not modify
  the `Status reply always shows live workspace snapshot` requirement (the existing
  change-non-empty branch already produces a correct line once the marker is
  stamped) nor the `Per-repo busy marker prevents concurrent work` requirement (it
  extends that contract to the revision path).
- Affected code: `autocoder/src/polling/revision_session.rs`
  (`process_pending_revision_execute` / `run_revision_execute`) to acquire and
  release the busy marker around the converge loop, and `loop_drive.rs:519`'s call
  site as needed.
- Follow-on (NOT in this change): a distinct `currently: revising <change>` label
  would refine the status branching (rule order in
  `Status reply always shows live workspace snapshot`) and is deliberately deferred
  — `working on <change>` already removes the false-idle, which is the bug.
- Operational note: with a high `executor.agentic_session_timeout_secs`, set
  `executor.busy_marker_stale_threshold_secs` comparably high, or a long but
  healthy revision (live pid past threshold) will render in `currently:` as a stale
  marker — existing busy-marker behavior, not introduced here.
