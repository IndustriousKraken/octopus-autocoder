## Why

`walk_queue` currently continues to the next pending change after a `Failed` or `Escalated` outcome:

```rust
Ok(QueueStep::Failed { reason }) => { handle_failure_counter(...); /* fall through */ }
Ok(QueueStep::Escalated) => {} // continue to next
```

This contradicts the project's serial-queue invariant. The `time-based-change-ordering` change established that pending changes are processed in `proposal.md` mtime order — i.e. authoring order — because **change N+1 may depend on change N**. The "queue blocked when any change is waiting" rule (existing) enforces this at iteration start. But once `walk_queue` is mid-walk, a Failed/Escalated outcome causes the walker to skip ahead, breaking the same invariant.

Concrete failure modes observed in production:
- Change N is "reorganize repository traits". Implementer fails.
- Change N+1 is "narrow saved-card JSON surface" — depends on the reorganized layout.
- Walker proceeds to N+1, which now either fails (no diff) OR ships a wrong-shape commit that contaminates N's eventual retry.
- N's perma-stuck counter advances slowly because each retry pass only gets one fair attempt; N+1's failure counter starts accumulating poisoned failures it can't recover from until N is fixed.

The fix is the same logic already used for waiting changes: any non-Archive outcome in the same iteration halts the walk. The pass ends, push+PR ships whatever was archived (could be 0 commits → no PR), the next iteration re-evaluates from the top. The perma-stuck mechanism (default threshold `2`) bounds repeat failures: a persistently-failing change drops out of `list_pending` after two failed iterations, freeing the queue.

## What Changes

- **MODIFIED capability:** `orchestrator-cli`'s "Daemon entry point" requirement. The "serial-queue invariant" wording is extended: in-flight `Failed` and `Escalated` outcomes halt the current iteration's `walk_queue` immediately, mirroring the existing waiting-change rule at iteration start.
- **Code:** Two `break;` statements added to `walk_queue` in `autocoder/src/polling_loop.rs`:
  - After `handle_failure_counter` in the `Failed` arm.
  - In the (currently empty) `Escalated` arm.
- **Test updates:** existing tests that depend on "Failed/Escalated → next change is attempted" require updates. Most existing tests use one change at a time and won't be affected; a couple of multi-change tests do exercise the now-removed path and need rewriting to verify the new halt semantics.

## Impact

- Affected specs: `orchestrator-cli` (one MODIFIED requirement).
- Affected code: `autocoder/src/polling_loop.rs::walk_queue` (two `break;` statements). No signatures change.
- Behavior change visible to operators: a Failed or Escalated change halts the iteration's queue walk. Other pending changes wait until the next iteration. PR count per pass goes down for repos with frequent failures; perma-stuck timing is unaffected (still bounded by `perma_stuck_after_failures`).
- Behavior change NOT made: cross-repo blocking is still not implied. Other repositories' polling tasks continue normally.
- Breaking: yes for operators who relied on the lenient "continue on Failed" behavior. No such reliance is documented; the original behavior was a defect inconsistent with the time-ordering and waiting-block rules.
