# Implementation tasks

## 1. Prompt rule

- [ ] 1.1 In the reviewer prompt (`prompts/code-review-default.md` and any role override path), add an explicit severity rule: credential/secret leakage, secret exposure, AND injection vulnerabilities are `Block`-class findings — never `Concerns` or `Pass`. Keep the rest of the verdict guidance intact.

## 2. Code-level escalation safety net

- [ ] 2.1 In the verdict-handling path (`code_reviewer.rs`), detect when the reviewer's structured output marks a finding as a secret/credential/key exposure (or injection). Use the reviewer's own finding signal/classification, not a substring scan of the prose.
- [ ] 2.2 When such a finding is present AND the returned verdict is not `Block`, escalate the effective verdict to `Block` before the PR-draft / auto-revise handling runs.
- [ ] 2.3 Leave non-security findings' verdicts untouched.

## 3. Tests

- [ ] 3.1 A reviewer result carrying a credential/secret-leak finding with a `Concerns` (or `Pass`) verdict yields an effective `Block` (assert the verdict).
- [ ] 3.2 An injection finding with a non-`Block` verdict escalates to `Block`.
- [ ] 3.3 A `Concerns` verdict with only non-security findings stays `Concerns` (no escalation).
- [ ] 3.4 The escalation is driven by the structured finding signal, not by message wording (assert behavior under a synthetic finding, not a copy match).

## 4. Acceptance gate

- [ ] 4.1 `cargo test` passes for the autocoder crate.
- [ ] 4.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [ ] 4.3 `openspec validate a004-security-findings-block --strict` passes.
