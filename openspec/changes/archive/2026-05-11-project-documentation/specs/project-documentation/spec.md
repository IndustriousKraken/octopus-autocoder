## ADDED Requirements

### Requirement: Implementing agents update user-facing documentation
Agents that implement OpenSpec changes SHALL update `README.md` and any relevant `docs/` files when their change affects user-visible behavior — CLI commands, configuration keys, deployment steps, public APIs, environment variables, or architectural shifts that the operator must understand to run or maintain the system.

#### Scenario: User-facing change includes documentation update
- **WHEN** an implementing agent's change adds, modifies, or removes a user-visible feature, configuration option, CLI argument, or operational step
- **THEN** the agent's commit MUST also include corresponding edits to `README.md` AND/OR the relevant files under `docs/` so the documentation accurately reflects the new behavior
- **AND** if the change introduces a feature that is partially-implemented or aspirational, the documentation MUST mark that feature as such (e.g. with a "Status: aspirational" or "Planned" note) rather than describing it as fully working

#### Scenario: Internal-only change does not require docs update
- **WHEN** a change is purely internal — refactoring, internal renaming, dependency bumps, test-only changes, build-system adjustments that do not affect user invocation
- **THEN** no documentation update is required
- **AND** the agent SHOULD note the internal-only scope in the commit message so reviewers can confirm the assessment

#### Scenario: Removing a user-facing feature
- **WHEN** an implementing agent's change removes a user-visible feature
- **THEN** the agent's commit MUST also remove the corresponding documentation, OR mark it as deprecated/removed with a date and rationale
- **AND** README sections describing the removed feature MUST NOT be left in a misleading state suggesting the feature still exists
