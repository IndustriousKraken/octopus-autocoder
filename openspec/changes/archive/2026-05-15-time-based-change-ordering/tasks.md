## 1. Code

- [x] 1.1 In `autocoder/src/queue.rs::list_pending`, change the sort step. Currently `out.sort();` (lexicographic by name). New: for each candidate, read `<change-dir>/proposal.md` metadata via `std::fs::metadata(...).modified()`. Build `Vec<(SystemTime, String)>` from `(mtime, name)` pairs. Sort by mtime ascending; secondary key on name ascending. Map back to `Vec<String>` of names.
- [x] 1.2 If `metadata` or `modified()` errors for a candidate (unusual — would mean the proposal exists but is unreadable), use `SystemTime::UNIX_EPOCH` as the sort key so the entry still appears in the output (deterministic ordering even on degenerate filesystems).

## 2. Tests

- [x] 2.1 `queue::tests::list_pending_orders_by_proposal_mtime_ascending` — fixture: create two pending changes. Write `proposal.md` for `b-change` first, then sleep enough to differentiate mtimes (use `tokio::time::sleep` or `std::thread::sleep` for 10 ms), then write `proposal.md` for `a-change`. Assert `list_pending` returns `["b-change", "a-change"]` — alphabetically reversed but mtime-correct.
- [x] 2.2 `queue::tests::list_pending_breaks_mtime_ties_alphabetically` — fixture: two changes whose `proposal.md` files have IDENTICAL mtime (set both with `filetime::set_file_mtime`). Assert returned order is `["a-change", "b-change"]` (alphabetical).
- [x] 2.3 `queue::tests::list_pending_excludes_perma_stuck_after_ordering_change` — sanity check that the previously-added `.perma-stuck.json` exclusion still works alongside the new sort key.
- [x] 2.4 **Verify:** existing `queue::list_pending` tests that asserted on alphabetical order MAY need updates if their fixtures wrote files in non-alphabetical order. Inspect and update. (Audited: all existing tests either assert single-element results or write fixtures in an order that matches both alphabetical and mtime sort; no changes needed.)

## 3. Documentation

- [x] 3.1 README "Operating Notes" or wherever queue ordering is described: brief mention that the queue is processed in `proposal.md` mtime order (oldest first), with names as a tiebreaker. Operators who want a specific order should author specs in the order they want them applied.

## 4. Dependency

- [x] 4.1 If `filetime` is not already a dev-dependency (it's used in §2.2 to forcibly tie mtimes), add it to `[dev-dependencies]` in `autocoder/Cargo.toml`. It's a tiny, well-maintained crate.

## 5. Verification

- [x] 5.1 `cargo test` passes.
- [x] 5.2 `openspec validate time-based-change-ordering --strict` passes.
