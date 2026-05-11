# git-workflow-manager Specification

## Purpose
TBD - created by archiving change orchestrator-architecture. Update Purpose after archive.
## Requirements
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
The git workflow manager SHALL push the agent branch and create a single Pull Request via the GitHub REST API at the end of each polling iteration that produced at least one commit. **When the code-reviewer is enabled, the PR body SHALL include the reviewer's report under a `## Code Review` heading, and a `Block` verdict SHALL cause the PR to be created as a draft (with a `do-not-merge` label fallback if the host rejects drafts).**

#### Scenario: Opening a PR with a passing review
- **WHEN** an iteration completes AND the agent branch contains at least one commit ahead of base AND `reviewer.enabled` is true AND `code_reviewer.review` returns `Ok(ReviewReport { verdict: Pass, .. })`
- **THEN** the manager pushes with `--force-with-lease` and POSTs to the GitHub PR API with `draft: false` and a body whose final section is `## Code Review` followed by the reviewer's `markdown`

#### Scenario: Opening a PR with a Block verdict
- **WHEN** an iteration completes AND the reviewer returns `Ok(ReviewReport { verdict: Block, .. })`
- **THEN** the manager pushes the agent branch and POSTs to the GitHub PR API with `draft: true`
- **AND** the PR body's final section is `## Code Review` followed by the reviewer's `markdown`

#### Scenario: Reviewer disabled or absent
- **WHEN** the `reviewer` config block is absent OR `reviewer.enabled` is false
- **THEN** the manager pushes the agent branch and POSTs to the GitHub PR API with `draft: false` and a body that does NOT include a `## Code Review` section
- **AND** no LLM API call is made

#### Scenario: Reviewer failure
- **WHEN** `reviewer.enabled` is true AND `code_reviewer.review` returns `Err(_)`
- **THEN** the manager logs `"reviewer failed: {error}"` naming the reason
- **AND** the manager pushes the agent branch and POSTs to the GitHub PR API with `draft: false`
- **AND** the PR body's `## Code Review` section contains only the line `(reviewer failed: <reason>)`

#### Scenario: Draft creation falls back to label
- **WHEN** `Block` verdict requires `draft: true` AND the GitHub API rejects the draft flag (specific GitHub error indicating drafts are not supported on this repo)
- **THEN** the manager retries the PR creation request with `draft: false`
- **AND** on success, the manager POSTs to `https://api.github.com/repos/<owner>/<repo>/issues/<pr_number>/labels` with body `{ "labels": ["do-not-merge"] }`
- **AND** the manager logs `"draft unsupported; applied do-not-merge label as fallback"`

