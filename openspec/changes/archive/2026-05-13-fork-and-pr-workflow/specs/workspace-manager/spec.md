## MODIFIED Requirements

### Requirement: Idempotent workspace initialization
The workspace manager SHALL ensure a repository is locally cloned before
each polling iteration begins, performing a clone if absent and a fetch
if present, without losing existing local state. When fork-PR mode is
active (`github.fork_owner` is configured), the manager SHALL ALSO
ensure a second remote named `fork` is registered, pointing at the
fork URL derived from the upstream URL and `fork_owner`.

#### Scenario: First-time clone (direct-push mode)
- **WHEN** the polling task begins an iteration AND the workspace path
  does not exist on disk AND `github.fork_owner` is unset
- **THEN** the manager runs `git clone <url> <workspace_path>`
- **AND** the resulting path contains a `.git` directory verifiable via
  filesystem inspection
- **AND** no additional remotes are registered (only `origin` exists)

#### Scenario: First-time clone (fork-PR mode)
- **WHEN** the polling task begins an iteration AND the workspace path
  does not exist on disk AND `github.fork_owner` is set
- **THEN** the manager runs `git clone <upstream-url> <workspace_path>`
- **AND** the manager then runs `git remote add fork <fork-url>` inside
  the workspace, where `<fork-url>` is derived from `<upstream-url>` by
  substituting `fork_owner` for the upstream owner segment
- **AND** the resulting workspace has exactly two remotes: `origin`
  pointing at the upstream URL AND `fork` pointing at the fork URL

#### Scenario: Re-initializing an existing workspace (direct-push mode)
- **WHEN** the polling task begins an iteration AND the workspace path
  exists on disk AND `github.fork_owner` is unset
- **THEN** the manager runs `git fetch origin` inside the workspace and
  does NOT run a fresh clone
- **AND** any pre-existing local branches in the workspace are preserved

#### Scenario: Re-initializing an existing workspace (fork-PR mode)
- **WHEN** the polling task begins an iteration AND the workspace path
  exists on disk AND `github.fork_owner` is set
- **THEN** the manager runs `git fetch origin` AND ensures the `fork`
  remote exists with the correct URL (`git remote add fork <url>` if
  absent, OR `git remote set-url fork <url>` if present with a stale
  URL)
- **AND** the `fork` remote setup is idempotent across iterations

#### Scenario: Workspace exists but is not a git repository
- **WHEN** the workspace path exists but does not contain a `.git`
  directory
- **THEN** `ensure_initialized` returns an error naming the path and
  the missing `.git` marker
- **AND** the manager does NOT delete or modify the existing path

## ADDED Requirements

### Requirement: Fork URL derivation
The workspace manager SHALL derive the fork URL deterministically from
the upstream URL and `github.fork_owner` by substituting the owner
segment while preserving the URL scheme and the repository name.

#### Scenario: SSH upstream URL
- **WHEN** the upstream URL is `git@github.com:UpstreamOrg/repo.git` AND
  `github.fork_owner` is `machine-user`
- **THEN** the derived fork URL is `git@github.com:machine-user/repo.git`

#### Scenario: HTTPS upstream URL
- **WHEN** the upstream URL is `https://github.com/UpstreamOrg/repo.git`
  AND `github.fork_owner` is `machine-user`
- **THEN** the derived fork URL is
  `https://github.com/machine-user/repo.git`

#### Scenario: Unrecognized URL scheme
- **WHEN** the upstream URL uses a scheme other than
  `git@github.com:` or `https://github.com/` (e.g. an enterprise
  GitHub host)
- **THEN** fork URL derivation returns an error naming the upstream
  URL and the unsupported scheme
