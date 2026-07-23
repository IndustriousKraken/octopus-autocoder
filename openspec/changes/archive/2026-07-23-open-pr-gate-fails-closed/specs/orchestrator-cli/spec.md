## MODIFIED Requirements

### Requirement: Skip iteration when an open PR exists for the agent branch
autocoder SHALL query GitHub for open PRs whose `head` matches the configured agent branch before running the executor on any pending changes. When such a PR exists, the iteration SHALL be skipped entirely — no executor invocation, no `recreate_branch` (which would obliterate the open PR's branch on the next force-push), no commit work. The skip persists across iterations until the open PR is closed or merged. This prevents redundant Claude executions, PR-diff thrashing under reviewers, and the 422 "PR already exists" loop that would otherwise occur every polling pass after a PR is opened but not resolved.

The gate SHALL fail closed: when the query cannot deliver an answer — a transport error, a non-2xx response, an unparseable repo URL, or a token-resolution failure — the iteration SHALL be skipped exactly as if an open PR existed, because "cannot confirm no open PR" risks precisely the harms this gate exists to prevent. A query failure parks only the CURRENT pass: the next iteration re-runs the query normally, so a transient failure costs one polling interval, never a duplicate agentic run. Persistent failure SHALL be operator-visible, not a silent idle: each failure-skip logs a WARN naming the cause, AND after three consecutive query-failure skips for a repository the daemon posts a throttled chatops alert naming the gate and the last error. A successful query that returns an empty list proceeds normally.

#### Scenario: An open PR exists for the agent branch
- **WHEN** the daemon completes workspace init and `pull --ff-only`
  succeeds AND a `GET /repos/{owner}/{repo}/pulls?state=open&head=<head>&base=<base>` query returns one or more PRs
- **THEN** the daemon logs an INFO line naming each PR number and
  the URL, and returns from the iteration without invoking
  `recreate_branch`, `walk_queue`, or any executor
- **AND** the polling task continues with its normal sleep + next-iteration cycle

#### Scenario: No open PR exists for the agent branch
- **WHEN** the GitHub query returns an empty list
- **THEN** the daemon proceeds with `recreate_branch` and the
  normal iteration as before

#### Scenario: GitHub query fails — the pass is skipped, not risked
- **WHEN** the `pulls` query errors at the transport layer or returns a non-2xx status (or the repo URL cannot be parsed, or no token resolves)
- **THEN** the daemon logs a WARN naming the failure (status code and/or error text)
- **AND** the iteration is skipped exactly as if an open PR existed — no `recreate_branch`, no lane walks, no executor
- **AND** the next iteration re-runs the query normally (the failure parks one pass, not the repository)

#### Scenario: A transient blip costs one pass and self-heals
- **WHEN** one iteration's open-PR query fails AND the next iteration's query succeeds with an empty list
- **THEN** the next iteration proceeds with normal work
- **AND** no duplicate executor run occurred for work whose PR was open during the failed pass

#### Scenario: Sustained query failure raises an operator alert
- **WHEN** the open-PR query fails on three consecutive iterations for a repository
- **THEN** the daemon posts a throttled chatops alert naming the open-PR gate, the repository, and the most recent error
- **AND** subsequent consecutive failures do not re-alert within the throttle window
- **AND** a successful query resets the consecutive-failure count

#### Scenario: Fork-PR mode head qualifier
- **WHEN** `github.fork_owner` is set
- **THEN** the `head` query parameter is
  `<fork_owner>:<agent_branch>` so GitHub disambiguates correctly
  against the upstream repo's PR list

#### Scenario: Direct mode head qualifier
- **WHEN** `github.fork_owner` is unset
- **THEN** the `head` query parameter is
  `<repo_owner>:<agent_branch>` where `<repo_owner>` is parsed
  from `repo.url`
