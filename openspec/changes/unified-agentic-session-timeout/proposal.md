# One configurable timeout for gate, review, and revision sessions

## Why

Five auxiliary agentic roles each carry their own hardcoded 900-second (15-minute)
timeout: the reviewer, the `[in]`/`[canon]`/`[rules]` gates, the `[out]` gate, and
the spec-revision executor + advisor. The number is decided by no one, tunable by
no one, and duplicated five times. Worse, a fixed 15-minute cap guillotines any
session whose work is genuinely large — reading a 50,000-line refactor's diff,
rewriting thousands of lines of spec — failing it not because anything is wrong
but because the wall clock ran out. The implementer already resolves its timeout
from config (`executor.timeout_secs`); the auxiliary roles never adopted that
pattern.

## What Changes

- A new `orchestrator-cli` requirement: the timeout for the verifier gates, the
  reviewer, and the revision sessions is resolved from ONE config value,
  `executor.agentic_session_timeout_secs`, defaulting to 3600 (one hour) and
  overridable. No auxiliary-session role embeds its own timeout literal.
- The five hardcoded `900s` constants (`AGENTIC_REVIEW_TIMEOUT`,
  `AGENTIC_CONTRADICTION_TIMEOUT`, `CORPUS_CHECK_TIMEOUT`,
  `AGENTIC_CODE_IMPLEMENTS_SPEC_TIMEOUT`, `REVISION_SESSION_TIMEOUT`) are removed
  in favor of the resolved value.
- A timed-out session is a session failure surfaced per gatekeepers-fail-closed —
  it never becomes a pass — and its diagnostic names the timeout and the resolved
  value.

## Impact

- Affected specs: `orchestrator-cli` (ADD the single-timeout requirement). Canon
  specifies no timeout values today, so there is no existing requirement to MODIFY.
- Affected code: add `executor.agentic_session_timeout_secs` (default 3600) to the
  executor config; route `code_reviewer.rs`, `change_contradiction.rs`,
  `corpus_check.rs`, `code_implements_spec.rs`, and `polling/revision_session.rs`
  to read it instead of their local constants; update CONFIG.md.
- The implementer is unchanged (keeps `executor.timeout_secs`). This is the
  configurable-base form; a progress/inactivity-based timeout that scales with
  work automatically is a possible later refinement, not part of this change.
