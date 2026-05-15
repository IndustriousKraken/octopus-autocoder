## MODIFIED Requirements

### Requirement: Reload handler hot-applies the safe config subset
The control socket's `reload` handler SHALL re-read the YAML config path the daemon was launched with, validate the new content fully (parse + semantic checks), and hot-apply changes to `github`, `reviewer`, `chatops`, AND `repositories` sections. Changes to the `executor` section SHALL NOT be hot-applied; the handler SHALL report it as `requires-restart` so the operator knows it still needs a full restart. The response SHALL include a `repositories_delta` field naming added / removed / changed repository URLs whenever the repository step modified the task set.

#### Scenario: Reload with no changes
- **WHEN** the YAML file is unchanged since startup AND the reload
  is triggered
- **THEN** the response is
  `{"ok": true, "applied": [], "requires_restart": [], "unchanged": ["github", "reviewer", "chatops", "repositories", "executor"], "repositories_delta": {"added": [], "removed": [], "changed": []}}`
- **AND** no in-memory state is modified

#### Scenario: Reload adds a new repository
- **WHEN** the new YAML contains a `repositories[]` entry whose
  `url` is not present in the current task map
- **THEN** autocoder spawns a new polling task for that URL
  (workspace path derivation, startup dirty-check, busy-marker
  acquire — all as at daemon startup)
- **AND** the new task receives an `Arc<ArcSwap<RepositoryConfig>>`
  seeded with the new entry's values
- **AND** the response's `applied` includes `"repositories"`
- **AND** the response's `repositories_delta.added` includes the
  new URL

#### Scenario: Reload removes a repository
- **WHEN** the new YAML omits a `repositories[]` entry whose `url`
  is currently in the task map
- **THEN** autocoder cancels that task's per-repo cancellation
  token
- **AND** the running task finishes its in-flight iteration
  normally (including push + PR if commits were produced) and
  exits at the next inter-poll sleep boundary
- **AND** the response's `repositories_delta.removed` includes the
  removed URL
- **AND** when the task exits, it removes its own entry from the
  daemon's task map

#### Scenario: Reload changes an existing repository's settings
- **WHEN** the new YAML contains a `repositories[]` entry whose
  `url` matches an existing task AND any other field
  (`base_branch`, `agent_branch`, `poll_interval_sec`,
  `chatops_channel_id`, `local_path`) differs
- **THEN** autocoder swaps the new values into that task's
  `ArcSwap<RepositoryConfig>` holder
- **AND** the next iteration of that task reads the new values
  (the current iteration, if one is in flight, completes with
  the old snapshot)
- **AND** the response's `repositories_delta.changed` includes
  the URL

#### Scenario: Reload changes a repository's URL
- **WHEN** the new YAML differs from the current YAML by replacing
  a repository's `url` value while leaving other fields the same
- **THEN** the diff treats this as `removed(old_url) +
  added(new_url)`: the old task is cancelled, a new task is
  spawned for the new URL
- **AND** the response's `repositories_delta` includes the old
  URL under `removed` and the new URL under `added`

#### Scenario: Reload during a repo's in-flight cancellation
- **WHEN** an earlier reload cancelled a repo's task but the
  task has not yet exited (its in-flight iteration is still
  running) AND a subsequent reload's new YAML re-adds that URL
- **THEN** autocoder logs a WARN naming the transient state
- **AND** the repo is NOT re-spawned on this reload (the URL is
  still in the task map but its token is cancelled)
- **AND** the response reports `"repositories"` as `unchanged`
  for this URL despite the YAML containing it; the next reload
  (after the old task has exited) will properly spawn the new
  task

#### Scenario: Reload with restart-required executor change
- **WHEN** the new YAML differs in `executor`
- **THEN** the executor section is NOT hot-applied
- **AND** the response includes `"executor"` under
  `requires_restart`
- **AND** other hot-applicable sections (including
  `repositories`) ARE applied if they also changed

#### Scenario: Reload rejected by validation
- **WHEN** the new YAML fails to parse (`serde_yaml` error) OR
  fails semantic validation (workspace collision between two
  repos, missing token route, etc.)
- **THEN** the response is `{"ok": false, "error": "<message>"}`
  naming the validation failure
- **AND** no in-memory state is modified, including no spawn / cancel
  of repository tasks
- **AND** the daemon continues running with the previous config

#### Scenario: Reload rejected by IO failure
- **WHEN** the YAML file cannot be read (permission denied, file
  missing)
- **THEN** the response is `{"ok": false, "error": "config file <path>: <error>"}`
- **AND** no in-memory state is modified

### Requirement: Per-repository asynchronous polling loop
autocoder SHALL spawn one tokio task per configured repository, each holding its `RepositoryConfig` behind an `Arc<ArcSwap<RepositoryConfig>>` and reading the current snapshot at the top of each iteration. Each task SHALL receive a per-repo `CancellationToken` derived from the global shutdown token via `child_token()`, so that the reload handler can cancel individual tasks without affecting siblings while a global SIGTERM still cancels all tasks.

#### Scenario: Task reads from swap holder at iteration start
- **WHEN** a polling iteration begins (after the inter-poll
  sleep)
- **THEN** the task calls `holder.load()` on its
  `Arc<ArcSwap<RepositoryConfig>>` to obtain a snapshot
  `Arc<RepositoryConfig>` for the duration of the iteration
- **AND** all per-iteration logic reads `base_branch`,
  `agent_branch`, `poll_interval_sec`, etc. from this snapshot
- **AND** no mid-iteration re-read occurs (a reload mid-iteration
  takes effect on the NEXT iteration)

#### Scenario: Per-repo cancellation token
- **WHEN** the daemon spawns a polling task
- **THEN** the task is given a `CancellationToken` derived from
  the daemon's global shutdown token via `child_token()`
- **AND** the inter-poll `tokio::select!` watches this token:
  if cancelled (by reload or by global shutdown), the task
  breaks out of its loop before starting the next iteration
- **AND** the per-repo token's cancellation does NOT affect
  sibling polling tasks

#### Scenario: Task cleans up its map entry on exit
- **WHEN** a polling task's loop exits (cancellation or fatal error)
- **THEN** the task removes its own entry from the daemon's
  task map keyed by repo URL
- **AND** subsequent reloads see the URL as absent and can
  re-spawn if the operator re-adds it
