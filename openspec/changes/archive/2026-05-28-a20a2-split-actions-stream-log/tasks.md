## 1. Dual-file structured log writer

- [x] 1.1 Refactored `autocoder/src/executor/event_log.rs`: `Inner` now holds two file handles (`summary_file`, `stream_file`); `open()` creates both at the summary path and its sibling `.stream.log` path eagerly so the empty-stream invariant holds; `write_prompt` writes PROMPT to summary then the `=== ACTIONS (see <basename>.stream.log) ===` pointer line; `append_action` writes to the stream file; `set_final_answer` and `finalize` write FINAL ANSWER + STDERR to the summary. Added `stream_path()` accessor and module-level `stream_path_for()` helper. Module docstring updated to describe the dual-file model.
- [x] 1.2 Dispatch routing complete: prompt → summary; tool_use / tool_result / assistant / raw / unknown → stream; result event text → summary FINAL ANSWER; stderr bytes → summary STDERR.
- [x] 1.3 The summary log's pointer line is written ONCE in `write_prompt`, regardless of whether any action events arrive later. Zero-action runs still emit the line.
- [x] 1.4 Both files created lazily at `open()` time; both have their content flushed at `finalize()`. The summary log's four section markers are written by `write_prompt` (first two) and `finalize` (last two), preserving the structural-completeness invariant.
- [x] 1.5 Tests in `event_log.rs`:
  - Updated `write_prompt_then_actions_then_finalize_produces_all_sections` to assert summary contains the pointer line AND no action content; stream contains the verbose action lines AND no section headers.
  - Updated `timeout_case_writes_empty_final_answer_section`: summary has empty FINAL ANSWER; stream has the partial actions.
  - Added `zero_action_run_creates_both_files_with_empty_stream` — both files exist; stream is empty.
  - Updated `unknown_kind_uses_event_type_in_prefix` and `raw_kind_uses_raw_prefix` to read from the stream file.
  - Added `stream_path_for_replaces_log_extension` and `stream_path_for_handles_non_log_extension` covering the path-derivation helper.

## 2. Retention pass updates

- [x] 2.1 Rewrote `prune_stale_logs` in `autocoder/src/log_retention.rs` with two passes per workspace directory: first, summary logs (delete-as-pair on archive-with-stale-mtime, preserve-as-pair on active); second, orphan-stream cleanup (stream logs without summary siblings, eligible by age + no change directory). Partial-success on stream-delete logs WARN and lets the next pass pick up the orphan.
- [x] 2.2 Active-change preservation extended to the pair: when a summary is preserved, the sibling stream (if present) is preserved AND counted in `files_preserved`.
- [x] 2.3 Orphan cleanup: stream-only files past the retention window with no change directory are deleted with a WARN naming the orphan path. Active-change stream-orphans (rare) are preserved.
- [x] 2.4 Added helpers `is_summary_log` and `parse_stream_change_name` to cleanly distinguish summary vs stream files.
- [x] 2.5 Tests in `log_retention.rs`:
  - `stale_pair_for_archived_change_is_deleted_atomically` — both files deleted in one pass.
  - `stale_pair_for_active_change_preserves_both` — both files preserved when change directory exists.
  - `summary_alone_for_archived_change_still_deletes` — legacy-shape (pre-a20a2) summary-only logs delete fine.
  - `orphan_stream_log_for_archived_change_is_cleaned_up` — orphan stream WITHOUT summary deleted.
  - `orphan_stream_log_for_active_change_is_preserved` — active-change orphan stream preserved.
  - `recent_orphan_stream_log_is_preserved` — under-window orphan stream preserved.
  - `is_summary_log_distinguishes_summary_from_stream` and `parse_stream_change_name_extracts_base` — helper unit tests.

## 3. Daemon-internal consumers unchanged

- [x] 3.1 The PR-comment composer reads FINAL ANSWER from the summary log via `event_log::read_final_answer` (path-derived; unchanged). The `a20a1` sentinel scanner reads `outcome.final_answer` from the structured outcome (file-shape-independent; unchanged). No daemon code path reads action content from log files for daemon-meaningful pattern matching.
- [x] 3.2 No additional consumer changes required — verified by reviewing call sites of `run_log_path` and `read_final_answer`; both are summary-log scoped.
- [x] 3.3 The four `json_streaming_*` tests in `claude_cli.rs` were updated to read action content from the stream-log path (`run_log_path(...).with_extension("stream.log")`); summary-log assertions now check for the pointer line and absence of action-prefixed content.

## 4. Docs update

- [x] 4.1 Updated `docs/OPERATIONS.md`'s "Per-change run log shape" section: describes the two-file layout, the ACTIONS pointer line, section-by-section breakdown, operator CLI snippets for `tail -f` and `grep` against each file, AND an explicit migration note ("Tools that previously grepped `<change>.log` for `[tool_use]`/... patterns SHALL be redirected to the `<change>.stream.log` file"). The retention paragraph now describes pair-atomic deletion AND orphan cleanup.
- [x] 4.2 No other doc updates required — the canonical retention requirement language is filename-agnostic.

## 5. Spec deltas

- [x] 5.1 `specs/executor/spec.md` MODIFIES `Executor invokes Claude CLI in JSON event streaming mode and captures events to a structured log` (5 preserved scenarios + 2 new) AND `Per-change log files are pruned after executor.log_retention_days days, preserving active-change logs` (3 preserved + 2 new). Validated.

## 6. Verification

- [x] 6.1 `cargo test --bin autocoder`: 1623 passed. One flaky timing test (`chatops::event_dedup::tests::key_past_ttl_is_treated_as_fresh_and_resets_count`) fails under load but passes in isolation — pre-existing, unrelated to a20a2 (chatops::event_dedup is untouched).
- [x] 6.2 `openspec validate a20a2-split-actions-stream-log --strict` passes.
- [x] 6.3 `cargo build --release` clean. Clippy on touched files: 3 pre-existing warnings in `executor/claude_cli.rs` at lines I did not modify; same baseline as a20a1.
- [ ] 6.4 Manual verification on a live iteration — deferred to operator after the daemon picks up this change.
