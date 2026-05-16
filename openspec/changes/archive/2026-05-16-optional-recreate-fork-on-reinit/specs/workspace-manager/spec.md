## ADDED Requirements

### Requirement: Optional fork recreation on workspace reinitialization
When `github.recreate_fork_on_reinit` is `true` AND fork-PR mode is active AND the workspace manager performs a clone (the workspace path was absent), the manager SHALL delete the existing fork on GitHub, recreate it from upstream, then proceed with the normal post-clone steps. This is destructive: any open PRs against branches on the deleted fork are closed by GitHub automatically. Default is `false`; operators opt in per their tolerance for losing fork-resident state.

#### Scenario: Recreate-on-reinit fires only when both conditions hold
- **WHEN** the workspace manager begins `ensure_initialized` AND the
  workspace path is absent (so a clone will happen) AND `fork_url` is
  `Some` (fork-PR mode is active) AND `recreate_fork_on_reinit` is
  `true`
- **THEN** before adding the `fork` remote, the manager resolves the
  upstream owner, repo name, and operator PAT
- **AND** calls `github::delete_repo(<fork_owner>, <repo_name>, token)`
  to delete the existing fork via `DELETE /repos/{owner}/{repo}`
- **AND** waits up to 5 seconds for the deletion to propagate
- **AND** calls `github::create_fork(<upstream_owner>, <repo_name>,
  token)` to re-fork upstream
- **AND** then proceeds with the existing `ensure_remote` +
  `fetch fork` sequence (the fetch returns empty tracking refs because
  the fork is freshly created)

#### Scenario: Recreate is skipped when workspace already exists
- **WHEN** `recreate_fork_on_reinit` is `true` AND
  `ensure_initialized` is called against an existing workspace (so
  no clone happens)
- **THEN** the re-fork path is NOT triggered
- **AND** the existing re-init-with-existing-workspace behavior runs
  unchanged

#### Scenario: Recreate is skipped when fork-PR mode is off
- **WHEN** `recreate_fork_on_reinit` is `true` BUT `fork_url` is
  `None` (direct-push mode)
- **THEN** the re-fork path is NOT triggered
- **AND** the manager runs the existing direct-push clone path unchanged

#### Scenario: Recreate is skipped when flag is false or unset
- **WHEN** `recreate_fork_on_reinit` is `false` OR unset (the default)
- **THEN** the re-fork path is NEVER triggered regardless of
  workspace state or fork-mode setting
- **AND** the conservative `fetch fork` behavior from
  `fetch-fork-at-workspace-init` applies

#### Scenario: Delete returns 404 is treated as success
- **WHEN** the `github::delete_repo` call returns 404 (the fork was
  already deleted out-of-band, e.g. via the GitHub UI before this
  iteration)
- **THEN** the manager logs INFO "fork already absent; proceeding to
  recreate" AND continues to the `create_fork` step
- **AND** does NOT treat this as an error

#### Scenario: Delete returns 403 (missing scope) falls back to conservative path
- **WHEN** the `github::delete_repo` call returns 403 (operator's
  PAT lacks the `delete_repo` scope)
- **THEN** the manager logs ERROR naming the missing scope AND posts
  a chatops alert (best-effort) telling the operator to add the
  `delete_repo` scope OR set `recreate_fork_on_reinit: false`
- **AND** falls back to the conservative `fetch fork` behavior so
  iteration can still proceed
- **AND** subsequent iterations continue to attempt re-fork on each
  fresh clone (the operator can disable the flag to silence)

#### Scenario: Recreate posts a destructive-action chatops notification
- **WHEN** the re-fork sequence (delete + create) completes
  successfully
- **THEN** autocoder posts a single ChatOps notification with body
  `:warning: <repo>: re-forked at workspace reinitialization
  (previous fork deleted; any open PRs from this fork are now closed)`
- **AND** the post is best-effort: a failure logs at WARN and
  does NOT propagate (the re-fork itself already succeeded)
