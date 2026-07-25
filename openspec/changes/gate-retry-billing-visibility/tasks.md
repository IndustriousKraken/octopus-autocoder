# Tasks: gate-retry-billing-visibility

## 1. Surface attempts-used from the retry wrapper

- [ ] 1.1 In `autocoder/src/verifier_gate.rs`, extend `run_session_with_retry`'s result to carry the number of attempts consumed (and the allowed budget) alongside the existing outcome, without changing the retry semantics: only no-submission completions are re-attempted; errors and timeouts still short-circuit to the fail-closed hold.

## 2. Notify on retried invocations

- [ ] 2.1 At the daemon gate call sites (the three pre-executor gates and the post-executor verdict gate), when an invocation consumed at least one re-attempt, post ONE chatops notification through the standard outbound notification path containing: the gate identifier, the change slug, attempts used vs. allowed, the final disposition (clean, findings, or held), and the model-attribution line. One message per gate invocation, never per attempt; no additional throttle.
- [ ] 2.2 In daemon-absent contexts (`autocoder verify`, standalone gate runs), emit the same fields as a single WARN log line instead of a chatops post.

## 3. Tests

- [ ] 3.1 Test: a no-submission first attempt followed by a clean second attempt yields attempts-used = 2 and triggers exactly one notification carrying gate id, slug, attempt counts, and disposition.
- [ ] 3.2 Test: a session error or timeout consumes one attempt, is not re-attempted, and triggers no retry notification.
- [ ] 3.3 Test: `executor.verifier_gate_retries: 0` produces a single attempt and no retry notification.

## 4. Documentation

- [ ] 4.1 Document `executor.verifier_gate_retries` (default, semantics, billing implication, and the retry notification) in `docs/CONFIG.md` and `config.example.yaml`.

## 5. Validation

- [ ] 5.1 Run `openspec validate gate-retry-billing-visibility --strict` and fix any findings.
