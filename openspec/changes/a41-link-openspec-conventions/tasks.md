# Implementation tasks

## 1. Regression test enforcing the OpenSpec-pointer convention

- [ ] 1.1 Add a test at `autocoder/tests/openspec_pointers.rs` (OR extend an existing prompts-presence test file if one is already present — check for `prompts_presence_test.rs` AND similar first). The test reads each of the nine files below via `std::fs::read_to_string` AND verifies the file contents contain BOTH (a) the literal substring `https://github.com/Fission-AI/OpenSpec`, AND (b) at least one of the topical hints `GIVEN`, `WHEN`, `scenario`, `delta`, OR `Requirement`.

  Covered files:

  - `prompts/implementer.md`
  - `prompts/implementer-revision.md`
  - `prompts/chat-request-triage.md`
  - `prompts/audit-triage.md`
  - `prompts/missing-tests-audit.md`
  - `prompts/security-bug-audit.md`
  - `prompts/brownfield-draft.md`
  - `prompts/scout.md`
  - `docs/README.md`

- [ ] 1.2 The test SHALL produce a single combined failure listing (NOT first-failure-only) so a contributor editing several files at once sees every offender in one run. Each entry in the failure message SHALL name the file path AND which check failed (URL substring missing, OR topical hint missing).
- [ ] 1.3 The test SHALL be deterministic — no network, no clock, no env mutation. File reads are the only I/O.
- [ ] 1.4 Path resolution: the test resolves file paths relative to the workspace root via `CARGO_MANIFEST_DIR` (the `autocoder/` crate dir) AND its parent (the repo root). Make the resolution explicit so the test passes regardless of how `cargo test` is invoked.

## 2. Acceptance gate

- [ ] 2.1 `cargo test` passes for the autocoder crate, including the new regression test.
- [ ] 2.2 `openspec validate a41-link-openspec-conventions --strict` passes.
- [ ] 2.3 Sanity check: temporarily delete the URL line from `prompts/implementer.md`, run the test, AND confirm it fails with `prompts/implementer.md: missing required substring 'https://github.com/Fission-AI/OpenSpec'`. Restore the line. (Do this once during implementation as a smoke test for the failure path; do NOT leave the test in a broken state.)
