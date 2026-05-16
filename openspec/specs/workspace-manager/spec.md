# workspace-manager Specification

## Purpose
TBD - created by archiving change orchestrator-foundation. Update Purpose after archive.
## Requirements
### Requirement: Deterministic workspace path derivation
The workspace manager SHALL derive a per-repository workspace path deterministically from the configured URL, so that restarting the daemon reuses existing local clones rather than creating new ones.

#### Scenario: Path derivation is stable
- **WHEN** the manager derives a path for a given URL
- **THEN** invoking the same derivation a second time with the same URL returns a path equal by `==` to the first
- **AND** the path is rooted at `/tmp/workspaces/`

#### Scenario: Distinct URLs produce distinct paths
- **WHEN** the manager derives paths for two URLs that differ in host, owner, or repo name
- **THEN** the resulting paths are not equal
- **AND** repeated derivations preserve the inequality

### Requirement: Cross-repository path collision detection at startup
autocoder SHALL detect any two configured repositories that resolve to the same workspace path and refuse to start, naming both URLs and the shared path in the error message.

#### Scenario: Two repos derive to the same path
- **WHEN** autocoder loads a config containing two repositories whose URLs sanitize to the same workspace path (or whose explicit `local_path` overrides collide)
- **THEN** autocoder emits a startup error whose text contains BOTH conflicting URLs verbatim AND the shared path
- **AND** no polling tasks are spawned for either repository
- **AND** the process exits non-zero within 5 seconds of config load

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

