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

