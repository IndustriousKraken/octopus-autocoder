# Tasks: durable-iteration-record

## 1. Iteration record module

- [ ] 1.1 Add a small module for the per-workspace iteration record at `<state_dir>/iteration-record/<workspace-basename>.json` (`finished_at`, `outcome` kind + one-line summary, `duration_secs`), with a path helper in `autocoder/src/paths.rs` and an atomic tempfile-then-rename write.
- [ ] 1.2 Write the record in the polling-task iteration driver at the single point where the iteration's result is handled, covering every terminal path: success-with-work (naming archived changes and processed issue units), idle empty queue, skipped (naming the park), audit-only, and failed (truncated reason). A write failure logs WARN and does not alter the iteration outcome.

## 2. Status reads the record

- [ ] 2.1 In `autocoder/src/control_socket/handlers.rs` `build_repo_status`, populate `last_iteration` from the iteration record; delete the failure-state-based fallback entirely.
- [ ] 2.2 Render `last iteration: no iteration yet` when no record exists (single-repo status block), matching the menu's existing placeholder wording; the `next iteration:` estimate derives from the record's `finished_at` plus the poll interval.
- [ ] 2.3 Render the record's outcome summary on the `outcome:` line; a failure appears there only when the last iteration itself failed.

## 3. Orphaned failure-entry pruning

- [ ] 3.1 At pass start (after branch sync), remove each failure-state file naming a change whose directory does not exist in the workspace, logging one INFO line per pruned entry. Retain entries for marker-excluded changes (their directories still exist).

## 4. Tests

- [ ] 4.1 Test: idle, failed, and success-with-work iterations each overwrite the record with the matching outcome kind and a fresh `finished_at`.
- [ ] 4.2 Test: status sources `last_iteration` from the record; with an old failure-state entry present and a fresh record, the block reflects the record and the old failure appears nowhere in it.
- [ ] 4.3 Test: no record present renders `no iteration yet`.
- [ ] 4.4 Test: pruning removes entries for absent change directories, retains entries for present (including marker-excluded) ones, and logs the removals.

## 5. Validation

- [ ] 5.1 Run `openspec validate durable-iteration-record --strict` and fix any findings.
