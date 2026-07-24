## 1. Preamble plumbing

- [x] 1.1 In `autocoder/src/code_reviewer/agentic.rs`, add an operator-preamble parameter to the prompt rendering: when present, its content is placed at the very top of the rendered prompt, ahead of the quality-scope instruction and the cross-change preamble; all code-built sections are unchanged.
- [x] 1.2 Resolve the override path once per review via the same nested-then-legacy loader the oneshot path uses (`reviewer.code_review.prompt_path` → `reviewer.prompt_template_path`), reading the file at review time so edits apply on the next review without a reload.
- [x] 1.3 On a configured path that is unreadable or empty: do not spawn the session; route the review into its existing failed/discarded state (no verdict, never implicit Approve) and fire the reviewer-failure alert naming the file and cause.

## 2. Tests

- [x] 2.1 Unit tests on the rendered prompt: with an override file, its content is the first thing in the prompt and every code-built section still follows; without one, the prompt is byte-identical to the current rendering.
- [x] 2.2 Failure tests: configured-but-missing file and configured-but-empty file both produce the failed-review path with the alert, and no session spawn; removing the config key restores normal rendering.
- [x] 2.3 Both reviewer modes: `per_change` sessions each carry the operator preamble ahead of their cross-change preamble; `bundled` carries it once.
- [x] 2.4 Run the full `cargo test` suite; confirm oneshot template resolution and the existing agentic-mode tests are unchanged.

## 3. Docs

- [x] 3.1 Update the CodeReview row of the prompt-overrides table in `docs/CONFIG.md`: oneshot uses the file as the full template; agentic includes it as an operator guidance preamble — write judgment guidance (e.g. `should_request_revision` calibration), not output-format mechanics.
