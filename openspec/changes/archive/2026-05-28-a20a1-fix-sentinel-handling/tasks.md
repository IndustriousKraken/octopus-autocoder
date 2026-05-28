## 1. Revise `prompts/implementer.md`

- [x] 1.1 Replaced the sentinel section with: substitution-instruction paragraph; worked example using `task_id: "6.4"` and the SSH-to-production-host scenario; field-by-field guidance; self-check hint naming the daemon's placeholder-detection diagnostic.
- [x] 1.2 The sentinel JSON shape is unchanged; only the surrounding prose + example changed.
- [x] 1.3 Added `bundled_implementer_prompt_worked_example_parses` in `claude_cli.rs` — extracts the sentinel from the embedded `DEFAULT_IMPLEMENTER_TEMPLATE` AND asserts deserialization yields a SpecNeedsRevision outcome with no placeholder text in any field. Guards against future prompt-template regressions.

## 2. Parser-side placeholder detection

- [x] 2.1 `try_parse_spec_needs_revision` now scans `task_id`, `task_text`, AND `reason` for the `<[a-z][a-z0-9 _-]*>` pattern after successful per-field parse. Returns `Err` with the documented diagnostic phrase (`looks like un-substituted placeholders — the agent emitted the prompt's example verbatim instead of substituting concrete values; see prompts/implementer.md sentinel section`). The Failed-reason format extended to include both `parse_err` AND `excerpt` so the operator sees the placeholder diagnostic directly in the Failed reason, not just the WARN log.
- [x] 2.2 Regex narrowed to lowercase-leading. Uppercase / digit-leading / symbol-leading angle-bracket content (HTML tags, MyType refs, !doctype, real markup) does NOT trip the detector. False positives still possible on legitimate lowercase content; treated as acceptable per spec.
- [x] 2.3 Tests added in `claude_cli.rs::tests`:
  - `placeholder_detection_matches_template_markers` — every literal from the pre-fix template trips the detector.
  - `placeholder_detection_ignores_uppercase_and_special_chars` — `<HTML>`, `<MyType>`, `<!doctype>`, `<3>`, `<>`, plain text don't trip.
  - `placeholder_detection_catches_template_in_task_id` — sentinel-end-to-end test asserting the Err contains the diagnostic phrase AND `prompts/implementer.md` AND `task_id`.
  - `placeholder_detection_catches_template_in_task_text_and_reason` — covers the other two fields.
  - `placeholder_detection_tolerates_legitimate_angle_brackets` — `Render <HTML> tags` task succeeds.
  - Existing `parse_spec_needs_revision_missing_required_field_falls_back_to_failed` still passes (non-placeholder parse failures take their canonical path).

## 3. Spec deltas

- [x] 3.1 `specs/executor/spec.md` ADDs the worked-example-mandate requirement AND the timeout-precedence + scan-scoping requirement.
- [x] 3.2 `specs/orchestrator-cli/spec.md` ADDs the placeholder-detection requirement.

## 4. Timeout precedence AND sentinel-scope tightening

- [x] 4.1 In `classify_outcome`, the `outcome.timed_out` check now fires BEFORE the sentinel extraction. A timed-out run returns `Failed { reason: "timeout" }` immediately. The sentinel scan never runs on timeout output.
- [x] 4.2 Replaced `outcome.final_answer.as_deref().unwrap_or(&outcome.stdout)` with a `match self.output_format` block: `Json => outcome.final_answer.as_deref()` (no stdout fallback), `Text => Some(outcome.stdout.as_str())`. Used `if let Some(source) = sentinel_source && let Some(payload) = ...` chained let-binding.
- [x] 4.3 `self.output_format` was already accessible on `&self`; no threading needed.
- [x] 4.4 Tests added in `claude_cli.rs::tests`:
  - `timed_out_run_with_sentinel_in_stdout_returns_timeout` — well-formed sentinel in stdout + timed_out=true → `Failed { reason: "timeout" }`.
  - `timed_out_run_with_line_numbered_prompt_echo_returns_timeout` — the exact `\n31\t` shape from the a21 incident → timeout.
  - `json_mode_sentinel_only_scanned_in_final_answer` — sentinel in stdout, final_answer present without sentinel → no SpecNeedsRevision.
  - `json_mode_sentinel_in_final_answer_is_honored` — happy path: sentinel in final_answer → SpecNeedsRevision.
  - `json_mode_with_no_final_answer_skips_sentinel_scan` — final_answer=None in JSON mode → no scan.
  - `text_mode_sentinel_in_stdout_is_honored` — text mode preserves legacy stdout-as-emission semantics.
  - `text_mode_timeout_precedence_skips_sentinel_scan` — text mode also yields to timeout precedence.
  - Updated `malformed_spec_needs_revision_sentinel_yields_failed` AND `spec_needs_revision_sentinel_routes_through_run` to use `with_output_format(Text)` — shell-script fixtures don't simulate JSON event streams, so the text-mode semantic is what they actually exercise.

## 5. Verification

- [x] 5.1 `cargo test --bin autocoder`: 1613 passed, 0 failed, 2 ignored.
- [x] 5.2 `openspec validate a20a1-fix-sentinel-handling --strict` passes.
- [x] 5.3 Clippy on the touched file: no new warnings. Three pre-existing warnings in `claude_cli.rs` (lines 127, 671, 676) are in code I didn't modify.
- [ ] 5.4 Manual verification (post-implementation, post-deploy) — deferred to operator.
