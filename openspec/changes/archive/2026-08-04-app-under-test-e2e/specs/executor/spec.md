# executor delta: app-under-test-e2e

## ADDED Requirements

### Requirement: Implementer prompt includes an "Application under test" block when an application is running
When the daemon has started an application for the pass and it became ready, the implementer prompt SHALL include an "Application under test" block naming the resolved base URL, the configured end-to-end command, and the instruction that behavioral scenarios are verified by an executed end-to-end test rather than asserted in prose.

The block SHALL state that the end-to-end command's exit code is the authoritative verification signal, and that screenshots or other captured artifacts are debugging aids for the agent's own loop, never the pass/fail signal. This keeps the contract identical across every `CliStrategy`, whose image-reading abilities differ.

When no application is running for the pass, the block SHALL be omitted entirely and the prompt SHALL render exactly as it did before this change. The block follows the same conditional-inclusion pattern as the existing "Prior iteration summary" block.

#### Scenario: Block present when an application is running
- **WHEN** the daemon started the application for the pass AND the readiness probe succeeded
- **THEN** the rendered implementer prompt contains the "Application under test" block naming the resolved base URL AND the end-to-end command
- **AND** the block states that the end-to-end command's exit code is the authoritative signal

#### Scenario: Block omitted when no application is running
- **WHEN** the repository declares no `app_under_test`, OR its toolchain is unavailable, OR readiness failed
- **THEN** the rendered prompt contains no "Application under test" block
- **AND** the prompt is byte-identical to the pre-change rendering for the same inputs

#### Scenario: Block is CLI-agnostic
- **WHEN** the implementer runs through any registered `CliStrategy`
- **THEN** the same block content is rendered
- **AND** no part of the block's contract depends on the wrapped CLI's ability to read images

#### Scenario: Agent restart of an established application is reaped with the session
- **WHEN** the daemon has already established the application for the pass AND the agent restarts it during its own session using the configured start command
- **THEN** the restarted process is inside the session's process group
- **AND** it is terminated when the session's process group is terminated
- **AND** the daemon retains responsibility for the pass's port allocation, readiness, and teardown
