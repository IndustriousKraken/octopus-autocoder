## ADDED Requirements

### Requirement: Per-repository asynchronous polling loop
The orchestrator SHALL implement the per-repository polling task referenced in `orchestrator-architecture/specs/orchestrator-cli/spec.md` as a sleep-then-iterate cycle that runs the architecture's single-pass workflow on every iteration.

#### Scenario: Spawn count matches config
- **WHEN** the daemon starts with a config containing N repositories AND the workspace collision check passes
- **THEN** exactly N polling tasks are spawned via `tokio::task::JoinSet`
- **AND** each task owns its own workspace path (no two tasks share a path; collision detection at startup enforces non-overlap)

#### Scenario: Normal iteration
- **WHEN** a polling task wakes (start of process or end of previous sleep)
- **THEN** it runs the full single-pass workflow for its repository: workspace init → stale-lock cleanup → dirty-workspace refusal → branch recreation → queue walk → push and PR creation if any commits were produced
- **AND** the task then sleeps for `poll_interval_sec` before iterating again
- **AND** no two iterations within the same task overlap

#### Scenario: Iteration runtime exceeds poll interval
- **WHEN** an iteration's wall-clock runtime exceeds `poll_interval_sec`
- **THEN** the next iteration begins immediately after the current one finishes
- **AND** no negative sleep is attempted; no two iterations within the same task run in parallel

### Requirement: Iteration-level error tolerance
The polling loop SHALL continue running after a failed iteration; a single iteration's error MUST NOT terminate the task or affect other repositories.

#### Scenario: Iteration fails
- **WHEN** any error occurs during a polling iteration (workspace init, git operation, executor failure, PR creation)
- **THEN** the task emits a log line of the form `"polling iteration failed for <url>: <error chain>"` naming the failed step
- **AND** the task sleeps for `poll_interval_sec` and proceeds to the next iteration
- **AND** other repositories' polling tasks are unaffected (their iterations continue on schedule)

### Requirement: Graceful shutdown on signal
The orchestrator SHALL respond to SIGINT or SIGTERM by cancelling all polling tasks; each task completes its current iteration (if any) and exits cleanly.

#### Scenario: Signal during inter-iteration sleep
- **WHEN** SIGINT or SIGTERM arrives while every polling task is sleeping
- **THEN** every task exits its sleep within 200 ms (verified in tests via the `CancellationToken` selecting against the sleep) and does not begin another iteration
- **AND** the main process exits within 30 seconds total

#### Scenario: Signal during iteration
- **WHEN** SIGINT or SIGTERM arrives while a polling iteration is in progress
- **THEN** the in-flight iteration runs to completion (mid-iteration cancellation is NOT performed); the task then observes the cancellation token and exits without sleeping or starting another iteration
- **AND** any child processes spawned by the iteration receive their normal lifecycle (the executor's child process completes or hits its own `executor.timeout_secs`)

### Requirement: Startup logging per repository
The orchestrator SHALL emit a startup log line per configured repository naming its URL, derived (or explicit) workspace path, and configured `poll_interval_sec`.

#### Scenario: Startup line emitted
- **WHEN** the daemon starts AND the workspace collision check passes
- **THEN** before any polling task begins iterating, the orchestrator emits one log line per repository containing the literal URL, the resolved workspace path, and the integer `poll_interval_sec`
