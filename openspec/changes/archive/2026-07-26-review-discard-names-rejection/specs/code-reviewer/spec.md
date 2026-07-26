# code-reviewer delta: review-discard-names-rejection

## MODIFIED Requirements

### Requirement: Reviewer session output is persisted and surfaced on a no-submission discard
The agentic reviewer SHALL retain and surface its captured session output on a no-submission discard, AND SHALL persist that output to a discoverable log, rather than dropping it. Today the reviewer runs in capture mode (which writes no streaming log) and discards the session's captured output on a no-submission discard, so the discard (per the executor capability's "a reviewer session that ends with no stored submission … discards the review AND alerts the operator" rule) surfaces only a bare "recorded no valid `submit_review` submission" with no recoverable diagnostic. This applies the same surface-the-captured-evidence principle as the executor failure reason.

On a reviewer session that ends with no valid `submit_review` submission (a discard outcome), the daemon SHALL include in the discard reason the session's captured evidence — the agent's final message (if non-empty) and captured standard-error (if non-empty), assembled in priority order and each truncated to a bounded budget, surfaced RAW without parsing or classification — so the operator can tell WHY the session failed to submit (an upstream-API message such as an overload notice, prose emitted instead of a tool call, a schema-rejected submission, etc.). When both are empty the reason SHALL surface the session's exit status or terminating signal. A session that timed out SHALL retain its distinct timeout reason rather than this assembled reason.

The discard reason SHALL additionally distinguish the two no-submission modes by what the daemon itself observed during the session. When one or more `submit_review` attempts were REJECTED by the daemon-side validator, the reason SHALL lead with the LAST rejection — its validator reason, truncated — and the rejection count (e.g. `submission rejected 2x — last: verdict "Concerns" not in [Approve, Block]`), ahead of the raw captured output; the daemon already computed that reason when it returned the correctable tool error, so the discard surfaces it rather than discarding it. When NO submission was attempted at all, the reason SHALL state that explicitly (e.g. `no submission attempted`). Retained rejections are per-session: they are drained with the session's submission consume and cleared when a valid submission is recorded, and each rejection is recorded, with its timestamp, in the per-session review log.

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

#### Scenario: A rejected-never-corrected discard names the rejection
- **WHEN** a reviewer session's `submit_review` attempts were rejected by the daemon-side validator AND the session ends with no valid submission
- **THEN** the discard reason leads with the last rejection's validator reason (truncated) and the rejection count, ahead of the raw captured output
- **AND** each rejection appears, with its timestamp, in the per-session review log

#### Scenario: A never-attempted discard says so
- **WHEN** a reviewer session ends with no valid submission AND no `submit_review` call was attempted at any point
- **THEN** the discard reason states explicitly that no submission was attempted, so the operator can tell a prompt-contract failure from a rejected-payload failure

#### Scenario: A corrected rejection leaves no residue
- **WHEN** a `submit_review` attempt is rejected AND a later attempt in the same session records a valid submission
- **THEN** the review completes normally with no rejection text in any surfaced output
- **AND** no rejection state carries over into any later session
