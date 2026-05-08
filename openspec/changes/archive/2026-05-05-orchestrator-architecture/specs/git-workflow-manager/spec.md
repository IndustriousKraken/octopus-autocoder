## ADDED Requirements

### Requirement: Per-pass agent branch
The git workflow manager SHALL ensure each polling pass starts from a clean branch off the configured base branch, recreating the agent branch each pass.

#### Scenario: Branch initialization at start of pass
- **WHEN** a polling pass begins for a repository AND the queue contains at least one ready change
- **THEN** the manager runs, in order: `git fetch origin`, `git checkout <base_branch>`, `git pull --ff-only origin <base_branch>`, `git checkout -B <agent_branch>`
- **AND** the resulting `HEAD` of `<agent_branch>` is verifiable as identical to the post-pull `HEAD` of `<base_branch>` (`git rev-parse <agent_branch>` equals `git rev-parse <base_branch>`)
- **AND** prior local content on `<agent_branch>` is overwritten without warning — this is by design

#### Scenario: Pull conflict on base branch
- **WHEN** `git pull --ff-only origin <base_branch>` exits non-zero (non-fast-forward, network error, etc.)
- **THEN** the manager aborts the polling pass for this repository
- **AND** the workspace is left in its pre-pull state (no agent branch is created or modified for this pass)
- **AND** the captured stderr from the failing git command is logged verbatim

### Requirement: Serial commit per change
The git workflow manager SHALL produce one commit per successfully implemented change, on the agent branch, in queue order.

#### Scenario: Committing a change with modifications
- **WHEN** the executor returns `Completed` for `<change>` AND `git status --porcelain` returns a non-empty result inside the workspace
- **THEN** the manager runs `git add -A` followed by `git commit -m "<change>: <summary>"`, where `<summary>` is the first non-empty line of the `## Why` section of `<change>/proposal.md`, truncated to 72 characters total subject length
- **AND** the resulting commit is verifiable as a new commit on `<agent_branch>` whose tree differs from its parent (`git diff-tree --no-commit-id --name-only HEAD` returns a non-empty list)

#### Scenario: Executor reported Completed but produced no diff
- **WHEN** the executor returns `Completed` for `<change>` AND `git status --porcelain` returns empty
- **THEN** the manager logs a warning naming `<change>` and does NOT create an empty commit
- **AND** the change is still archived by the queue engine, since the executor explicitly signaled completion

### Requirement: Monolithic PR at end of pass
The git workflow manager SHALL push the agent branch and create a single Pull Request via the GitHub REST API at the end of each polling pass that produced at least one commit.

#### Scenario: Opening a PR after a productive pass
- **WHEN** a polling pass completes AND `<agent_branch>` contains at least one commit ahead of `<base_branch>` (`git rev-list --count <base_branch>..<agent_branch>` returns a value greater than zero)
- **THEN** the manager runs `git push --force-with-lease origin <agent_branch>`
- **AND** the manager issues an HTTP `POST` to `https://api.github.com/repos/<owner>/<repo>/pulls` with header `Authorization: Bearer <token>` (token sourced from the configured environment variable name) and a JSON body containing `head: "<agent_branch>"`, `base: "<base_branch>"`, and a title and body summarizing the included changes
- **AND** on a 2xx response, the manager logs the returned `html_url`
- **AND** on a non-2xx response, the manager logs the response status code and body verbatim, and the polling pass is recorded as failed; the agent branch and commits remain on the remote for human inspection

#### Scenario: Push rejected by force-with-lease
- **WHEN** `git push --force-with-lease` exits non-zero because the remote `<agent_branch>` has advanced beyond what the manager last observed
- **THEN** the manager aborts the PR creation step, leaves the local agent branch intact, and logs an error indicating that the remote was modified by an outside process
- **AND** no PR is opened

#### Scenario: No commits in pass
- **WHEN** a polling pass completes AND `<agent_branch>` contains zero commits ahead of `<base_branch>`
- **THEN** no push is performed, no PR is created, and the manager logs a single line indicating the pass produced no changes
