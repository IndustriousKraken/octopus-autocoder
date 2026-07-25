# Tasks: iteration-sequence-gates-once

## 1. Gate-pass record

- [x] 1.1 Add a small module for the state-dir gate-pass record at `<state_dir>/gate-pass/<workspace-basename>/<change>.json` (`inputs_hash`, `recorded_at`), with a path helper in `autocoder/src/paths.rs`, atomic write, and idempotent removal.
- [x] 1.2 Implement the inputs hash: SHA-256 over sorted (relative path, bytes) of every regular file under the change's directory, excluding the daemon marker files listed in design.md. Any I/O error during hashing surfaces as "no usable record" — never as a match.

## 2. Queue-walk integration

- [x] 2.1 In `autocoder/src/polling_loop/queue_walk.rs` `process_one_pending_change`, when the pickup is a continuation (iteration-pending marker present) AND a gate-pass record exists AND the recomputed hash matches, skip spawning the three pre-executor gate sessions and mark their ledger entries as the sequence's carried-forward verdicts; in every other case run the gates exactly as today.
- [x] 2.2 Write the record when all enabled pre-executor gates pass in one pickup; replace any existing record for the change.
- [x] 2.3 Remove the record at each sequence-terminating path where the iteration-pending marker is dropped (`Completed`, `Failed`, `SpecNeedsRevision`).
- [x] 2.4 Render carried-forward entries in the gate-verdict ledger with a "carried forward (sequence)" annotation.

## 3. Tests

- [x] 3.1 Test: continuation pickup with an unchanged hash spawns no pre-executor gate sessions and renders carried-forward ledger entries.
- [x] 3.2 Test: a mid-sequence edit to any file in the change's directory produces a hash mismatch and the gates run in full.
- [x] 3.3 Test: missing or unreadable record, or a hashing error, runs the gates in full (fail toward running).
- [x] 3.4 Test: a fresh pickup (no iteration-pending marker) always runs the gates, even when a stale record exists, and the record is replaced on pass.
- [x] 3.5 Test: the record is removed on each sequence-terminating outcome.

## 4. Validation

- [x] 4.1 Run `openspec validate iteration-sequence-gates-once --strict` and fix any findings.
