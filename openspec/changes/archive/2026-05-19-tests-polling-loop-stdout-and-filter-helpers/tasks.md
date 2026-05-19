## 1. `extract_stdout_section` parser branches

- [x] 1.1 `extract_stdout_section_returns_body_between_markers` —
  given input `"=== STDOUT (10) ===\nhello world\n=== STDERR (0)
  ===\nignored\n"`, assert the returned `&str` equals
  `"hello world\n"`.
- [x] 1.2 `extract_stdout_section_returns_empty_when_no_stdout_marker`
  — given input `"no markers anywhere\n=== STDERR (0) ===\n"`,
  assert the returned slice is `""`.
- [x] 1.3 `extract_stdout_section_returns_empty_when_header_has_no_newline`
  — given input `"=== STDOUT (10) ==="` (no trailing newline after
  the header), assert the returned slice is `""` (the
  `find('\n')` early-return branch).
- [x] 1.4 `extract_stdout_section_returns_to_eof_when_no_stderr_marker`
  — given input `"=== STDOUT (5) ===\nbody only\n"`, assert the
  returned slice equals `"body only\n"` (the `find(stderr_marker)`
  fallback to `raw.len()` branch).

## 2. `filter_alert_state_lines` porcelain filter

- [x] 2.1 `filter_alert_state_lines_passes_through_when_no_alert_state` —
  given porcelain `" M src/foo.rs\n?? new.txt\n"`, assert the
  returned string is unchanged.
- [x] 2.2 `filter_alert_state_lines_strips_only_alert_state_entry` —
  given porcelain `"?? .alert-state.json\n"`, assert the returned
  string is empty (or whitespace-only).
- [x] 2.3 `filter_alert_state_lines_keeps_real_files_and_strips_alert_state`
  — given porcelain `" M src/foo.rs\n?? .alert-state.json\n M
  src/bar.rs\n"`, assert the returned string contains the two
  `src/` lines and does NOT contain `.alert-state.json`.
- [x] 2.4 `filter_alert_state_lines_does_not_match_subpath_or_similar_name`
  — given porcelain `" M subdir/.alert-state.json\n?? prefix.alert-state.json\n"`,
  assert BOTH lines survive (the production check is exact-equality
  on the path component, not a `contains`).

## 3. `truncate_reason` boundary behavior

- [x] 3.1 `truncate_reason_passthrough_when_under_or_equal_to_cap` —
  build a string of exactly `PERMA_STUCK_REASON_EXCERPT_MAX` ASCII
  characters, call `truncate_reason`, assert the result equals the
  input AND does not end with `'…'`.
- [x] 3.2 `truncate_reason_truncates_and_appends_ellipsis_when_over_cap`
  — build a string of `PERMA_STUCK_REASON_EXCERPT_MAX + 50` ASCII
  characters, call `truncate_reason`, assert the result's
  `chars().count()` equals `PERMA_STUCK_REASON_EXCERPT_MAX + 1`
  (`MAX` chars of input + one `…`) AND the final char is `'…'`.
- [x] 3.3 `truncate_reason_respects_char_boundary_on_multibyte_input`
  — build a string composed of multibyte characters
  (e.g. `"é".repeat(PERMA_STUCK_REASON_EXCERPT_MAX + 50)`), call
  `truncate_reason`, assert no panic occurs and the result's
  `chars().count()` equals `PERMA_STUCK_REASON_EXCERPT_MAX + 1`.
