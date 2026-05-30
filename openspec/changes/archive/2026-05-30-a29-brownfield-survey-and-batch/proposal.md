## Why

`a23` introduced the `brownfield` verb for documenting ONE capability at a time on an existing codebase. It's the right shape for incremental retrofit AND for narrow gaps, but it puts the burden of "which capabilities does this project have, AND in what order should I spec them" on the operator. For a previously-unspecced project, that's the hardest question — the operator needs to read the whole codebase to answer it.

Two problems flow from the manual approach:

1. **Operator-side cognitive load.** A medium-sized codebase has 5-15 plausible capability boundaries. Identifying them by hand requires reading the project carefully, which is exactly the work brownfield exists to reduce.
2. **Context-compression risk on a single-shot all-at-once attempt.** An operator who tries to side-step the per-capability flow by asking the implementer to "spec the whole project" runs into mid-iteration context compression OR the LLM losing thread on later capabilities after spending its budget on earlier ones.

A `brownfield-survey` verb addresses both. The survey pass is read-only AND CHEAP — it produces a list of proposed capability boundaries (slug, scope, complexity, source modules) without writing any specs. The operator reviews the list, refines via re-invocation with new guidance if needed, AND triggers batch generation when satisfied. The batch handler then drains the survey list ONE CAPABILITY PER ITERATION — each brownfield generation runs in its own fresh executor invocation, eliminating compression-mid-batch as a failure mode.

The shape mirrors the scout → spec-it pattern from `a25`: a curated-list-producing verb followed by an act-on-list verb. Where scout produces opportunities AND spec-it picks ONE, brownfield-survey produces capabilities AND `send it` acts on ALL.

## What Changes

**New `brownfield-survey` chatops verb.** Syntax:

```
@<bot> brownfield-survey <repo-substring> [optional guidance]
```

Repo-substring follows the canonical match rule. Optional guidance is everything after the substring (trimmed, capped at 10,000 characters); it steers the survey's focus (e.g., `focus on the data layer; skip CLI commands which are well-understood`). The dispatcher emits a `BrownfieldSurveyAction { repo_url, guidance: Option<String>, channel, thread_ts, request_id }` AND posts a top-level ack whose `ts` becomes the lifecycle thread.

**Brownfield-survey polling-iteration handler.** A new per-iteration step drains at most one `BrownfieldSurveyAction` per iteration. The handler invokes the executor in survey mode:

