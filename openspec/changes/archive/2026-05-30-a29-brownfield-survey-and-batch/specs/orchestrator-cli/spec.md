## ADDED Requirements

### Requirement: Brownfield-survey polling-iteration handler produces a capability list AND persists `BrownfieldSurveyState`
The polling iteration SHALL, after processing other chatops-driven request queues AND before standard change processing, drain at most one pending brownfield-survey request from `pending_brownfield_survey_requests`. The handler SHALL invoke the executor in survey mode with `WritePolicy::None` AND a sandbox profile permitting `Read`, `Glob`, `Grep`, AND `Bash` (read-only).

The survey prompt SHALL be loaded via `PromptLoader::load(PromptId::BrownfieldSurvey, &workspace_config)` (per the executor spec, established by `a24`). The prompt input SHALL include:

1. The resolved prompt template.
2. The operator's guidance, when non-empty, in a `## Operator guidance` section.
3. `README.md` contents AND the list of `docs/*.md` filenames.
4. A code-symbol overview (`cargo metadata` for Rust workspaces; ripgrep for other languages).
5. The list of already-specced capabilities — directories present under `<spec-root>/specs/` where `<spec-root>` honors `a26`'s `spec_storage.path` config when set. These SHALL be excluded from the survey output by instruction.
6. `features.brownfield_survey.max_capabilities` (passed into the prompt context so the LLM respects the cap).

The executor's response SHALL be a JSON array. Each item SHALL have:

- `id: usize` — 1-indexed sequential identifier.
- `slug: String` — proposed capability slug; matches `^[a-z][a-z0-9-]*$`.
- `summary: String` — one-line description.
- `scope_in: String` — short paragraph naming what's IN.
- `scope_out: String` — short paragraph naming related concerns NOT in this capability.
- `source_modules: Vec<String>` — source-tree paths the capability covers.
- `estimated_complexity: String` — `"small" | "medium" | "large"`.

The handler SHALL validate the response: well-formed JSON, every item has all required fields, slug matches the regex AND is NOT in the already-specced set, complexity is in the allowed set, AND `items.len() <= features.brownfield_survey.max_capabilities`. On validation failure: post a thread reply naming the failure AND do not persist state.

On success, the handler SHALL persist `BrownfieldSurveyState` at `<workspace>/.state/brownfield_surveys/<request_id>.json` with `status: Pending` AND each `SurveyItem.status: pending`. The handler SHALL render the list to the lifecycle thread (one section per item, grouped in the order returned) AND append the closing note `Reply with @<bot> send it to batch-generate ALL <N> specs (one per iteration). Or re-run @<bot> brownfield-survey <repo> <refined guidance> to refresh.`

#### Scenario: Happy-path survey run
- **WHEN** the executor returns a valid JSON list of 8 capabilities AND none of them collide with existing `<spec-root>/specs/<cap>/` directories
- **THEN** the handler persists `BrownfieldSurveyState` with 8 items, all `pending`, AND the survey `status: Pending`
- **AND** the thread reply lists 8 numbered items with the closing send-it instruction

#### Scenario: Already-specced capability excluded
- **WHEN** the executor's response includes an item with `slug: "scheduler"` AND `openspec/specs/scheduler/` already exists in the workspace
- **THEN** the handler rejects the response via validation
- **AND** the thread reply names the collision so the operator can re-run with refined guidance

#### Scenario: Slug regex violation rejects the run
- **WHEN** the response includes an item with `slug: "Bad_Slug"` (uppercase / underscore)
- **THEN** the handler rejects the run via validation
- **AND** the thread reply names the slug-regex failure

#### Scenario: Max-capabilities cap enforced
- **WHEN** `features.brownfield_survey.max_capabilities: 10` AND the executor returns 15 items
- **THEN** the handler rejects the run via validation
- **AND** the thread reply names the cap violation

#### Scenario: Survey uses spec_storage.path when set
- **WHEN** the workspace has `spec_storage.path: "../my-specs"` set per `a26`
- **THEN** the already-specced-capabilities listing reads from `../my-specs/openspec/specs/`, NOT `<workspace>/openspec/specs/`
- **AND** the survey persistence at `<workspace>/.state/brownfield_surveys/<request_id>.json` remains in the code workspace (state files always live with their workspace, not the spec_storage repo)

### Requirement: Brownfield-batch polling-iteration handler drains one survey item per iteration AND runs brownfield generation per item
On receipt of a `BrownfieldBatchAction { survey_request_id, channel, thread_ts }`, the daemon SHALL:

1. Load the referenced `BrownfieldSurveyState`. If the file is missing (cleared between dispatch AND processing), post `✗ send it: survey state <request_id> not found (was it cleared?). Re-run brownfield-survey for a fresh list.` AND return.
2. If the survey's `status` is `InProgress` OR `Completed`, post a thread reply naming the no-op AND return without changing state.
3. If ANY other survey on the same workspace has `status: InProgress`, post `✗ send it: a brownfield batch is already in progress for this workspace (survey <other-request_id>). Wait for it to finish OR run @<bot> clear-survey <repo> to abort.` AND return. Only ONE batch per workspace at a time.
4. Otherwise, transition `status` to `InProgress` (atomic-rename) AND post `✓ Queued <N> capability spec generations. The first will start on the next iteration.`

