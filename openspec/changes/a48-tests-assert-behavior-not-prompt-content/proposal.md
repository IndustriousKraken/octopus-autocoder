## Why

Several tests assert the verbatim wording of an embedded prompt — e.g. `low_confidence_finding_filtering_explicit_in_prompt` (`autocoder/src/audits/security_bug.rs`) asserts four full sentences are present in the security-bug audit prompt. A meaning-preserving reword of the prompt left two of those assertions failing, so `cargo test` is red, and an implementer agent then mis-attributed the failure as "pre-existing and unrelated" and shipped anyway.

The defect is the category of test, not its brittleness. A test that reads a real shipped prompt and checks for a hand-authored substring is a change-detector: it passes because someone typed the words and fails because someone retyped them differently. It encodes no independent truth, cannot tell a better reword from a deletion, and does not exercise the feature (whether the audit actually drops low-confidence findings is model behavior, invisible to a substring check). The wording rides the same PR/review pipeline as code, so review already covers it.

The healthy line is: a test asserts what the code DOES (behavior), or that mechanically-derived output matches its source (derivation). Design intent about a prompt's *content* belongs in requirement prose, where the existing drift audit verifies it semantically — not in a unit test that pins exact wording. This change states that rule as a requirement (so the drift audit flags future wording-tests), removes the canonical requirements that mandated wording-tests, and deletes or refactors the offending tests.

Coarse "tripwire" content checks (assert a URL or keyword is still present) are the same category, not an exception — they guarantee nothing review and the drift audit do not. That includes the `a41` regression test asserting prompts contain the OpenSpec docs URL; its requirement is removed here. The OpenSpec links remain as prompt content.

## What Changes

**New requirement codifies the healthy test form (project-documentation).** `Tests assert behavior or derivation, never message wording` states: tests assert behavior or derivation-from-source; a test SHALL NOT read a real shipped prompt/message and assert a hand-authored substring of its prose; behavior tests use synthetic fixtures; a behavior-relevant property of a real prompt (e.g. it references a placeholder the substitution code fills) is verified by rendering with sentinel inputs and asserting the substituted values, never the surrounding wording. This requirement is the source of truth the drift audit enforces against — a wording-assertion test becomes a drift-audit finding.

**Two canonical requirements that mandated wording-tests are removed or softened.**
- The `a41` requirement `OpenSpec upstream-docs pointer is regression-tested across the spec-drafting prompt set AND `docs/README.md`` is REMOVED. It is a pure content test with no behavior; the links survive as reviewed prompt content.
- The orchestrator-cli `Security & bug audit` scenario `Prompt instructs confidence-filtered output` is softened from verbatim-sentence quotes to design intent, so the drift audit verifies meaning rather than exact wording and deleting the verbatim test leaves canon consistent.
- The code-reviewer `Default prompt template enforces code-quality scope` requirement keeps its scope-intent statement in prose but replaces the verbatim scope-sentence assertion with a sentinel-substitution behavior scenario.

**Offending tests are deleted or refactored.** Pure wording assertions are deleted (no token-check replacements). Tests that asserted a behavior-relevant property of a real prompt are refactored to synthetic fixtures or sentinel-substitution. Loader-precedence tests that compared against a real prompt's wording are refactored to symbol identity.

**The broader sweep is intentionally left to the drift audit.** This change touches only the canonical requirements whose tests it removes; other audits' wording scenarios (e.g. `architecture_consultative`'s rewrite-at-scale scenario) are not hand-swept here. The new requirement makes each a drift-audit finding as the audit re-runs. This is a deliberate, documented choice, not silent truncation.

## Impact

- **Affected specs:**
  - `project-documentation` — ADDED `Tests assert behavior or derivation, never message wording`. REMOVED `OpenSpec upstream-docs pointer is regression-tested across the spec-drafting prompt set AND `docs/README.md``.
  - `code-reviewer` — MODIFIED `Default prompt template enforces code-quality scope` (intent in prose; sentinel-substitution scenario replaces the verbatim scope-sentence assertion).
  - `orchestrator-cli` — MODIFIED `Security & bug audit` (scenario `Prompt instructs confidence-filtered output` softened verbatim → intent; six other scenarios preserved verbatim).
- **Affected code (tests only; no production behavior changes):**
  - DELETE `low_confidence_finding_filtering_explicit_in_prompt` (`autocoder/src/audits/security_bug.rs`).
  - DELETE the wording assertions in `autocoder/src/code_reviewer.rs` (the `DEFAULT_TEMPLATE.contains("…revision-requests…")` / `should_request_revision` / `actionable_request` / `most-critical-first` group, and the `DEFAULT_TEMPLATE.contains("You are reviewing code quality only…")` / `VERDICT:` group).
  - DELETE the `a41` OpenSpec-pointer regression test.
  - REFACTOR the loader-override test that asserts the default template's scope sentence is absent (`code_reviewer.rs` `~1228`) to compare template identity instead of wording.
  - ADD a sentinel-substitution test that renders the real `code-review-default.md` with distinct per-placeholder sentinels and asserts each appears (verifies the shipped default references all three placeholders without pinning prose).
- **Operator-visible behavior:** none. This change removes and rewrites tests and the requirements that mandated them; no runtime path changes.
- **Acceptance:** `cargo test` passes (the red test is removed, not worked around); `openspec validate a48-tests-assert-behavior-not-prompt-content --strict` passes.
- **Dependencies:** none. Independent of the queued a43–a47 changes.
