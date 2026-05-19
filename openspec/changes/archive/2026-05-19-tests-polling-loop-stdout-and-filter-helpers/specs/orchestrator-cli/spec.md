## ADDED Requirements

### Requirement: Polling-loop helpers handle their boundary inputs without panicking
Three small pure helpers in the polling loop (`extract_stdout_section`, `filter_alert_state_lines`, `truncate_reason`) have branchy behavior whose boundaries change observable operator-facing output: the PR-comment summary the implementer posts, the workspace-dirty alert that fires when uncommitted changes are detected, and the perma-stuck chatops excerpt. Each helper SHALL behave deterministically across the boundary inputs below and SHALL NOT panic on malformed or multi-byte input.

#### Scenario: extract_stdout_section returns the slice between markers
- **WHEN** `extract_stdout_section` is called with a log body containing both a `=== STDOUT (...)` header line AND a `=== STDERR (...)` line
- **THEN** the returned slice is the text strictly between the newline after the STDOUT header and the start of the STDERR marker

#### Scenario: extract_stdout_section returns empty when STDOUT marker is missing
- **WHEN** `extract_stdout_section` is called with a body that contains no `=== STDOUT (` substring
- **THEN** the returned slice is empty (no panic, no false-positive content)

#### Scenario: extract_stdout_section returns empty when STDOUT header has no terminating newline
- **WHEN** `extract_stdout_section` is called with a body containing `=== STDOUT (n) ===` but no `\n` after that header
- **THEN** the returned slice is empty (the early-return guard against partial input fires)

#### Scenario: extract_stdout_section runs to EOF when STDERR marker is absent
- **WHEN** `extract_stdout_section` is called with a body whose STDOUT marker is present AND whose STDERR marker is absent
- **THEN** the returned slice is the body from just after the STDOUT header line through end-of-input

#### Scenario: filter_alert_state_lines strips only exact-path entries
- **WHEN** `filter_alert_state_lines` is called with porcelain text containing a mix of real-file entries AND a line whose path is exactly `.alert-state.json`
- **THEN** the returned text omits the `.alert-state.json` line AND preserves every other entry verbatim
- **AND** a line whose path is `subdir/.alert-state.json` OR `prefix.alert-state.json` is NOT filtered (the check is exact-equality, not substring match)

#### Scenario: truncate_reason boundary behavior
- **WHEN** `truncate_reason` is called with input length less than or equal to its cap
- **THEN** the returned string equals the input verbatim AND does not end with `…`
- **AND WHEN** the input length exceeds the cap
- **THEN** the returned string ends with `…` AND its `chars().count()` equals the cap plus one
- **AND** truncation respects UTF-8 char boundaries (no panic on multi-byte input even when byte-count and char-count diverge)