Each subsequent polling iteration SHALL, after processing other queues AND before standard change processing, drain ONE item from the in-progress survey:

1. Identify the workspace's in-progress survey (only one possible).
2. Find the first `SurveyItem` whose status is `pending`.
3. Re-check `<spec-root>/specs/<slug>/spec.md` does NOT exist (where `<spec-root>` honors `a26`'s `spec_storage.path`). If it does exist, mark the item `skipped` (the operator may have merged a sibling brownfield PR) AND return without invoking the executor.
4. Mark the item `generating`.
5. Run the canonical brownfield-generation flow from `a23` for the item's `slug` with the following prompt-input extension: APPEND a `## Survey context` section to the brownfield prompt containing the item's `scope_in`, `scope_out`, AND `source_modules`. The LLM SHALL use this to scope its draft appropriately.
6. On `Completed` outcome with valid change-directory artifacts AND successful PR creation: mark the item `completed`, persist `pr_url`, post `✅ Spec PR opened for \`<slug>\` (M/N done): <pr-url>` to the lifecycle thread.
7. On any failure (executor `Err`, missing artifacts, sandbox leak, PR-create failure): mark the item `failed`, persist `failure_reason`, post `✗ Spec for \`<slug>\` failed: <reason> (continuing with next)`.
8. When ALL items in the survey reach a terminal state (`completed`, `skipped`, OR `failed`), transition the survey `status` to `Completed` AND post the summary: `✅ Brownfield batch complete. <X> succeeded, <Y> skipped (already specced), <Z> failed. See the survey thread for individual PR links AND failure reasons.`

The batch handler does NOT process more than one item per polling iteration even if multiple are `pending`. The one-per-iteration discipline gives each brownfield run its own fresh executor invocation, eliminating mid-batch context compression as a failure mode.

#### Scenario: Batch start acknowledges queue size
- **WHEN** a `BrownfieldBatchAction` arrives for a survey with 5 pending items AND no other batch is in progress
- **THEN** the survey's `status` transitions to `InProgress`
- **AND** the thread receives `✓ Queued 5 capability spec generations. The first will start on the next iteration.`

#### Scenario: One item per iteration
- **WHEN** the in-progress survey has 5 pending items
- **THEN** iteration N processes item 1; iteration N+1 processes item 2; etc.
- **AND** no iteration processes more than one item from the survey

#### Scenario: Spec-already-exists triggers skip mid-batch
- **WHEN** item 3 (slug `auth`) is the next pending item AND between iterations the operator manually merged a sibling brownfield PR creating `openspec/specs/auth/spec.md`
- **THEN** the iteration marks item 3 `skipped` AND posts the skip notice
- **AND** does NOT invoke the executor for item 3
- **AND** the next iteration processes item 4

#### Scenario: Generation failure does not abort the batch
- **WHEN** item 2 generation fails (e.g., executor returns Failed with reason `revision suggestion: scope is unclear`)
- **THEN** item 2 is marked `failed` with the reason persisted
- **AND** the thread receives `✗ Spec for \`<slug>\` failed: revision suggestion: scope is unclear (continuing with next)`
- **AND** the next iteration processes item 3
- **AND** the batch does NOT abort

#### Scenario: All items terminal triggers summary
- **WHEN** the last `pending` item reaches a terminal state
- **THEN** the survey's `status` transitions to `Completed`
- **AND** the thread receives the batch-complete summary with success / skipped / failed counts

#### Scenario: Concurrent batch rejection
- **WHEN** a `BrownfieldBatchAction` arrives for survey A while survey B is already `InProgress` on the same workspace
- **THEN** survey A's status remains `Pending`
- **AND** the thread reply names survey B's request_id AND advises waiting OR clearing

#### Scenario: Spec_storage.path applies to batch
- **WHEN** the workspace has `spec_storage.path` configured AND the batch handler runs
- **THEN** every spec PR is created in the spec_storage repo, NOT the code workspace
- **AND** the per-item `pr_url` records the spec_storage repo's PR URL

### Requirement: `features.brownfield_survey` config schema
The per-repo config schema SHALL accept an optional `features.brownfield_survey` block:

- `enabled: bool` (default `true`) — when `false`, the `brownfield-survey`, `send it`-in-survey-thread, AND `clear-survey` verbs are refused at parse time.
- `prompt_path: Option<String>` (default `None`) — per the uniform PromptLoader pattern.
- `max_capabilities: usize` (default `20`, valid range `1..=50`) — cap on survey item count.

Invalid values cause config-load to fail-fast with a clear error.

#### Scenario: Default config enables the survey verbs
- **WHEN** a per-repo config omits the `features.brownfield_survey` block
- **THEN** all three fields take their defaults

#### Scenario: Explicit disable refuses all three related verbs
- **WHEN** a per-repo config sets `features.brownfield_survey.enabled: false`
- **THEN** the dispatcher refuses `@<bot> brownfield-survey`, refuses `@<bot> send it` when posted in a (still-present) survey thread, AND refuses `@<bot> clear-survey` for that workspace

#### Scenario: max_capabilities outside valid range fails config load
- **WHEN** a per-repo config sets `features.brownfield_survey.max_capabilities: 100`
- **THEN** config-load fails with an error naming the field AND the valid range `1..=50`
