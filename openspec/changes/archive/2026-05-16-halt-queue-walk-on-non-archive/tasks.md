## 1. Code

- [x] 1.1 In `autocoder/src/polling_loop.rs::walk_queue`, find the match-on-`result` block (currently around line 952).
- [x] 1.2 In the `Ok(QueueStep::Failed { reason })` arm: after the existing `handle_failure_counter(...).await;` call, add an info log naming the halt reason AND `break;`.
- [x] 1.3 In the `Ok(QueueStep::Escalated)` arm: replace the empty body with an info log + `break;`.
- [x] 1.4 Added explanatory comment above the match block explaining the serial-queue invariant and the perma-stuck bound.

## 2. Tests

- [x] 2.1 `polling_loop::tests::walk_queue_halts_on_failed_change` — four pending changes (ch01, ch02-fails, ch03, ch04). Executor archives ch01, fails ch02-fails. Asserts processed = `["ch01"]`; ch02-fails / ch03 / ch04 still in `list_pending`; ch03's `.failure-state.json` does NOT exist (walker never reached it).
- [x] 2.2 `polling_loop::tests::walk_queue_halts_on_escalated_change` — three pending changes (ch01, ch02-asks, ch03). Executor archives ch01, returns AskUser for ch02-asks. Mockito chatops fixture. Asserts processed = `["ch01"]`; ch02-asks has `.question.json`; ch03 still in `list_pending`.
- [x] 2.3 `polling_loop::tests::walk_queue_failed_change_does_not_count_toward_cap` was the test that exercised the now-removed "continue on Failed" behavior. Replaced (renamed + rewritten) by `walk_queue_halts_on_failed_change` above. The cap-of-many-with-failure case is no longer meaningful; the failure halts before the cap matters.
- [x] 2.4 `walk_queue_stops_at_max_changes` — uses `PerChangeArtifactExecutor` (only archives). Unchanged. Confirmed passing.
- [x] 2.5 `walk_queue_cap_of_1_ships_one_per_pass` — same shape, only archives. Unchanged. Confirmed passing.
- [x] 2.6 `execute_one_pass_resumed_change_counts_toward_cap` — resume + pending archive flow, no in-walk failure. Unchanged. Confirmed passing.
- [x] 2.7 `same_repo_queue_blocks_when_another_change_waiting` — iteration-start gate, separate from in-walk halt. Unchanged. Confirmed passing.
- [x] 2.8 Inspected full `cargo test` output: 378/379 pass (1 ignored, unrelated). No other test depends on the removed "continue after Failed" behavior.

## 3. Documentation

- [x] 3.1 README "Operating Notes" → "Queue order" subsection: appended a paragraph describing the halt-on-fail/escalate rule and the perma-stuck escape valve.

## 4. Verification

- [x] 4.1 `cargo test` passes (378/379; 1 ignored, unrelated to this change).
- [x] 4.2 `openspec validate halt-queue-walk-on-non-archive --strict` passes.
