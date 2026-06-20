## ADDED Requirements

### Requirement: One configurable timeout for gate, review, and revision sessions
The wall-clock timeout for every auxiliary agentic session — the verifier gates (`[in]`, `[canon]`, `[rules]`, `[out]`), the code reviewer, AND the spec-revision sessions (the `send it` executor AND the revision advisor) — SHALL be resolved from a SINGLE configurable value, NOT from a per-role hardcoded constant. The value SHALL be `executor.agentic_session_timeout_secs`, defaulting to `3600` (one hour) when unset, AND overridable in config. There SHALL be exactly one source of this timeout; no auxiliary-session role embeds its own timeout literal.

The implementer is OUT of scope: it retains its existing `executor.timeout_secs` (and the derived spec-implicit threshold). This requirement governs the auxiliary agentic sessions that previously each carried their own fixed constant.

The default exists because these sessions do real work whose size varies — reading a large diff, judging a wide spec delta, rewriting substantial code — AND a short fixed cap guillotines a legitimately long task. An operator whose repositories run large refactors or spec rewrites raises the one value rather than hunting per-role constants.

This timeout's exhaustion is a session failure, surfaced per the gatekeepers-fail-closed standard: a gate/reviewer/revision session that times out enters its failed-to-run state (it does NOT pass), naming the timeout AND the resolved value in its diagnostic.

#### Scenario: Unset uses the one-hour default
- **WHEN** `executor.agentic_session_timeout_secs` is unset
- **THEN** every gate, reviewer, AND revision session uses a 3600-second timeout
- **AND** no auxiliary-session role applies a different, role-private timeout value

#### Scenario: The configured value governs every auxiliary session
- **WHEN** `executor.agentic_session_timeout_secs` is set to a value
- **THEN** the verifier gates, the reviewer, AND the revision sessions all use that value
- **AND** changing it in one place changes the timeout for all of them

#### Scenario: A timed-out session fails to run, never passes
- **WHEN** an auxiliary agentic session exceeds the resolved timeout
- **THEN** the session enters its failed-to-run state per the gatekeepers-fail-closed standard (a blocking gate holds; an advisory gate/reviewer renders an explicit failed-to-run result), never a pass
- **AND** the diagnostic names the timeout AND the resolved value
