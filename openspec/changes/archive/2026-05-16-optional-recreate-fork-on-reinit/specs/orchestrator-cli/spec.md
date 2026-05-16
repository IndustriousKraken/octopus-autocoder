## ADDED Requirements

### Requirement: github.recreate_fork_on_reinit config field
The `github:` config block SHALL accept an optional `recreate_fork_on_reinit: bool` field that defaults to `false` when unset. When `true`, the workspace manager applies the destructive re-fork behavior described in `workspace-manager`'s "Optional fork recreation on workspace reinitialization" requirement.

#### Scenario: Field defaults to false when absent
- **WHEN** the operator's `github:` block does NOT include a
  `recreate_fork_on_reinit` key
- **THEN** the effective value is `false` AND the conservative
  fetch-fork-at-init behavior applies on fresh clones

#### Scenario: Field is global, not per-repo
- **WHEN** the operator sets `github.recreate_fork_on_reinit: true`
- **THEN** the flag applies to every configured repository in this
  daemon process AND there is no per-repo override
- **AND** the rationale is that `github.fork_owner` is itself global
  (all repos in one autocoder process share the same fork owner),
  so re-fork policy follows the same scope

#### Scenario: Field requires fork-PR mode to have any effect
- **WHEN** `recreate_fork_on_reinit: true` AND `github.fork_owner`
  is unset (direct-push mode)
- **THEN** config load succeeds without error (the field is not
  invalid; it's just inactive)
- **AND** the daemon emits an INFO log at startup noting that
  `recreate_fork_on_reinit: true` has no effect when fork mode is off
- **AND** no re-fork attempts are made at runtime
