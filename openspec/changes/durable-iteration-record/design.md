# Design: durable-iteration-record

## Record shape and location

`<state_dir>/iteration-record/<workspace-basename>.json` — ONE file per workspace, atomically overwritten (tempfile-then-rename) at the end of every iteration. Contents: `finished_at` (UTC), `outcome` (kind + one-line summary), `duration_secs`. No history: the status block only ever shows the latest iteration, and the unified daemon log already carries the full history. State-dir, never workspace: bookkeeping must not appear in the managed repo (established rule).

## Where the write happens

In the polling-task iteration driver, at the single point where an iteration's `Ok`/`Err` is already being handled (where "polling iteration failed for <url>" is logged) — so every terminal path writes exactly once: success-with-work, idle empty queue, skipped (open-PR park / queue blocked on waiting / push-block resume), audit-only, failed. Write failure is WARN-and-continue; the record is observability, never control flow, and it must not convert a healthy iteration into a failed one.

## Why "idle also writes" matters

Because idle iterations stamp the record too, `finished:` age has a clean invariant: on a live daemon it is always younger than the poll interval (plus in-flight time). An old age therefore means the polling task genuinely has not completed an iteration since then — dead task, skipped repo, stopped daemon. Today that same line means nothing at all.

## Orphaned failure-entry pruning

At pass start (after branch sync, so the freshly-pulled base state decides existence): for each `<state_dir>/failure-state/<workspace-basename>/<change>.json`, if `openspec/changes/<change>/` does not exist in the workspace, remove the file and log INFO. Marker-excluded changes (perma-stuck, needs-revision) still have their directories, so their counters survive; only changes that are truly gone (archived via a merge the server did not perform itself, or deleted) are pruned. The counter's sole consumer is perma-stuck detection, which is meaningless for a change that no longer exists.

## What the status handler loses

The `build_repo_status` failure-state shim (handlers.rs:284-308) is deleted, not conditionalized. `last_iteration` comes from the record or is `None` → the block renders `no iteration yet` (matching the menu's existing placeholder wording). Failure detail still reaches the operator through the record itself when the LAST iteration actually failed — which is the only time a failure belongs under "last iteration".
