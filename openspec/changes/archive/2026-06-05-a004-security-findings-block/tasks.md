# Implementation tasks

## 1. Prompt rule

- [x] 1.1 In the reviewer prompt (`prompts/code-review-default.md` and any role override path), add an explicit severity rule: credential/secret leakage, secret exposure, AND injection vulnerabilities are `Block`-class findings — never `Concerns` or `Pass`. Keep the rest of the verdict guidance intact.

## 2. Code-level escalation safety net

- [x] 2.1 In the verdict-handling path (`code_reviewer.rs`), detect when the reviewer's structured output marks a finding as a secret/credential/key exposure (or injection). Use the reviewer's own finding signal/classification, not a substring scan of the prose.
- [x] 2.2 When such a finding is present AND the returned verdict is not `Block`, escalate the effective verdict to `Block` before the PR-draft / auto-revise handling runs.
- [x] 2.3 Leave non-security findings' verdicts untouched.

## 3. Tests

- [x] 3.1 A reviewer result carrying a credential/secret-leak finding with a `Concerns` (or `Pass`) verdict yields an effective `Block` (assert the verdict).
- [x] 3.2 An injection finding with a non-`Block` verdict escalates to `Block`.
- [x] 3.3 A `Concerns` verdict with only non-security findings stays `Concerns` (no escalation).
- [x] 3.4 The escalation is driven by the structured finding signal, not by message wording (assert behavior under a synthetic finding, not a copy match).

## 4. Acceptance gate

- [x] 4.1 `cargo test` passes for the autocoder crate.
- [x] 4.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [x] 4.3 `openspec validate a004-security-findings-block --strict` passes.
