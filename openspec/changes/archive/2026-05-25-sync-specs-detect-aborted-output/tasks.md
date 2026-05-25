## 1. Detect openspec's `Aborted.` marker

- [x] 1.1 Add `pub fn detect_openspec_abort(stdout: &str) -> Option<String>` in `autocoder/src/cli/sync_specs.rs`. The function scans `stdout` line-by-line and returns:
  - `Some(reason)` when a line begins with `Aborted.` (after trimming leading whitespace). `reason` is the most informative preceding line — the line immediately before the `Aborted.` line if non-empty, otherwise the `Aborted.` line itself. This captures patterns like the real-world case where `MODIFIED failed for header "..." - not found` precedes the `Aborted. No files were changed.` line.
  - `None` when no `Aborted.` line is present.
- [x] 1.2 The detection is line-based, not substring-based: a stdout line that happens to contain the word `aborted` lowercase, or `Aborted` inside a quoted code block on the same line as other content, does NOT match. Only a line whose first non-whitespace token is `Aborted.` triggers the detection.
- [x] 1.3 Tests:
  - Real-world case: stdout containing `member-saved-cards MODIFIED failed for header "..." - not found\nAborted. No files were changed.` returns `Some("member-saved-cards MODIFIED failed for header \"...\" - not found")`.
  - `Aborted.` on its own line, no preceding line: returns `Some("Aborted. No files were changed.")` (or just `Aborted.` if no trailing text).
  - Clean archive stdout (`Specs to update: ...; Applying changes to ...; Totals: ...; Specs updated successfully.`): returns `None`.
  - The literal word "aborted" lowercase mid-sentence: returns `None`.
  - `Aborted` (no trailing period) on its own line: returns `None` — the period is part of the openspec signal pattern.

## 2. Integrate detection into the rebuild loop

- [x] 2.1 In the success-path branch of the rebuild loop (currently lines 422-470 of `sync_specs.rs`), insert an `Aborted.`-marker check BEFORE the post-condition verification. The new flow:
  1. Exit code zero → proceed
  2. Log captured stdout at INFO if non-empty (unchanged).
  3. **NEW:** `if let Some(reason) = detect_openspec_abort(&out.stdout)` → build a headline-format failure_reason `format!("openspec refused to apply: {reason}; full output: {full}", full = format_archive_output_for_report(&out))`, call `record_failure_with_rollback`, continue.
  4. Otherwise, run `verify_archive_post_condition` as today.
- [x] 2.2 The post-condition check remains in place as a safety net. If openspec ever changes its `Aborted.` wording without changing exit behavior, the post-condition check still catches the silent skip — operators just get the older, less-informative failure message until the marker string is updated. Defense in depth is the design intent.
- [x] 2.3 Tests:
  - New `AbortedOutputArchiveRunner` stub: exits 0, prints `MODIFIED failed for header "X" - not found\nAborted. No files were changed.`, performs no fs work.
  - Integration test with the new stub: the failure_reason starts with `openspec refused to apply:`, the report's `rolled_back` count increments, the change directory is back in `archive/`, the active path is empty.
  - Integration test with a mixed runner stub (some slugs succeed, some abort, some silent-skip without the marker): each failure category gets its appropriate failure_reason headline (`openspec refused to apply:` for the abort case, `openspec archive exited 0 but post-condition failed:` for the marker-less silent skip).
  - Existing post-condition tests continue to pass (the post-condition path still fires when the marker is absent).

## 3. PR body text update

- [x] 3.1 In `autocoder/src/polling_loop.rs` (the rebuild-PR creation path near line 1866), replace `**Failed changes** (left at active path for operator inspection):` with `**Failed changes** (rolled back to archive — see failure reasons below for the openspec output explaining each):`.
- [x] 3.2 In the same function, update the summary line currently rendered as `"Replayed N archived change(s) chronologically; X succeeded, Y failed."` to include the rolled-back count when non-zero: `"Replayed N archived change(s) chronologically; X succeeded, Y failed (Z rolled back to archive)."`. When `rolled_back == failed`, both numbers will match and the operator confirms at a glance that the workspace is clean. When they differ (data-loss-shaped failures, rollback-of-rollback failures), the operator sees the gap and digs into the failure reasons.
- [x] 3.3 Tests:
  - PR body snapshot test for a fixture with 3 successes + 0 failures: summary contains no parenthetical, no failures section.
  - PR body snapshot test for 3 successes + 2 failures both rolled back: summary reads `5 archived change(s) chronologically; 3 succeeded, 2 failed (2 rolled back to archive).` and the failures section header reads "rolled back to archive — see failure reasons below for the openspec output explaining each".
  - PR body snapshot test for 3 successes + 2 failures with 1 rollback failure (rare): summary reads `..., 2 failed (1 rolled back to archive).` and the failure_reason for the unrolled-back entry includes "rollback ALSO failed" per the existing atomicity spec.

## 4. Spec delta

- [x] 4.1 The ADDED requirement in `openspec/changes/sync-specs-detect-aborted-output/specs/orchestrator-cli/spec.md` codifies: the `Aborted.` stdout-marker detection rule, the failure-reason headline format when it fires, the fact that the post-condition check remains as a defense-in-depth fallback, and the PR-body wording requirements (summary line includes rolled-back count, failures-section header describes rollback rather than active-path retention).

## 5. Verification

- [x] 5.1 `cargo test` passes (new + existing).
- [x] 5.2 `openspec validate sync-specs-detect-aborted-output --strict` passes.
- [x] 5.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
