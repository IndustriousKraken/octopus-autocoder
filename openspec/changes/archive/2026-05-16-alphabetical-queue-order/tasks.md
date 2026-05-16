## 1. Code

- [x] 1.1 In `autocoder/src/queue.rs::list_pending`, reverted the sort step. Removed the `Vec<(SystemTime, String)>` construction, the `std::fs::metadata().modified()` call, and the unused `use std::time::SystemTime` import. Now pushes `name: String` directly onto `out`, then `out.sort();` at the end.
- [x] 1.2 The function's doc comment updated to describe alphabetical (entry name) ordering and the `01-`/`02-` prefix convention.

## 2. Tests

- [x] 2.1 Removed `queue::tests::list_pending_orders_by_proposal_mtime_ascending`.
- [x] 2.2 Removed `queue::tests::list_pending_breaks_mtime_ties_alphabetically`.
- [x] 2.3 Removed `queue::tests::list_pending_excludes_perma_stuck_after_ordering_change` — perma-stuck exclusion still covered by `list_pending_excludes_perma_stuck`.
- [x] 2.4 Existing tests that asserted on alphabetical order continue to pass (all 15 queue tests pass).

## 3. Dependency

- [x] 3.1 Removed `filetime = "0.2"` from `autocoder/Cargo.toml` `[dev-dependencies]`.

## 4. Documentation

- [x] 4.1 README "Queue order" subsection rewritten to describe alphabetical ordering and the `01-`/`02-` prefix convention for stacked-dependency cases.

## 5. Verification

- [x] 5.1 `cargo test` passes (375/376; 1 ignored, unrelated).
- [x] 5.2 `openspec validate alphabetical-queue-order --strict` passes.