- `WritePolicy::None` — the survey writes no files; output is a JSON array returned to the daemon.
- Sandbox: `Read`, `Glob`, `Grep`, `Bash` (read-only).
- Inputs: the survey prompt template (default `prompts/brownfield-survey.md`; override via `features.brownfield_survey.prompt_path` per `a24`'s uniform PromptLoader), the workspace's `README.md` + the list of `docs/*.md` filenames, a code-symbol overview, the existing `openspec/specs/` listing (so already-specced capabilities are excluded from the survey), AND the operator's optional guidance.

The executor's response SHALL be a JSON array of proposed capabilities. Each item:

- `id: usize` — 1-indexed sequential identifier within the survey run.
- `slug: String` — proposed capability slug (matches `^[a-z][a-z0-9-]*$`).
- `summary: String` — one-line description.
- `scope_in: String` — short paragraph naming what's IN this capability (modules, behaviors).
- `scope_out: String` — short paragraph naming related concerns that DO NOT belong in this capability (handed off to other capabilities OR explicitly out-of-scope).
- `source_modules: Vec<String>` — list of source-tree paths (e.g., `src/scheduler/`, `src/cron/`) the capability covers.
- `estimated_complexity: String` — one of `small`, `medium`, `large` (heuristic the survey LLM applies; not enforced).

The list is capped at `features.brownfield_survey.max_capabilities` (default `20`). The handler persists `BrownfieldSurveyState { request_id, repo_url, guidance, head_sha_at_survey, completed_at, thread_ts, channel, items: Vec<SurveyItem>, status: Pending }` to `<workspace>/.state/brownfield_surveys/<request_id>.json` AND posts the rendered list to the lifecycle thread.

**Send-it in a brownfield-survey thread = batch generation.** The existing `send it` verb gains a new recognized context: when posted as a reply inside a brownfield-survey lifecycle thread, the verb SHALL submit a `BrownfieldBatchAction { survey_request_id, channel, thread_ts }` instead of the canonical audit-triage action. The two contexts (audit thread vs survey thread) are distinguished at parse time by looking up the parent thread's `ts` against per-workspace `BrownfieldSurveyState.thread_ts` AND `AuditThreadState.thread_ts` values; the listener routes to the appropriate action.

The `BrownfieldBatchAction` handler in the polling iteration:

1. Loads the referenced `BrownfieldSurveyState`. If status is already `InProgress` OR `Completed`, post a thread reply naming the no-op AND return.
2. Transitions the state to `InProgress`. Each item's per-item status starts as `pending`.
3. Posts `✓ Queued <N> capability spec generations. The first will start on the next iteration.`.

Subsequent iterations drain ONE item at a time per iteration:

1. Find the first item whose status is `pending`.
2. Re-check `openspec/specs/<slug>/spec.md` does not exist; if it does (the operator merged a sibling brownfield PR), mark the item `skipped` AND return to the next iteration.
3. Mark the item `generating`.
4. Run the canonical brownfield generation flow from `a23` for that capability — same executor mode, same `OpenSpecOnly` sandbox, same prompt + guidance translation, same spec-only PR output. The survey item's `scope_in`, `scope_out`, AND `source_modules` are appended to the brownfield prompt as additional guidance so the LLM scopes its draft accordingly.
5. On success: mark the item `completed`, persist the PR URL in the item's state, post `✅ Spec PR opened for \`<slug>\` (M/N done): <pr-url>` to the survey thread.
6. On failure: mark the item `failed` with the reason, post `✗ Spec for \`<slug>\` failed: <reason> (continuing with next)`, AND return to the next iteration.

When ALL items reach a terminal state (`completed`, `skipped`, OR `failed`):

- Transition `BrownfieldSurveyState.status` to `Completed`.
- Post a summary to the thread: `✅ Brownfield batch complete. <X> succeeded, <Y> skipped (already specced), <Z> failed. See the survey thread for individual PR links AND failure reasons.`

**New `clear-survey` chatops verb.** Operator-recovery verb (alongside `clear-perma-stuck`, `clear-revision`, `clear-scout` from `a25`). Syntax:

```
@<bot> clear-survey <repo-substring>
```

Deletes all `BrownfieldSurveyState` files for the repo. Idempotent. Replies with the count deleted.

**Mid-batch behavior on operator intervention.**

- If the operator runs `@<bot> brownfield-survey` again mid-batch (a fresh survey overwrites the previous), the in-progress batch SHALL complete the item currently in `generating` state, then halt (subsequent `pending` items in the OLD survey are NOT processed). The new survey's lifecycle is independent.
- If the operator runs `@<bot> revise <text>` on one of the spec PRs produced mid-batch, the revise loop applies per the canonical mechanism without affecting the batch's progress.
- If a generation fails persistently (the operator can see why in the thread + the daemon log), the operator can either: re-run `brownfield-survey` to refresh the list (the failed item's `openspec/specs/<slug>/spec.md` doesn't exist so it'll re-appear), OR invoke `@<bot> brownfield <repo> <slug> [guidance]` for that specific capability per `a23`'s single-capability flow.

**Configuration:**

```yaml
features:
  brownfield_survey:
    enabled: true                  # disable per-workspace
    prompt_path: null              # uniform a24 override pattern
    max_capabilities: 20           # cap on the survey list length; valid 1..=50
```

The survey verb is implicitly available when `features.brownfield.enabled: true` (the existing knob from `a23`); operators who want survey but not single-cap brownfield (or vice versa) can disable independently.

**`spec_storage.path` (from `a26`) applies uniformly.** When configured, all spec writes during the batch land in the spec_storage repo, NOT the code workspace.

## Impact

- **Affected specs:**
  - `chatops-manager` — ADDED: `Inbound listener recognizes the brownfield-survey verb AND submits a BrownfieldSurveyAction`. ADDED: `Inbound listener routes send-it to BrownfieldBatchAction when posted in a brownfield-survey thread`. ADDED: `Inbound listener recognizes the clear-survey verb`.
  - `orchestrator-cli` — ADDED: `Brownfield-survey polling-iteration handler produces a capability list AND persists BrownfieldSurveyState`. ADDED: `Brownfield-batch polling-iteration handler drains one survey item per iteration AND runs brownfield generation per item`. ADDED: `features.brownfield_survey config schema`.
  - `project-documentation` — ADDED: `docs/CHATOPS.md, docs/OPERATIONS.md, AND docs/CONFIG.md document the brownfield-survey, send-it-in-survey-thread, AND clear-survey verbs alongside the features.brownfield_survey config block`.
- **Affected code:**
  - `autocoder/src/chatops/listener.rs` — recognize `brownfield-survey` AND `clear-survey`; route `send it` to the correct action based on parent-thread context (audit-thread → existing canonical handler; survey-thread → new `BrownfieldBatchAction`).
  - `autocoder/src/control_socket/actions.rs` — `BrownfieldSurveyAction`, `BrownfieldBatchAction`, `ClearSurveyAction` variants.
  - `autocoder/src/state/brownfield_survey.rs` (new) — `BrownfieldSurveyState` AND `SurveyItem` with atomic-rename writes.
  - `autocoder/src/polling/brownfield_survey.rs` (new) — survey handler producing the list.
  - `autocoder/src/polling/brownfield_batch.rs` (new) — batch handler draining one item per iteration AND invoking the existing brownfield generation flow.
  - `autocoder/src/config.rs` — extend `features` with the `brownfield_survey` sub-block.
  - `prompts/brownfield-survey.md` (new) — embedded default survey prompt.
  - `PromptId::BrownfieldSurvey` (added in `a24`'s loader registry).
  - `docs/CHATOPS.md`, `docs/OPERATIONS.md`, `docs/CONFIG.md` — verb + config documentation.
- **Operator-visible behavior:**
  - `@<bot> help` lists `brownfield-survey`, `clear-survey` alongside existing verbs.
  - `@<bot> brownfield-survey <repo> [guidance]` produces a curated capability list in a lifecycle thread.
  - `@<bot> send it` in a survey thread initiates batch generation; each iteration produces one spec PR.
  - Status updates on each item; final summary when done.
- **Breaking:** no. New verbs; opt-in via `features.brownfield_survey.enabled` (defaults true). Existing `send it` behavior in audit threads unchanged.
- **Acceptance:** `cargo test` passes; `openspec validate a29-brownfield-survey-and-batch --strict` passes. New tests:
  - Listener parses `brownfield-survey` happy AND refusal paths.
  - Listener routes `send it` to the correct action based on parent thread context.
  - Survey handler produces a valid `BrownfieldSurveyState` from a mocked executor response.
  - Batch handler drains one item per iteration; per-item status transitions through `pending → generating → completed | skipped | failed`.
  - Mid-batch `openspec/specs/<slug>/spec.md` appearing causes the in-flight item to be skipped on its turn.
  - `clear-survey` removes all state files for a repo AND is idempotent (zero-count case works).
