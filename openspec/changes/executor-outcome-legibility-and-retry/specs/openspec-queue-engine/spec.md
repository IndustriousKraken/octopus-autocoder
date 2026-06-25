## MODIFIED Requirements

### Requirement: Lock state management
The queue engine SHALL atomically lock and unlock changes via filesystem markers to prevent duplicate execution and to signal in-progress state to humans inspecting the workspace.

#### Scenario: Locking a change
- **WHEN** the orchestrator selects a change for execution
- **THEN** the queue engine creates an empty file at `<workspace>/openspec/changes/<change>/.in-progress` BEFORE invoking the executor
- **AND** the file is verifiable on disk via standard filesystem inspection (e.g. `ls -a`)

#### Scenario: Unlocking after any executor outcome
- **WHEN** the executor returns ANY outcome (`Completed`, `AskUser`, `Failed`) OR the executor invocation panics
- **AND** the orchestrator is NOT executing an in-pass bounded retry (i.e. this is the final outcome of the retry loop, or retry is disabled)
- **THEN** the queue engine deletes the `.in-progress` file
- **AND** the deletion is idempotent (no error if the file is already absent)

#### Scenario: Lock is retained during in-pass bounded retry
- **WHEN** the executor returns `Failed` mid-loop during an in-pass bounded retry (i.e. retry attempts remain)
- **THEN** the queue engine SHALL NOT delete `.in-progress` until the retry loop completes
- **AND** the `.in-progress` file persists across retry attempts to prevent concurrent pickup by other workers
- **AND** only the final outcome of the retry loop (either a non-Failed outcome or all attempts exhausted) triggers the deletion described in "Unlocking after any executor outcome"

#### Scenario: Stale lock cleanup on startup
- **WHEN** the orchestrator initializes a workspace at process startup
- **THEN** any pre-existing `.in-progress` files inside `<workspace>/openspec/changes/<change>/` are deleted before the polling loop for that repository begins
- **AND** a log line is emitted for each lock cleared, naming the change
