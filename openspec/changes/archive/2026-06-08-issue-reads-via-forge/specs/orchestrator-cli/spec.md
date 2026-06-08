# orchestrator-cli — delta for issue-reads-via-forge

## MODIFIED Requirements

### Requirement: Scout polling-iteration handler produces a triage list AND persists `ScoutRunState`
The daemon's per-repo polling iteration SHALL, after processing pending proposal AND brownfield requests AND before the standard change-processing pass, drain at most one pending scout request from `pending_scout_requests`. The handler SHALL invoke the executor in scout mode with `WritePolicy::None` AND a sandbox profile permitting `Read`, `Glob`, `Grep`, AND `Bash` (read-only, with `gh` permitted).

The scout prompt SHALL be loaded via `PromptLoader::load(PromptId::Scout, &workspace_config)` (per the executor spec). The prompt input SHALL be assembled from:

1. The resolved prompt template.
2. The operator's guidance (when non-empty), interpolated into a `## Operator guidance` section.
3. The workspace's `README.md` contents AND the list of `docs/*.md` filenames.
4. A code-symbol overview built via `cargo metadata` (Rust workspaces) OR a ripgrep pass for top-level public items (other languages).
5. `git log --since="<N> days ago" --pretty=oneline` output for recent-activity context, where N is `features.scout.staleness_warn_days * 4`.
6. The open-issues list via the forge provider's authenticated open-issue listing (the same configured credential as PR operations; see the git-workflow-manager forge requirement), NOT the `gh` CLI, when `features.scout.include_issues: true`. On a forge issue-read failure (auth, rate limit, network), the handler SHALL log a WARN naming the failure AND continue with an empty issue list.

The executor's response SHALL be a JSON array of opportunity items. Each item SHALL have:

- `id: usize` — 1-indexed sequential identifier.
- `category: String` — one of: `security`, `bug`, `error_handling`, `type_tightening`, `code_smell`, `perf`, `documentation`, `test_coverage`, `issue`, `todo_fixme`, `research`.
- `title: String` — one-line summary.
- `body: String` — one-paragraph description.
- `source: String` — `<file>:<line>` for code-derived, issue URL for issue-derived, OR commit-range for git-log-derived.
- `tractability: String` — one of: `small`, `medium`, `large`.

The handler SHALL validate the response: well-formed JSON, every item has all required fields, categories AND tractability values fall in the allowed sets, AND `items.len() <= features.scout.max_items`. On validation failure, the handler SHALL post a thread reply naming the failure AND NOT persist any state file.

On validation success, the handler SHALL: write `<workspace>/.state/scout_runs/<request_id>.json` with `ScoutRunState { request_id, repo_url, guidance, head_sha_at_run, completed_at, thread_ts, channel, items }`; render the list (grouped by category, compact per-item format) AND post it to the request's thread; append the closing note `Reply with @<bot> spec-it <N> [optional guidance] to scope work on any item.`. When the rendered list exceeds the threaded-notification length limit, the handler SHALL truncate the displayed list AND append `… (truncated; full list in <workspace>/.state/scout_runs/<request_id>.json)`.

#### Scenario: Happy-path scout run
- **WHEN** the executor returns a valid JSON list of 12 items AND the issue fetch did not fail
- **THEN** the handler persists `ScoutRunState` with 12 items
- **AND** posts a thread reply grouping items by category with the closing spec-it instruction
- **AND** the thread reply does NOT contain `(truncated; …)`

#### Scenario: Invalid JSON aborts the run
- **WHEN** the executor returns text that is not valid JSON OR is missing required item fields
- **THEN** no state file is written
- **AND** the thread reply names the validation failure AND points at the daemon log

#### Scenario: Issue fetch unavailable falls through gracefully
- **WHEN** `features.scout.include_issues: true` AND the forge open-issue listing fails (auth, rate limit, network)
- **THEN** a WARN is logged naming the failure
- **AND** the scout proceeds with code-derived items only
- **AND** the thread reply includes a note that issue-derived items were skipped this run

#### Scenario: Long list triggers truncation
- **WHEN** the rendered list exceeds the threaded-notification length limit
- **THEN** the handler posts the first N categories that fit
- **AND** appends `… (truncated; full list in <workspace>/.state/scout_runs/<request_id>.json)`
- **AND** the persisted state file contains ALL items (truncation affects display only)

#### Scenario: Max-items cap enforced
- **WHEN** `features.scout.max_items: 10` AND the executor returns a list with 15 items
- **THEN** the handler rejects the run via the validation step
- **AND** the thread reply names the cap violation

### Requirement: `features.scout` config schema
The per-repo config schema SHALL accept an optional `features.scout` block:

- `enabled: bool` (default `true`) — when `false`, the `scout`, `spec-it`, AND `clear-scout` verbs are refused at parse time.
- `prompt_path: Option<String>` (default `None`) — per the uniform PromptLoader pattern.
- `max_items: usize` (default `30`, valid range `1..=50`) — cap on the scout's item list.
- `include_issues: bool` (default `true`) — controls whether the handler fetches open issues (via the forge provider's authenticated API) for inclusion in the scout input.
- `staleness_warn_days: u64` (default `7`) — threshold for the staleness warning.

Invalid values (non-bool where bool expected; `max_items` outside `1..=50`) cause config-load to fail-fast with an error naming the offending field.

#### Scenario: Default config enables scout
- **WHEN** a workspace's config omits the `features.scout` block
- **THEN** all five fields take their defaults (`enabled: true, prompt_path: None, max_items: 30, include_issues: true, staleness_warn_days: 7`)

#### Scenario: Explicit disable refuses all three verbs
- **WHEN** a workspace's config sets `features.scout.enabled: false`
- **THEN** the dispatcher refuses `@<bot> scout`, `@<bot> spec-it`, AND `@<bot> clear-scout` for that workspace

#### Scenario: max_items outside valid range fails config load
- **WHEN** a workspace's config sets `features.scout.max_items: 0` OR `features.scout.max_items: 100`
- **THEN** config-load fails with an error naming `features.scout.max_items` AND the valid range `1..=50`
