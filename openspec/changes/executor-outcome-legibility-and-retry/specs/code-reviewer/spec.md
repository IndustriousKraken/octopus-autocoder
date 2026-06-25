## ADDED Requirements

### Requirement: Reviewer session output is persisted and surfaced on a no-submission discard
The agentic reviewer SHALL retain and surface its captured session output on a no-submission discard, AND SHALL persist that output to a discoverable log, rather than dropping it. Today the reviewer runs in capture mode (which writes no streaming log) and discards the session's captured output on a no-submission discard, so the discard (per the executor capability's "a reviewer session that ends with no stored submission … discards the review AND alerts the operator" rule) surfaces only a bare "recorded no valid `submit_review` submission" with no recoverable diagnostic. This applies the same surface-the-captured-evidence principle as the executor failure reason.

On a reviewer session that ends with no valid `submit_review` submission (a discard outcome), the daemon SHALL include in the discard reason the session's captured evidence — the agent's final message (if non-empty) and captured standard-error (if non-empty), assembled in priority order and each truncated to a bounded budget, surfaced RAW without parsing or classification — so the operator can tell WHY the session failed to submit (an upstream-API message such as an overload notice, prose emitted instead of a tool call, a schema-rejected submission, etc.). When both are empty the reason SHALL surface the session's exit status or terminating signal. A session that timed out SHALL retain its distinct timeout reason rather than this assembled reason.

The reviewer SHALL ALSO persist each session's captured output to a discoverable per-session log file under the run-logs directory, mirroring the audit logs' `audits/<type>-<timestamp>.log` pattern, regardless of outcome — so the full output is recoverable from disk when the surfaced reason is truncated. The surfaced reason, when truncated, SHALL name that log-file path. This is provider-agnostic: it surfaces and persists raw captured output, never parsing it for any decision.

#### Scenario: A no-submission discard surfaces the captured session output
- **WHEN** a reviewer session ends with no valid `submit_review` submission AND its captured output (final message and/or standard-error) is non-empty
- **THEN** the discard reason includes that captured output, truncated to a bounded budget, rather than only the bare "recorded no valid `submit_review` submission" text
- **AND** the reviewer's discard-not-approve behavior is otherwise unchanged (the review is still discarded and the operator alerted; it is NOT treated as an implicit approve)

#### Scenario: An empty-output no-submission discard surfaces the exit status or signal
- **WHEN** a reviewer session ends with no valid submission AND captured neither a final message nor standard-error
- **THEN** the discard reason surfaces the session's exit status or terminating signal, so an empty-output failure is still legible rather than blank
- **AND** a session that TIMED OUT instead reports its distinct timeout reason

#### Scenario: The reviewer session writes a discoverable log
- **WHEN** a reviewer session runs to any terminal outcome (a recorded submission, a no-submission discard, or a timeout)
- **THEN** its captured output is persisted to a per-session log file under the run-logs directory, mirroring the audit-log file pattern, so an operator can open it without re-running the review
- **AND** when the surfaced discard reason is truncated, it names that log-file path
