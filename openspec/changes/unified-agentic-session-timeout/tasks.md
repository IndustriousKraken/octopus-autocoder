# Tasks

## 1. Config: one timeout field

- [ ] 1.1 Add `executor.agentic_session_timeout_secs: u64` to the executor config, defaulting to 3600 when unset (serde default). Document it in CONFIG.md as the single timeout for the verifier gates, reviewer, and revision sessions (implementer keeps `timeout_secs`).

## 2. Route every auxiliary role to the one value

- [ ] 2.1 Remove the hardcoded constants and resolve the timeout from config in each role: `code_reviewer.rs` (`AGENTIC_REVIEW_TIMEOUT`), `preflight/change_contradiction.rs` (`AGENTIC_CONTRADICTION_TIMEOUT`), `preflight/corpus_check.rs` (`CORPUS_CHECK_TIMEOUT`), `code_implements_spec.rs` (`AGENTIC_CODE_IMPLEMENTS_SPEC_TIMEOUT`), `polling/revision_session.rs` (`REVISION_SESSION_TIMEOUT`).
- [ ] 2.2 Thread the resolved value from the executor config to each call site (the roles already receive executor/config context, or take the timeout as a parameter). There SHALL be exactly one place the default literal `3600` appears.

## 3. Fail-closed on timeout

- [ ] 3.1 Confirm a timed-out auxiliary session enters its failed-to-run state (blocking gate holds; advisory gate/reviewer renders failed-to-run), never a pass, and the diagnostic names the timeout AND the resolved value (this is existing fail-closed behavior; verify it still holds with the configurable value).

## 4. Tests

- [ ] 4.1 Unset config → every auxiliary role resolves 3600.
- [ ] 4.2 A configured value is used by the gates, the reviewer, AND the revision sessions (assert each call site uses the resolved value, e.g. via the config-to-call-site wiring, not a literal).
- [ ] 4.3 No role retains a private timeout literal (the constants are gone).
