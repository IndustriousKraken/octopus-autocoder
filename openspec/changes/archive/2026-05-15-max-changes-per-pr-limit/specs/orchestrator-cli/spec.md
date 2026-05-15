## MODIFIED Requirements

### Requirement: Per-repository asynchronous polling loop
autocoder SHALL implement the per-repository polling task referenced in `orchestrator-architecture/specs/orchestrator-cli/spec.md` as a sleep-then-iterate cycle that runs the architecture's single-pass workflow on every iteration. Each iteration's queue walk SHALL commit at most `max_changes_per_pr` archived changes before stopping; remaining pending changes wait for the next iteration.

#### Scenario: Spawn count matches config
- **WHEN** the daemon starts with a config containing N repositories AND the workspace collision check passes
- **THEN** exactly N polling tasks are spawned via `tokio::task::JoinSet`
- **AND** each task owns its own workspace path (no two tasks share a path; collision detection at startup enforces non-overlap)

#### Scenario: Normal iteration
- **WHEN** a polling task wakes (start of process or end of previous sleep)
- **THEN** it runs the full single-pass workflow for its repository: workspace init → stale-lock cleanup → dirty-workspace refusal → branch recreation → queue walk → push and PR creation if any commits were produced
- **AND** the task then sleeps for `poll_interval_sec` before iterating again
- **AND** no two iterations within the same task overlap

#### Scenario: Iteration runtime exceeds poll interval
- **WHEN** an iteration's wall-clock runtime exceeds `poll_interval_sec`
- **THEN** the next iteration begins immediately after the current one finishes
- **AND** no negative sleep is attempted; no two iterations within the same task run in parallel

#### Scenario: Per-iteration commit cap
- **WHEN** the queue walk has already archived `max_changes_per_pr`
  changes during the current iteration AND additional pending changes
  remain in the queue
- **THEN** the walk stops; the iteration proceeds to the push+PR step
  with exactly `max_changes_per_pr` commits
- **AND** the remaining pending changes are NOT removed from the queue
  and SHALL be picked up by the next iteration

#### Scenario: Failed and escalated changes do not count toward cap
- **WHEN** the queue walk processes a change whose outcome is `Failed`
  or `Escalated` (i.e. no commit is produced)
- **THEN** that change does NOT count toward `max_changes_per_pr`; the
  walk continues to the next pending change
- **AND** only `Archived` and `ArchivedSelfHeal` outcomes (which DO
  produce commits) increment the count

#### Scenario: Resumed waiting change counts toward cap
- **WHEN** the pass begins by resuming a previously-waiting change AND
  that resume archives successfully
- **THEN** the resumed-and-archived change counts as `1` toward the
  iteration's `max_changes_per_pr` cap before the walk reads new
  pending changes from the queue

#### Scenario: Cap of 1 ships one change per PR
- **WHEN** `max_changes_per_pr == 1` AND the queue contains multiple
  pending changes
- **THEN** each polling iteration ships exactly one change per PR; the
  queue drains across N iterations rather than one
- **AND** the operator sees N small PRs over time rather than one large PR

## ADDED Requirements

### Requirement: Per-repository config schema for the polling loop
The `RepositoryConfig` schema SHALL include an optional `max_changes_per_pr: u32` field that bounds the number of archived changes committed in one iteration's PR. When unset on a repository, the value SHALL fall back to the executor-level default `executor.max_changes_per_pr`; when both are unset, the global default of `3` SHALL apply.

#### Scenario: Per-repo override takes precedence
- **WHEN** a repository sets `max_changes_per_pr: 5` AND
  `executor.max_changes_per_pr` is unset (or set to a different value)
- **THEN** the effective cap for that repository is `5`

#### Scenario: Executor-level fallback applies when per-repo is unset
- **WHEN** a repository does NOT set `max_changes_per_pr` AND
  `executor.max_changes_per_pr` is `2`
- **THEN** the effective cap for that repository is `2`
- **AND** other repositories that also do not set the field also get
  `2` (the executor-level default is global)

#### Scenario: Global default when neither is configured
- **WHEN** neither `RepositoryConfig.max_changes_per_pr` nor
  `executor.max_changes_per_pr` is set
- **THEN** the effective cap is `3` for every repository

#### Scenario: A configured zero is clamped to one with a warning
- **WHEN** a configured value (per-repo or executor-level) is `0`
- **THEN** autocoder treats the effective cap as `1` AND emits exactly
  one WARN-level log line at startup naming the field path (e.g.
  `repositories[2].max_changes_per_pr` or
  `executor.max_changes_per_pr`) and the clamp
- **AND** the loaded `Config` retains the raw `0` so operator-visible
  diagnostics show what was configured (matching the
  `perma_stuck_after_failures` precedent)
