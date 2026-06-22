## ADDED Requirements

### Requirement: Operator-facing failure notifications surface the assembled outcome reason
An operator-facing notification that reports an executor session failure SHALL surface the assembled outcome reason (defined in the executor capability's "Backend-agnostic execution contract") — the captured agent final message, standard-error, and/or exit status or terminating signal, truncated at the source — rather than only a bare exit code. The notification SHALL render the reason as produced at the source; it SHALL NOT re-summarize, re-derive, or discard it. This applies uniformly to executor-session failures regardless of lane (implementation pass, PR revision, audit, or agentic gate), so an operator can tell a transient infrastructure condition from a real failure without opening server-side logs.

A failure that the orchestrator is still retrying within its bounded retry (see orchestrator-cli) SHALL be presented as distinct from a terminal, retries-exhausted failure, so a transient-and-recovering condition is not mistaken for a final failure.

#### Scenario: A failure notification carries the assembled reason
- **WHEN** the daemon posts an operator-facing notification for a failed executor session whose assembled reason carries captured output (e.g. an upstream-API overload message in the final answer, or a panic trace on standard-error)
- **THEN** the notification includes that assembled reason (as truncated at the source)
- **AND** it does NOT replace it with, or reduce it to, only the bare exit code

#### Scenario: An empty-output failure still names the exit status or signal
- **WHEN** the failed session captured no final message and no standard-error (the assembled reason is the exit status or terminating signal)
- **THEN** the notification surfaces that exit status or signal rather than an empty or generic message

#### Scenario: A retrying failure is distinguishable from a terminal one
- **WHEN** a session has failed but the orchestrator is retrying it within the bounded retry
- **THEN** the operator-facing surface distinguishes the retry-in-progress state from a terminal, retries-exhausted failure
