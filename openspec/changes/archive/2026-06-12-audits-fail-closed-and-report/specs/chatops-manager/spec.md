## ADDED Requirements

### Requirement: On-demand audit completion notification
When an operator triggers an audit on demand (the `audit` verb), the daemon SHALL post a single terminal completion notification to the thread the request originated from once the audit reaches a terminal outcome. The notification SHALL report which terminal state was reached and SHALL carry the audit's `examined_summary` so the operator sees what the audit looked at — never a bare result. The notification SHALL distinguish at least three terminal states: findings produced, no findings, and did-not-complete (the audit could not run to a verdict). The did-not-complete notification SHALL name the cause so the inability to run is explicit and actionable, never silent. The notification SHALL degrade gracefully via the threaded-notification path the audit-findings notifications already use, falling back to a non-threaded post when the backend does not support threads.

#### Scenario: Findings produced
- **WHEN** an operator-triggered audit completes having produced one or more proposals or findings
- **THEN** the daemon posts a completion notification to the originating thread reporting the findings AND the examined summary

#### Scenario: No findings, with evidence of the survey
- **WHEN** an operator-triggered audit completes having positively declared zero findings
- **THEN** the daemon posts a completion notification to the originating thread stating no findings were produced AND including the examined summary (what the audit looked at)
- **AND** the notification is NOT suppressed merely because the result is clean (the operator explicitly asked for this run)

#### Scenario: Did-not-complete names the cause
- **WHEN** an operator-triggered audit reaches a did-not-complete outcome
- **THEN** the daemon posts a completion notification to the originating thread stating the audit did NOT complete AND naming the cause (e.g. the session errored, produced no terminal verdict, or found an issue it could not persist)
- **AND** the notification does NOT report the run as a clean / no-findings result

#### Scenario: Threaded delivery degrades gracefully
- **WHEN** a completion notification is posted AND the configured chatops backend does not support threaded replies
- **THEN** the notification is delivered as a non-threaded post rather than dropped
