## MODIFIED Requirements

### Requirement: Idempotent workspace initialization
The workspace manager SHALL ensure a repository is locally cloned before each polling iteration begins, performing a clone if absent and a fetch if present, without losing existing local state. When fork-PR mode is active (`github.fork_owner` is configured), the manager SHALL ALSO ensure a second remote named `fork` is registered, pointing at the fork URL derived from the upstream URL and `fork_owner`. When the manager performs a clone (the workspace path was absent) AND fork-PR mode is active, the manager SHALL ALSO fetch from the `fork` remote so the local tracking ref `refs/remotes/fork/<branch>` reflects the fork's actual state — necessary so subsequent `git push --force-with-lease` operations compare against accurate data and do not falsely report "stale info."

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
- **AND** the manager then runs `git fetch fork` inside the workspace,
  populating `refs/remotes/fork/*` so that local tracking reflects the
  fork's actual state
- **AND** the resulting workspace has exactly two remotes: `origin`
  pointing at the upstream URL AND `fork` pointing at the fork URL

#### Scenario: Fork fetch failure on first-time clone is non-fatal
- **WHEN** the post-clone `git fetch fork` step fails (network error,
  fork is empty, fork doesn't yet exist, authentication failure for
  the fork remote, etc.)
- **THEN** the manager logs the failure at WARN naming the fork URL
  and the error
- **AND** `ensure_initialized` still returns Ok — the clone +
  remote-registration succeeded, and the empty local tracking ref
  is no worse than the pre-fix behavior
- **AND** the next polling iteration proceeds normally; a real
  divergence will surface as a `--force-with-lease` failure with
  the existing branch-push-failure alert path

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
- **AND** the manager does NOT re-fetch the fork remote on every
  iteration — fork tracking refs persist across iterations and are
  updated by `git push` itself when autocoder pushes successfully

#### Scenario: Workspace exists but is not a git repository
- **WHEN** the workspace path exists but does not contain a `.git`
  directory
- **THEN** `ensure_initialized` returns an error naming the path and
  the missing `.git` marker
- **AND** the manager does NOT delete or modify the existing path
