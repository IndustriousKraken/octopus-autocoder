## MODIFIED Requirements

### Requirement: Per-pass agent branch
The git workflow manager SHALL ensure each polling pass starts from a
clean branch off the configured base branch, recreating the agent
branch each pass. The branch source remains `origin/<base_branch>` in
both direct-push and fork-PR modes — the fork's view of the base
branch is never consulted.

EXCEPT when a **push-block marker** is present for the workspace AND the agent
branch tip matches the marker's recorded tip commit (the state left by a prior pass
whose branch push failed, per the `orchestrator-cli` requirement "Branch-push
failure preserves completed work via a push-block hold"): in that case the pass
SHALL NOT recreate or overwrite the agent branch, so the preserved commits are
retained, AND it SHALL NOT re-run the executor for those already-committed changes.
The pass instead retries the push step only; on success the marker is removed and
the PR opened. A push failure SHALL never cause the completed work on the branch to
be discarded. If a marker is present but the tip no longer matches, the marker is
stale: it is removed and the branch is recreated normally.

#### Scenario: Branch initialization at start of pass
- **WHEN** a polling pass begins for a repository AND the queue
  contains at least one ready change AND no push-block marker is
  present (or a present marker is stale — its tip no longer matches)
- **THEN** the manager runs, in order: `git fetch origin`,
  `git checkout <base_branch>`,
  `git pull --ff-only origin <base_branch>`,
  `git checkout -B <agent_branch>`
- **AND** the resulting `HEAD` of `<agent_branch>` is verifiable as
  identical to the post-pull `HEAD` of `<base_branch>` (`git rev-parse
  <agent_branch>` equals `git rev-parse <base_branch>`)
- **AND** prior local content on `<agent_branch>` is overwritten
  without warning — this is by design
- **AND** in fork-PR mode, the `fork` remote is NEVER consulted
  during branch initialization (it is push-only)

#### Scenario: Pull conflict on base branch
- **WHEN** `git pull --ff-only origin <base_branch>` exits non-zero
  (non-fast-forward, network error, etc.)
- **THEN** the manager aborts the polling pass for this repository
- **AND** the workspace is left in its pre-pull state (no agent
  branch is created or modified for this pass)
- **AND** the captured stderr from the failing git command is logged
  verbatim

#### Scenario: Preserved unpushed work is not overwritten
- **WHEN** a polling pass begins AND a push-block marker is present for the
  workspace AND the agent branch tip matches the marker's recorded tip commit
- **THEN** the manager SHALL NOT run `git checkout -B <agent_branch>` and SHALL NOT
  otherwise reset or overwrite the agent branch
- **AND** the preserved commits remain on `<agent_branch>`
- **AND** the executor is NOT re-run for the already-committed changes
- **AND** whether the push is retried this pass is governed by the push-block hold
  (per the `orchestrator-cli` requirement), independent of recreating the branch
