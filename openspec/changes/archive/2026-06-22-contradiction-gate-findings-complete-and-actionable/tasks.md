# Tasks

OpenSpec: implements the two ADDED requirements in
`specs/orchestrator-cli/spec.md` (completeness; actionable suggested fix).

## 1. `[in]` gate prompt: exhaustive sweep + concrete suggested fix

- [x] 1.1 In `prompts/change-contradiction-check.md`, add a completeness
  directive: evaluate EVERY requirement against EVERY other requirement, report
  EVERY distinct contradiction, do NOT stop after the first; one requirement may
  conflict with several others, each its own entry.
- [x] 1.2 Add a `suggested_fix` directive: for each contradiction, produce a
  concrete edit plan — which requirement(s) to ADD / MODIFY / RENAME / REMOVE and
  a sketch of the resulting text — NOT a restatement. Make explicit that `summary`
  = why they conflict (one line) and `suggested_fix` = what to change and how.
- [x] 1.3 Reframe the `submit_contradictions` JSON example as illustrative of a
  set AND include the `suggested_fix` field in the example shape.
- [x] 1.4 Leave the existing NOT-a-contradiction guidance intact.

## 2. `[canon]` gate prompt: exhaustive sweep + wider scope + concrete suggested fix

- [x] 2.1 In `prompts/change-vs-canonical-check.md`, add the completeness
  directive (as 1.1, against canon).
- [x] 2.2 Widen the canon-reading instruction: replace "same — or related —
  capabilities" with a directive to read the canonical specs of EVERY capability
  whose invariants the change's behavior bears on — not only the
  name-matching/obvious one. Note that a change requirement can violate an
  invariant in a SECOND capability and that missing it forces another revision.
- [x] 2.3 Add the `suggested_fix` directive (as 1.2): concrete edit plan per
  finding (which requirement(s) to add/modify/rename/remove, with text sketch),
  distinct from the why-summary. For the common case, the suggested fix is often
  "turn the contradicted canonical requirement into a coherent MODIFIED delta of
  this change" OR "align the change's requirement to canon's existing term" — say
  which and sketch the text.
- [x] 2.4 Reframe the `submit_canon_contradictions` JSON example as illustrative
  of a set AND include the `suggested_fix` field.
- [x] 2.5 PRESERVE the MODIFIED false-positive guardrail (a MODIFIED delta is
  never a contradiction with its own same-titled canonical requirement).

## 3. Schema + finding structs: `suggested_fix` field

- [x] 3.1 Add `suggested_fix: String` to the `[in]` finding struct
  (`autocoder/src/preflight/change_contradiction.rs`) and the `[canon]` finding
  struct (`CanonContradictionFinding`, `autocoder/src/preflight/canon_contradiction.rs`),
  with the payload parsers defaulting it to empty when absent (back-compat).
- [x] 3.2 Add `suggested_fix` to the `submit_contradictions` and
  `submit_canon_contradictions` MCP tool schemas
  (`autocoder/src/mcp_askuser_server.rs`) as an optional string property; do NOT
  add a `maxItems` cap on the `contradictions` array.

## 4. Rendering: complete + actionable output

- [x] 4.1 In the marker `revision_suggestion` builders
  (`build_canon_contradiction_revision_suggestion` and the `[in]` equivalent in
  `autocoder/src/polling_loop/preflight_checks.rs`), render EVERY finding (no cap)
  AND, for each, render its `suggested_fix` on its own labeled line
  ("Suggested fix: …") distinct from the summary. When `suggested_fix` is empty,
  render identity + summary only. Trim the generic boilerplate footer to a single
  short line so the per-finding fixes are the prominent content.
- [x] 4.2 In the chatops alert rendering
  (`autocoder/src/polling_loop/alerts_throttle.rs`), surface each finding's
  `suggested_fix` alongside its identity, labeled distinctly from the summary.

## 5. Tests

- [x] 5.1 Plumbing/completeness: a submission of N (>2) findings → all N appear in
  the marker `revision_suggestion` AND alert, none dropped. Assert finding
  presence/count, not wording.
- [x] 5.2 Suggested-fix rendering: a finding with a non-empty `suggested_fix` →
  the marker AND alert render that fix labeled distinctly from the summary; a
  finding with an empty `suggested_fix` → identity + summary still render with no
  error. Assert the field is surfaced/labeled distinctly (data flow), not specific
  prose.
- [x] 5.3 Back-compat parse: a `submit_*` payload omitting `suggested_fix` parses
  with the field defaulting to empty.
- [x] 5.4 Do NOT add a test that greps the prompt files for phrasing (prompt
  wording is not a behavioral contract; recall and fix-concreteness are validated
  in operation).

## 6. Validation

- [x] 6.1 `cd autocoder && cargo test --bin autocoder` (the suite is known-flaky
  under parallel load — re-run / isolate any failure before treating it as real).
- [x] 6.2 `openspec validate contradiction-gate-findings-complete-and-actionable
  --strict` from the repo root.
