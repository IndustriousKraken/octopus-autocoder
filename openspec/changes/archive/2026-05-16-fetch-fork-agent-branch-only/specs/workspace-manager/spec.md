## MODIFIED Requirements

### Requirement: Idempotent workspace initialization
The workspace manager SHALL ensure a repository is locally cloned before each polling iteration begins, performing a clone if absent and a fetch if present, without losing existing local state. When fork-PR mode is active (`github.fork_owner` is configured), the manager SHALL ALSO ensure a second remote named `fork` is registered, pointing at the fork URL derived from the upstream URL and `fork_owner`. When the manager performs a clone (the workspace path was absent) AND fork-PR mode is active, the manager SHALL ALSO fetch ONLY the configured agent branch from the `fork` remote — using an explicit refspec `+refs/heads/<agent_branch>:refs/remotes/fork/<agent_branch>` — so the local tracking ref reflects the fork's actual state for that single branch. The fetch SHALL NOT populate any other `refs/remotes/fork/<other-branch>` refs, because a fork branch whose name shadows an upstream branch (e.g. both `origin/dev` and `fork/dev` present) would otherwise cause `git checkout <base_branch>` DWIM to fail with "matched multiple remote tracking branches."

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
- **AND** the manager then runs
  `git fetch fork +refs/heads/<agent_branch>:refs/remotes/fork/<agent_branch>`
  inside the workspace, populating ONLY `refs/remotes/fork/<agent_branch>`
- **AND** no other `refs/remotes/fork/<branch>` refs are populated by
  this fetch (any pre-existing remote-tracking refs from prior
  iterations are preserved, but new ones for non-agent branches are
  NOT created)
- **AND** the resulting workspace has exactly two remotes: `origin`
  pointing at the upstream URL AND `fork` pointing at the fork URL

#### Scenario: Fork has a branch that shadows an upstream branch name
- **WHEN** the upstream repository has branches `main` and `dev`
- **AND** the fork has its own `dev` branch (a leftover from previous
  work, possibly with a different SHA than `origin/dev`)
- **AND** the polling task begins an iteration AND the workspace path
  does not exist on disk AND `github.fork_owner` is set
- **AND** the configured agent branch is `agent-q`
- **THEN** after `ensure_initialized` completes, the local tracking
  ref `refs/remotes/fork/agent-q` resolves (if it exists on the fork)
- **AND** `refs/remotes/fork/dev` does NOT resolve (the fetch refspec
  did not match it)
- **AND** a subsequent `git checkout dev` succeeds without the
  "matched multiple remote tracking branches" error, because
  `refs/remotes/origin/dev` is the only `dev` remote-tracking ref

#### Scenario: Fork fetch failure on first-time clone is non-fatal
- **WHEN** the post-clone
  `git fetch fork +refs/heads/<agent_branch>:refs/remotes/fork/<agent_branch>`
  step fails (network error, fork is empty, fork doesn't yet exist,
  authentication failure for the fork remote, the agent branch does
  not yet exist on the fork, etc.)
- **THEN** the manager logs the failure at WARN naming the fork URL,
  the agent branch, and the error
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
