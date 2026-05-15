## Why

`queue::list_pending` currently sorts pending changes ascending by directory name. Alphabetical order is arbitrary — `cache-has-admin-flag` happens to land before `consolidate-auth-middleware` because of letter ordering, with no relationship to whether either should be implemented first. When operators write a queue of dependent changes (e.g. ten refactors stacked so each builds on the previous), they typically author them in the order they want them applied. The `proposal.md` file's modification time captures that authoring order directly.

Switching the queue's sort key from name to `proposal.md` mtime aligns implementation order with authoring intent for free, with no operator action and no config field.

## What Changes

- **MODIFIED capability:** `openspec-queue-engine`'s "Enumerate ready changes" requirement. The returned list SHALL be sorted by ascending `proposal.md` modification time. Ties (or filesystem-quirks where mtime resolution is coarse) SHALL be broken by ascending entry name for determinism.
- **Code:** `queue::list_pending` reads each candidate's `proposal.md` metadata to obtain its modification time. Sorts the result by `(mtime, name)` ascending. If `proposal.md` is missing (the entry would already have been filtered out earlier by the proposal-required rule), the ordering is undefined — but since such entries are excluded, this case doesn't arise.
- **No config field.** Time is the new behavior across the board.

## Impact

- Affected specs: `openspec-queue-engine` (one MODIFIED scenario under "Enumerate ready changes").
- Affected code: `autocoder/src/queue.rs`.
- Behavior change visible to operators: changes are now picked up in authoring order rather than alphabetical. For most operators this is invisible (single-change queues, or changes whose authoring order happens to match name order). For operators with stacked dependent changes, it removes the need to prepend ordering characters.
- Determinism: ties broken by name keep behavior reproducible across passes when two files share an mtime (rare; filesystem mtime is typically nanosecond-resolution on Linux).
- Breaking: yes if an operator's deployment has somehow come to depend on alphabetical order. No such deployments are known; the project has one active operator.
