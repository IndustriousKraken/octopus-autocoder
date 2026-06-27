# Tasks

## 1. Refine the contradiction-marker definition

- [x] 1.1 Treat a `.needs-spec-revision.json` as a CONTRADICTION marker (the `RevisionThreadState`-recording, `send it`-able path) ONLY when `unimplementable_tasks` is empty AND `gate_error` is empty AND `unarchivable_deltas` is empty. A marker with a non-empty `unarchivable_deltas` array is a MANUAL-FIX hold, not a contradiction — it records NO `RevisionThreadState` (this aligns code with `alerts_throttle.rs:172-174`).

## 2. Explain the manual fix in the alert body

- [x] 2.1 When the spec-delta archivability pre-flight posts the `AlertCategory::SpecNeedsRevision` alert for an UNARCHIVABLE-DELTAS marker, the alert body SHALL state that the change is held for unarchivable spec deltas, that `@<bot> send it` cannot revise it, AND that the operator should fix the delta header(s) to match canonical and then post `@<bot> clear-revision`.
- [x] 2.2 When the pipeline posts the alert for a GATE-ERROR hold (the `gate_error` population), the alert body SHALL state that the change is held because a verifier gate could not run, that `@<bot> send it` cannot revise it, AND that the operator should fix the gate and then post `@<bot> clear-revision`.
- [x] 2.3 The CONTRADICTION alert body is unchanged (it still advertises discussing the revision and `@<bot> send it`). Neither manual-fix alert records a `RevisionThreadState`.

## 3. Leave `send it` routing alone

- [x] 3.1 Make NO change to the inbound `send it` dispatcher or the generic untracked-thread refusal text. A `send it` reply in a manual-fix thread matches no recorded state and falls through to the existing generic refusal — the operator was already told the manual remediation in the alert body.

## 4. Tests

- [x] 4.1 Posting an unarchivable-deltas alert records NO `RevisionThreadState`, and the alert body names the unarchivable-deltas cause AND the `clear-revision` remediation (assert on behavior/content, not a brittle full-string match).
- [x] 4.2 Posting a gate-error alert records NO `RevisionThreadState`, and the alert body names the gate-error cause AND the `clear-revision` remediation.
- [x] 4.3 Regression: a CONTRADICTION marker (empty `unimplementable_tasks` AND empty `gate_error` AND empty `unarchivable_deltas`) still records a `RevisionThreadState` and its alert still advertises `@<bot> send it`.
- [x] 4.4 Regression: `send it` in a tracked CONTRADICTION revision thread still runs the spec-revision executor; `send it` in a manual-fix thread still produces the existing generic untracked-thread refusal (routing unchanged).
