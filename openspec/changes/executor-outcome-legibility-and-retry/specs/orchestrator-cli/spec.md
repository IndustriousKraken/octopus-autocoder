## ADDED Requirements

### Requirement: Executor sessions with no committable result are bounded-retried with backoff
The orchestrator SHALL retry an executor session that FAILED and produced no committable result, up to `executor.session_retries` additional attempts with backoff between attempts, before surfacing the failure. This is provider-agnostic: it does NOT classify or parse the failure to decide — a transient failure clears on a retry, while a deterministic failure recurs through all attempts and is then surfaced with its assembled reason.

The "no committable result" guard SHALL reuse the existing success signal — a clean working tree (`git status --porcelain` empty, the same signal that maps a no-diff `Completed` to `Failed`). A session that DID produce a committable result (a non-empty diff the flow could open a PR from) SHALL NOT be retried, even on a failed outcome; it is surfaced with whatever it produced. The orchestrator SHALL consult the strategy's optional `CliStrategy::is_retryable(&outcome)` hint: `Some(false)` SHALL short-circuit (no retry); `Some(true)` SHALL retry even when a committable result exists — UNLESS `executor.session_retries` is `0`, in which case the hint is suppressed and no retry occurs; `None` SHALL apply the no-committable-result rule above.

`executor.session_retries` SHALL be an attempt-count configuration value (additional whole-session re-invocations beyond the first; small default; `0` disables retry entirely, including suppressing any `Some(true)` strategy hint — `session_retries` is the absolute bound on total additional attempts). It is distinct from `executor.timeout_secs` (a per-attempt duration) and from the gate-scoped verifier-gate retries (which cover a different, no-submission case). A retry-in-progress SHALL be observably distinct from a terminal failure in the operator-facing surface. This bounded in-pass retry SHALL NOT alter the daemon's separate next-pass re-pickup scheduling.

#### Scenario: A no-result failure is retried up to the bound
- **WHEN** an executor session returns a failed outcome AND left no committable result (clean working tree) AND `executor.session_retries` is a positive N
- **THEN** the orchestrator re-invokes the session up to N additional times, with backoff between attempts
- **AND** if an attempt produces a committable result the retry loop stops and that result is used
- **AND** if all attempts are exhausted the failure is surfaced with its assembled reason

#### Scenario: A failure that produced committable work is not retried
- **WHEN** an executor session returns a failed outcome BUT left a committable result (non-empty working-tree diff) AND `CliStrategy::is_retryable` returns `None` or `Some(false)`
- **THEN** the orchestrator does NOT re-invoke the session
- **AND** the outcome is handled with the work it produced (not blindly re-run)

#### Scenario: Retry is disabled when the bound is zero
- **WHEN** `executor.session_retries` is `0` AND a session fails with no committable result
- **THEN** the orchestrator does not retry and surfaces the failure immediately with its assembled reason

#### Scenario: The strategy retry hint overrides the default rule
- **WHEN** the resolved `CliStrategy::is_retryable(&outcome)` returns `Some(false)` for a failed no-result session
- **THEN** the orchestrator does NOT retry (the strategy has declared the failure non-retryable)
- **WHEN** it returns `Some(true)` for a failed session that DID produce a committable result AND `executor.session_retries` is positive
- **THEN** the orchestrator retries (the strategy has overridden the committable-result guard; retries remain subject to the `session_retries` count)

### Requirement: `executor.session_retries` bounds whole-session retries
The configuration SHALL expose `executor.session_retries`, an unsigned attempt count of additional whole-session re-invocations the orchestrator may perform on a no-committable-result failure (see "Executor sessions with no committable result are bounded-retried with backoff"). It SHALL have a small positive default when absent and SHALL accept `0` to disable retry. It SHALL NOT be conflated with `executor.timeout_secs` (a duration) or the verifier-gate retry count (a different scope).

#### Scenario: Default applied when unset
- **WHEN** the configuration omits `executor.session_retries`
- **THEN** the resolved value is the small positive default (retry is enabled with a bounded count)

#### Scenario: Explicit zero disables retry
- **WHEN** the configuration sets `executor.session_retries: 0`
- **THEN** no whole-session retry is performed for any reason — including a failed session that a `Some(true)` strategy hint would otherwise cause to retry
