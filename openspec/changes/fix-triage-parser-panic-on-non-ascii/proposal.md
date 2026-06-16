# Fix triage-verdict parser panic on non-ASCII output

## Why

`strip_label` in `autocoder/src/lanes/ingestion.rs:263` slices a string on a
**byte** index that is not guaranteed to fall on a UTF-8 character boundary:

```rust
fn strip_label<'a>(line: &'a str, label: &str) -> Option<&'a str> {
    let line = line.trim_start_matches('*').trim_start();
    let prefix = format!("{label}:");
    if line.len() >= prefix.len() && line[..prefix.len()].eq_ignore_ascii_case(&prefix) {
        Some(line[prefix.len()..].trim_start_matches('*').trim())
    } else {
        None
    }
}
```

The guard `line.len() >= prefix.len()` checks only the byte length, not
char-boundary alignment. `line[..prefix.len()]` therefore panics with
`byte index N is not a char boundary` whenever a multi-byte UTF-8 character
straddles byte offset `prefix.len()`. `strip_label` is called for **every
line** of the parsed text with the labels `CLASSIFICATION` (prefix 15 bytes),
`SLUG` (5 bytes), `SUMMARY` (8 bytes), and `TASKS` (6 bytes)
(`ingestion.rs:225-234`), so any line at least 5 bytes long whose 5th, 6th,
8th, or 15th byte lands mid-codepoint triggers the panic. A line containing
CJK text, accented Latin, or emoji (each multi-byte) does this trivially — e.g.
a line beginning `日本語…` (each character 3 bytes) panics on the 5-byte
`SLUG:` check at byte offset 5.

The text fed to this parser is **untrusted, attacker-influenceable LLM
output**. `parse_triage_verdict(&text)` is called at `ingestion.rs:848` where
`text` is the executor's `final_answer` from `executor.run_issue_triage` — an
LLM triage run over a GitHub issue report fetched from the forge. A public
issue author who includes non-ASCII content in the issue title or body (a
quoted error string, an emoji, an accented word, CJK text) can steer the model
into echoing multi-byte characters in its verdict, and any such character at
an unlucky byte offset crashes the parse.

**Harm:** denial of service. A single reported issue can panic the
issue-ingestion lane of the long-running daemon. The `parse_triage_verdict`
call at `ingestion.rs:848` is not wrapped in `catch_unwind`, so the panic
propagates up the issues-lane task and aborts that ingestion pass — a remotely
triggerable crash from public, unauthenticated input (anyone who can file an
issue on the watched repository).

Note the contrast with the sibling slice on the next line
(`line[prefix.len()..]`), which is safe: it executes only **after** the
`eq_ignore_ascii_case` match against an all-ASCII prefix has confirmed that
bytes `0..prefix.len()` are ASCII, so byte `prefix.len()` is a guaranteed char
boundary. Only the slice in the guard condition is unsafe.

## What Changes

- Rewrite the prefix check in `strip_label` to compare bytes without slicing
  on a possibly-invalid char boundary — compare `line.as_bytes()` against
  `prefix.as_bytes()` (byte slices have no char-boundary requirement), or use
  `line.get(..prefix.len())` and match on the returned `Option<&str>` (which
  yields `None` for a non-boundary index instead of panicking). Either form
  preserves the existing semantics (case-insensitive ASCII-prefix match,
  leading-`*` tolerance) and keeps the subsequent `line[prefix.len()..]` slice
  valid, while making the function total over arbitrary `&str` input.

- Add a regression test that feeds `strip_label` / `parse_triage_verdict` a
  line whose prefix-length byte offset falls inside a multi-byte character and
  asserts it returns normally (no panic) rather than crashing.

## Impact

- `autocoder/src/lanes/ingestion.rs` — `strip_label` (the fix) and a new unit
  test. No change to the public `parse_triage_verdict` signature or to triage
  classification/routing behavior for valid input.
- Spec: `orchestrator-cli` — "Triage routing classifies each report" gains a
  robustness invariant: parsing the triage verdict SHALL NOT panic on
  arbitrary (including non-ASCII) agent output.
