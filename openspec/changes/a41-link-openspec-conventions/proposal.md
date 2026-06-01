## Why

A prior session direct-edited eight agent-facing prompts AND `docs/README.md` to carry a pointer to OpenSpec's upstream documentation (`https://github.com/Fission-AI/OpenSpec/tree/main/docs`). The links give agents drafting spec content AND humans drafting their first OpenSpec change a canonical reference for scenario syntax, delta format, AND requirement-header rules.

The links are in place. What is NOT yet in place: a regression test that catches their accidental removal. A future contributor editing one of the prompts could trim out the OpenSpec reference without noticing — the prompts are large AND the reference is a single paragraph that does NOT visibly anchor the prompt's operational rules. Without a check in CI, the convention quietly degrades.

This change adds the regression test AND captures the requirement (which prompts carry the link, what the link contains) as a canonical OpenSpec requirement so future audits can verify it.

## What Changes

**Regression test asserts the link is present in nine files.** The covered set is fixed by this change AND captured in the spec:

- `prompts/implementer.md`
- `prompts/implementer-revision.md`
- `prompts/chat-request-triage.md`
- `prompts/audit-triage.md`
- `prompts/missing-tests-audit.md`
- `prompts/security-bug-audit.md`
- `prompts/brownfield-draft.md`
- `prompts/scout.md`
- `docs/README.md`

For each file, the test reads the contents via `std::fs::read_to_string` AND verifies the literal substring `https://github.com/Fission-AI/OpenSpec` is present. A second assertion verifies each file ALSO contains at least one of the topical hints (`GIVEN`, `WHEN`, `scenario`, `delta`, OR `Requirement`) — this catches the case where someone leaves the URL in place but strips the surrounding explanation that gives the agent context.

**Combined-failure reporting.** The test SHALL produce a single combined failure listing (NOT first-failure-only) so a contributor editing several files at once sees every offender in one run.

**No new convention text.** The link text already in place stays as it is. This change does NOT re-author any of the prompts OR `docs/README.md` content; it only locks in their current state via the regression test.

**No spec-drafting convention file.** Per the prior session's design, project-specific deviations from upstream OpenSpec conventions do NOT yet warrant a dedicated `openspec/AGENTS.md` OR `docs/SPEC-CONVENTIONS.md`. If they accumulate enough to warrant consolidation, a future change can author the file AND update this requirement's covered set to include it.

## Impact

- **Affected specs:**
  - `project-documentation` — ADDED a new requirement defining the covered set of files (the nine listed above), the substring + topical-hint check, AND the combined-failure-reporting behavior of the test.
- **Affected code:**
  - New test at `autocoder/tests/openspec_pointers.rs` (OR an extension to an existing prompts-presence test file if one exists — check before creating).
- **Operator-visible behavior:** none directly. The link is already present; the test guards against accidental removal.
- **Backward compatibility:** none affected.
- **Dependencies:** none. Independent of every queued change.
- **Acceptance:** `cargo test` passes (including the new regression test); `openspec validate a41-link-openspec-conventions --strict` passes. Tests:
  - The regression test passes against the current repo state (all nine files contain both the URL substring AND a topical hint).
  - The regression test fails with a multi-line diagnostic if any file is modified to remove the URL substring OR the topical hints, with each offender named.
