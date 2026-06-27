# Tasks

## 1. Track consecutive failed revision rounds

- [x] 1.1 Add a per-change counter of CONSECUTIVE failed `send it` rounds, carried in or alongside the `.needs-spec-revision.json` marker (untracked daemon state). A "failed round" is a `send it` that exhausts the bounded converge attempts with a contradiction remaining (the budget-exhausted branch in `revision_session.rs`).
- [x] 1.2 Increment the counter on each budget-exhausted failure. Reset it to zero when the change clears (a clean re-gate that opens a PR) OR when the marker is cleared (`@<bot> clear-revision`, or the marker file is removed).

## 2. Nudge decomposition at the threshold

- [x] 2.1 Add a configurable threshold (default 3) for consecutive failed rounds.
- [x] 2.2 When the count reaches the threshold, the budget-exhausted failure reply SHALL, in addition to naming the remaining contradiction (existing behavior), recommend decomposing the change into smaller changes — stating that a change failing repeated revision rounds is likely too large or too interconnected to converge via `send it`. The operator may still `send it` again, but decomposition is the recommended path.
- [x] 2.3 Below the threshold, the failure reply is unchanged.

## 3. Tests

- [x] 3.1 After N (= threshold) consecutive failed rounds on one change, the failure reply contains the decomposition recommendation (assert on the reply content/behavior, not brittle full-string match).
- [x] 3.2 Below the threshold, the failure reply is the existing "names the contradiction, invites send it again" form with no decomposition nudge.
- [x] 3.3 The counter resets after a clean re-gate (PR opened) and after the marker is cleared — a later first failure does not start at the threshold.
