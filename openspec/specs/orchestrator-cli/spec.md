# orchestrator-cli Specification

## Purpose
TBD - created by archiving change orchestrator-architecture. Update Purpose after archive.
## Requirements
### Requirement: Daemon entry point
The orchestrator SHALL provide a `run` subcommand that loads a YAML configuration file and starts an asynchronous polling loop for each configured repository, terminating only on signal (SIGINT/SIGTERM) or fatal initialization error.

#### Scenario: Normal startup
- **WHEN** the user executes `orchestrator run --config <path>`
- **THEN** the process loads the config from `<path>`, validates that each configured repository's workspace can be initialized (clone or pull succeeds), and spawns one tokio task per repository
- **AND** the process emits one log line per repository at startup naming the repository URL and configured poll interval
- **AND** the process does not exit until SIGINT or SIGTERM is received OR every spawned polling task has terminated

#### Scenario: Missing or malformed config
- **WHEN** the user executes `orchestrator run --config <path>` with a path that does not exist or whose contents fail YAML parsing
- **THEN** the process exits with a non-zero status code within 5 seconds of invocation
- **AND** stderr contains a single error line naming the offending file path and the underlying parse or I/O error

#### Scenario: Dirty workspace at startup
- **WHEN** the orchestrator initializes a per-repo workspace and `git status --porcelain` inside that workspace returns a non-empty result
- **THEN** the orchestrator emits an error log naming the workspace path and the dirty file count
- **AND** the polling loop for that repository is skipped for the remainder of the process lifetime
- **AND** other configured repositories continue to be serviced

### Requirement: Rewind subcommand
The orchestrator SHALL provide a `rewind` subcommand that recovers from a failed PR or bad implementation by unarchiving specified changes and resetting the relevant agent branch.

#### Scenario: Rewinding a single change
- **WHEN** the user executes `orchestrator rewind <change_name> --config <path>`
- **THEN** the process locates the most recent archived directory inside the configured workspace whose name matches `^\d{4}-\d{2}-\d{2}-<change_name>$`, moves it from `openspec/changes/archive/` back to `openspec/changes/<change_name>/`, and resets the configured `agent_branch` to the configured `base_branch`
- **AND** if no matching archived directory is found, the process exits non-zero and stderr names the missing change

#### Scenario: Hard rewind deletes the agent branch
- **WHEN** the user passes `--hard` to the rewind subcommand
- **THEN** the process force-deletes the configured `agent_branch` both locally (`git branch -D`) and on the remote (`git push origin --delete`) before unarchiving

#### Scenario: Soft rewind requires confirmation
- **WHEN** the user invokes rewind WITHOUT `--hard` AND the configured agent branch exists locally or remotely
- **THEN** the process prompts the user on stdin for explicit confirmation before any branch deletion or unarchive operation
- **AND** if the user declines (any input other than `y` or `Y`), the process exits with status 0 and the workspace is left untouched

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

