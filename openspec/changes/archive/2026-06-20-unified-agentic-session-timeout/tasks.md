# Tasks

## 1. Config: one timeout field

- [x] 1.1 Add `executor.agentic_session_timeout_secs: u64` to the executor config, defaulting to 3600 when unset (serde default). Document it in CONFIG.md as the single timeout for the verifier gates, reviewer, and revision sessions (implementer keeps `timeout_secs`).

## 2. Route every auxiliary role to the one value

- [x] 2.1 Remove the hardcoded constants and resolve the timeout from config at every call site. Note `CORPUS_CHECK_TIMEOUT` is shared by THREE gate call sites, so all three must be re-pointed: `code_reviewer.rs` (`AGENTIC_REVIEW_TIMEOUT`); `preflight/change_contradiction.rs` (`AGENTIC_CONTRADICTION_TIMEOUT`, the `[in]` gate); `code_implements_spec.rs` (`AGENTIC_CODE_IMPLEMENTS_SPEC_TIMEOUT`, the `[out]` gate); `polling/revision_session.rs` (`REVISION_SESSION_TIMEOUT`); AND the `CORPUS_CHECK_TIMEOUT` consumers — `preflight/corpus_check.rs` (definition), `preflight/canon_contradiction.rs` (the `[canon]` gate, two call sites), AND `preflight/global_rules.rs` (the `[rules]` gate). After this, no call site references a removed constant.
- [x] 2.2 Thread the resolved value from the executor config to each call site (the roles already receive executor/config context, or take the timeout as a parameter). There SHALL be exactly one place the default literal `3600` appears.

## 3. Fail-closed on timeout

- [x] 3.1 Confirm a timed-out auxiliary session enters its failed-to-run state (blocking gate holds; advisory gate/reviewer renders failed-to-run), never a pass, and the diagnostic names the timeout AND the resolved value (this is existing fail-closed behavior; verify it still holds with the configurable value).

## 4. Tests

- [x] 4.1 Unset config → every auxiliary role resolves 3600.
- [x] 4.2 A configured value is used by the gates, the reviewer, AND the revision sessions (assert each call site uses the resolved value, e.g. via the config-to-call-site wiring, not a literal).
- [x] 4.3 No role retains a private timeout literal (the constants are gone).
