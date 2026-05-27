## MODIFIED Requirements

### Requirement: Throttled predictable-failure alerts
autocoder SHALL emit a ChatOps notification at most once every 24 hours per (repository, failure category) combination for three categories of predictable infrastructure failure: `workspace_init_failure`, `branch_push_failure`, `pr_creation_failure`. Throttle state SHALL be persisted at `<state_dir>/alert-state/<workspace-basename>.json` (resolved via the daemon's `DaemonPaths.alert_state_path()` helper) AND cleared on the next successful iteration of the same repository. The state file lives outside the managed repository's workspace — daemon bookkeeping never appears in the managed repo's working tree, nor in `git status`, nor in any `git checkout` operation's clobber-protection logic.

#### Scenario: First failure in a category alerts immediately
- **WHEN** any of the three categorized failures occurs in a repository whose `<state_dir>/alert-state/<basename>.json` has no entry for that category AND `slack.notifications.failure_alerts` is unset OR `true`
- **THEN** autocoder calls `chatops.post_notification(channel, text)` with category-specific text containing the repo URL, a category label, and a truncated error excerpt (max 200 chars)
- **AND** on successful post, autocoder writes the category's `last_alerted_at` (current UTC) and `last_error_excerpt` to `<state_dir>/alert-state/<basename>.json` atomically (tempfile-then-rename)

#### Scenario: Repeat failure within 24h is silent
- **WHEN** a categorized failure occurs in a repository whose `<state_dir>/alert-state/<basename>.json` has an entry for that category with `last_alerted_at` within the past 24 hours
- **THEN** no notification is posted for that iteration
- **AND** the state file is NOT modified

#### Scenario: Repeat failure beyond 24h re-alerts
- **WHEN** a categorized failure occurs AND `now - last_alerted_at >= 24h`
- **THEN** a new notification is posted with the most recent error excerpt
- **AND** `last_alerted_at` is updated to the current UTC time

#### Scenario: Success clears alert state
- **WHEN** an iteration of a repository completes its `run_pass_through_commits` workflow without returning Err (regardless of whether any changes were processed or whether the queue was empty)
- **THEN** autocoder removes `<state_dir>/alert-state/<basename>.json` from disk (or writes an empty `{ "alerts": {} }` map, equivalent semantics)
- **AND** the next failure of any category re-alerts immediately

#### Scenario: Alert post failure does NOT update state
- **WHEN** a categorized failure occurs AND the 24h window is open AND `post_notification` itself returns Err
- **THEN** the failure is logged to stderr including the alert text that would have been posted
- **AND** the state file is NOT updated (so the next iteration re-attempts the alert immediately)

#### Scenario: Failure-alerts disabled
- **WHEN** `slack.notifications.failure_alerts` is `false`
- **THEN** no failure alerts are posted regardless of category or history
- **AND** the state file is NEITHER read NOR written
- **AND** the failure still produces the existing stderr log line

#### Scenario: Out-of-scope failures are not alerted
- **WHEN** an executor returns `Failed` OR the reviewer LLM call fails OR `post_notification` itself fails
- **THEN** no failure alert is posted (these categories are out of scope for this change)

#### Scenario: State file never appears in the managed workspace
- **WHEN** the daemon writes alert-state for any repository
- **THEN** no file named `.alert-state.json` exists at any path inside the repository's workspace directory
- **AND** `git status` in the workspace shows no daemon-bookkeeping file (the workspace contains only the repo's tracked content, the daemon's own `.git/info/exclude`-listed in-workspace bookkeeping per other specs, AND any operator-edited uncommitted work)
- **AND** the daemon's writes never interfere with the workspace's git checkout / dirty-check / pull operations

## ADDED Requirements

### Requirement: Alert-state migration from workspace to state-dir on first startup
On the first daemon start after this spec ships, autocoder SHALL migrate any pre-existing `<workspace>/.alert-state.json` files to their corresponding `<state_dir>/alert-state/<basename>.json` paths. The migration SHALL be per-repository AND idempotent. A daemon-wide migration marker `<state_dir>/alert-state/.migration-from-workspace-done` records that the scan ran AND prevents subsequent startups from re-attempting work.

The migration handles three cases per workspace:

1. **Workspace file exists, state-dir file absent**: move the file via `fs::rename` (same-filesystem) or copy + delete (cross-filesystem).
2. **Both files exist**: the state-dir version wins (more recently authoritative AND survived any prior workspace wipes). Delete the workspace file.
3. **Workspace file is git-tracked** (rare; only for repos whose history transiently committed it): run `git rm --cached <workspace>/.alert-state.json`, commit with subject `chore: untrack .alert-state.json (now stored in daemon state dir per a16)`, push to the base branch.

The migration runs at daemon startup BEFORE any polling task starts. Errors during migration are per-repository AND non-fatal: if one repository's migration fails (e.g., `git push` rejected due to branch protection), the daemon logs ERROR naming the repository AND the failure mode, continues processing other repositories, AND does NOT set the migration marker. Subsequent startups retry.

#### Scenario: Workspace file moves to state-dir cleanly
- **WHEN** the daemon starts up AND the migration marker is absent AND a configured repository has `<workspace>/.alert-state.json` (not git-tracked, no state-dir version present)
- **THEN** the daemon moves the file to `<state_dir>/alert-state/<basename>.json`
- **AND** logs INFO naming the repository AND the source + destination paths
- **AND** after all repositories complete, writes the migration marker

#### Scenario: Both-files-exist case prefers state-dir
- **WHEN** a configured repository has BOTH `<workspace>/.alert-state.json` AND `<state_dir>/alert-state/<basename>.json`
- **THEN** the daemon deletes the workspace file AND keeps the state-dir version unchanged
- **AND** logs INFO noting that the state-dir version was preferred

#### Scenario: Git-tracked workspace file is untracked + committed + pushed
- **WHEN** a configured repository has `<workspace>/.alert-state.json` AND `git ls-files` shows the file tracked
- **THEN** the migration runs `git rm --cached`, commits with the documented subject, AND pushes to the base branch
- **AND** the migration treats success as "complete for this repository"
- **AND** on push failure, the migration logs ERROR with the suggested operator action AND continues to other repositories

#### Scenario: Migration is idempotent via the marker
- **WHEN** the daemon starts up AND `<state_dir>/alert-state/.migration-from-workspace-done` exists
- **THEN** the migration code returns immediately without scanning any workspace
- **AND** the daemon proceeds to its normal startup flow

#### Scenario: Per-repository failure does not set the marker
- **WHEN** the migration attempts a repository whose `git push` fails
- **THEN** the daemon logs ERROR with the failure detail AND the operator action
- **AND** the migration marker is NOT set (so the next startup retries)
- **AND** other repositories' migrations are unaffected — they continue to attempt AND succeed independently

#### Scenario: No pre-existing workspace files means no-op migration
- **WHEN** the daemon starts up AND NO configured repository has `<workspace>/.alert-state.json`
- **THEN** the migration logs INFO noting that no files needed migration
- **AND** writes the migration marker (recording that the scan ran AND found nothing)
- **AND** subsequent startups skip the scan
