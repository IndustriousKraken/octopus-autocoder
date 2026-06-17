# Tasks

## 1. Make `strip_label` panic-free on non-ASCII input

- [x] 1.1 In `autocoder/src/lanes/ingestion.rs::strip_label`, replace the
  panicking guard `line.len() >= prefix.len() && line[..prefix.len()].eq_ignore_ascii_case(&prefix)`
  with a char-boundary-safe equivalent. Preferred form: match on
  `line.get(..prefix.len())` and test `head.eq_ignore_ascii_case(&prefix)`,
  or compare `line.as_bytes()` against `prefix.as_bytes()` over the prefix
  range. The function must return `Some`/`None` (never panic) for any `&str`
  input, including strings where a multi-byte character straddles
  `prefix.len()`.
- [x] 1.2 Keep the subsequent `line[prefix.len()..]` slice and the
  `trim_start_matches('*').trim()` behavior unchanged — it remains valid
  because a successful ASCII-prefix match guarantees byte `prefix.len()` is a
  char boundary.

## 2. Regression test

- [x] 2.1 Add a unit test `strip_label_does_not_panic_on_multibyte_boundary`
  in the `ingestion.rs` test module that calls
  `strip_label("日本語のバグ", "SLUG")` (and at least one other label whose
  byte length lands mid-codepoint, e.g. `"SUMMARY"`) and asserts it returns
  without panicking (result `None` is acceptable).
- [x] 2.2 Add a unit test
  `parse_triage_verdict_handles_non_ascii_lines_without_panic` that passes a
  multi-line string containing a valid `CLASSIFICATION: BUG` line plus a line
  of non-ASCII text whose prefix-length byte offset is mid-codepoint, and
  asserts `parse_triage_verdict` returns a `Some(TriageVerdict)` (the
  classification still parses) rather than panicking.
