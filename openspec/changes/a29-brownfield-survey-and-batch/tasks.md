## 1. Chatops inbound parsing

- [ ] 1.1 Add `brownfield-survey` AND `clear-survey` to the recognized-verb list.
- [ ] 1.2 Parse `@<bot> brownfield-survey <repo-substring> [optional guidance]` with the existing repo-match rule. Refusals: missing/ambiguous repo, `features.brownfield_survey.enabled: false`. On success: generate `request_id`, post top-level ack, submit `BrownfieldSurveyAction`.
- [ ] 1.3 Parse `@<bot> clear-survey <repo-substring>` AS an operator-recovery verb: submit `ClearSurveyAction`. The polling iteration handles deletion AND replies with the count cleared.
- [ ] 1.4 Extend `send it` parsing: when posted as a reply, look up the parent thread's `ts` AGAINST:
  - Audit threads (per the existing canonical `audit-reply-acts` mechanism — unchanged).
  - Brownfield-survey threads (`BrownfieldSurveyState.thread_ts` set — NEW).
  Submit `BrownfieldBatchAction` for survey threads; canonical audit-triage action for audit threads. If the parent thread matches neither context, post the existing "send it: only valid as a reply in a known thread context" rejection.
- [ ] 1.5 Tests: each parse path (survey happy, survey disabled, send-it-in-survey-thread, send-it-in-audit-thread regression, send-it-outside-known-context, clear-survey happy).

## 2. Control-socket + state plumbing

- [ ] 2.1 Add `BrownfieldSurveyAction`, `BrownfieldBatchAction`, AND `ClearSurveyAction` variants to `autocoder/src/control_socket/actions.rs`.
- [ ] 2.2 New module `autocoder/src/state/brownfield_survey.rs`:
  ```rust
  pub struct BrownfieldSurveyState {
      pub request_id: String,
      pub repo_url: String,
      pub guidance: Option<String>,
      pub head_sha_at_survey: String,
      pub completed_at: chrono::DateTime<Utc>,
      pub thread_ts: String,
      pub channel: String,
      pub items: Vec<SurveyItem>,
      pub status: SurveyStatus,
  }
  pub struct SurveyItem {
      pub id: usize,
      pub slug: String,
      pub summary: String,
      pub scope_in: String,
      pub scope_out: String,
      pub source_modules: Vec<String>,
      pub estimated_complexity: ComplexityBand,
      pub status: ItemStatus,           // pending | generating | completed | skipped | failed
      pub pr_url: Option<String>,
      pub failure_reason: Option<String>,
  }
  pub enum SurveyStatus { Pending, InProgress, Completed }
  pub enum ItemStatus { Pending, Generating, Completed, Skipped, Failed }
  pub enum ComplexityBand { Small, Medium, Large }
  ```
  Atomic-rename writes; per-workspace path `<workspace>/.state/brownfield_surveys/<request_id>.json`.
- [ ] 2.3 Per-repo state extension: `pending_brownfield_survey_requests`, `pending_brownfield_batch_requests` queues.
- [ ] 2.4 Tests: state-file round-trip, queue enqueue/dequeue, atomic-write safety.

## 3. Brownfield-survey polling handler

- [ ] 3.1 New module `autocoder/src/polling/brownfield_survey.rs` exposing `process_pending_brownfield_survey(repo_state, daemon_ctx) -> Result<()>`. Drains at most one survey request per iteration.
- [ ] 3.2 Gather inputs:
  - Survey prompt template via `PromptLoader::load(PromptId::BrownfieldSurvey, &workspace_config)`.
  - `README.md` + the list of `docs/*.md` filenames.
  - Code-symbol overview via `cargo metadata` (Rust) OR a ripgrep pass.
  - Listing of `<spec-root>/specs/` directories (where `<spec-root>` honors `a26`'s `spec_storage.path` when set) — already-specced capabilities are EXCLUDED from the survey.
  - The operator's optional guidance text.
- [ ] 3.3 Invoke the executor in survey mode (`WritePolicy::None`; sandbox: Read, Glob, Grep, Bash read-only).
- [ ] 3.4 Parse the executor's JSON response. Validate:
  - Each item has all required fields.
  - `slug` matches `^[a-z][a-z0-9-]*$` AND is NOT in the already-specced list.
  - `estimated_complexity` is in the allowed set.
  - `items.len() <= features.brownfield_survey.max_capabilities`.
  - On validation failure: post a thread reply naming the failure AND do not persist state.
- [ ] 3.5 Persist `BrownfieldSurveyState` AND post the rendered list to the lifecycle thread:
  ```
  📋 Surveyed capabilities for <repo_url>:
  
  1. `<slug>` — <complexity> — <one-line summary>
     Scope-in:  <short paragraph>
     Scope-out: <short paragraph>
     Source:    <comma-separated source_modules>
  
  2. ... (next item)
  
  Reply with @<bot> send it to batch-generate ALL <N> specs (one per iteration).
  Or re-run @<bot> brownfield-survey <repo> <refined guidance> to refresh.
  ```
- [ ] 3.6 Tests: happy path; validation-failure path; already-specced filtering.

## 4. Brownfield-batch polling handler

- [ ] 4.1 New module `autocoder/src/polling/brownfield_batch.rs` exposing `process_pending_brownfield_batch(repo_state, daemon_ctx) -> Result<()>`.
- [ ] 4.2 On receiving a `BrownfieldBatchAction`:
  - Load the referenced `BrownfieldSurveyState`.
  - If `status` is already `InProgress` OR `Completed`: post a thread reply naming the no-op AND return.
  - Transition `status` to `InProgress` (atomic-rename).
  - Post `✓ Queued <N> capability spec generations. The first will start on the next iteration.` to the lifecycle thread.
- [ ] 4.3 Per-iteration drain: each iteration, after processing other queues AND before standard change processing, the batch handler:
  - Walks all known surveys' states (across all repos this iteration is touching).
  - Finds the first survey whose status is `InProgress`.
  - Within it, finds the first item whose status is `pending`.
  - Re-checks `<spec-root>/specs/<slug>/spec.md` does not exist. If it does, mark the item `skipped` AND return (next iteration picks the next item).
  - Marks the item `generating`.
  - Builds a brownfield prompt invocation per `a23`'s flow, with two additions to the prompt: the item's `scope_in` AND `scope_out` AND `source_modules` are appended as `## Survey context` so the LLM scopes accordingly.
  - Runs the existing brownfield-handler logic (executor with `WritePolicy::OpenSpecOnly`, sandbox, change-directory verification, PR creation).
  - On success: mark `completed`, persist `pr_url`, post `✅ Spec PR opened for \`<slug>\` (M/N done): <pr-url>`.
  - On failure: mark `failed` with `failure_reason`, post `✗ Spec for \`<slug>\` failed: <reason> (continuing with next)`.
  - When all items reach a terminal state: transition survey `status` to `Completed`, post the batch-complete summary.
- [ ] 4.4 Concurrent-batch handling: only ONE survey can be `InProgress` per workspace at a time. If a `BrownfieldBatchAction` arrives for a workspace with an active batch, post `✗ send it: a brownfield batch is already in progress for this workspace (survey <prior-request_id>). Wait for it to finish OR run @<bot> clear-survey <repo> to abort.`
- [ ] 4.5 Tests:
  - Happy path: 3 pending items → 3 iterations, 3 spec PRs, status updates posted, final summary.
  - Mid-batch skip: item 2's `spec.md` appears between iterations → item marked `skipped`; item 3 still processes.
  - Mid-batch failure: item 2's generation fails → marked `failed`; item 3 still processes.
  - Concurrent-batch rejection.
  - All-items-terminal summary posting.

## 5. Clear-survey handler

- [ ] 5.1 On `ClearSurveyAction`: list all files in `<workspace>/.state/brownfield_surveys/`, delete each, reply with the count.
- [ ] 5.2 Tests: clear with multiple surveys; clear with none; idempotent across re-invocations.

## 6. Brownfield-survey prompt template

- [ ] 6.1 Create `prompts/brownfield-survey.md`. Required content:
  - Role statement: "You are surveying an existing codebase to identify the discrete capabilities that warrant their own OpenSpec spec. Your output is a curated list of proposed capabilities; you do NOT write the specs themselves — that happens in a later step."
  - Process: read the code structure, identify cohesive slices of behavior, propose capability boundaries.
  - Output: JSON array per the documented item shape.
  - Anti-noise rules: do NOT propose capabilities for already-specced areas (the prompt receives the already-specced list); do NOT split a single cohesive behavior across multiple capabilities; do NOT bundle unrelated behaviors into one capability; aim for capabilities of small-to-medium complexity (5-10 requirements each); flag genuinely-large capabilities as `large` so the operator can decide whether to split them.
  - Tone: surface candidates for consideration, NOT ranked recommendations. The operator decides what to spec.
  - Cap rule: produce up to `<max_capabilities>` items (passed in via the prompt context).
- [ ] 6.2 Register `PromptId::BrownfieldSurvey` in `a24`'s loader registry.

## 7. Config integration

- [ ] 7.1 In `autocoder/src/config.rs`, extend `features` with `brownfield_survey: { enabled: bool (default true), prompt_path: Option<String>, max_capabilities: usize (default 20, valid 1..=50) }`.
- [ ] 7.2 Tests: defaults; explicit fields; invalid `max_capabilities` fails fast.

## 8. Help-verb output

- [ ] 8.1 Update help output to include `brownfield-survey` (chat-driven workflow) AND `clear-survey` (operator recovery). `send it`'s help text gains "(in audit OR brownfield-survey thread)" qualifier.

## 9. Docs

- [ ] 9.1 `docs/CHATOPS.md`: add `### brownfield-survey` AND `### clear-survey` subsections. Extend `### send it`'s description to name the brownfield-survey-thread context.
- [ ] 9.2 `docs/OPERATIONS.md`: add a "Bootstrapping specs for an existing project" section AND describe the survey → send-it batch loop. Cross-reference `a23`'s single-capability brownfield for when to use which.
- [ ] 9.3 `docs/CONFIG.md`: document `features.brownfield_survey.{enabled, prompt_path, max_capabilities}`.
- [ ] 9.4 Update the `a24` Prompt overrides table to include `PromptId::BrownfieldSurvey`.
- [ ] 9.5 `config.example.yaml`: include the `features.brownfield_survey` block commented out.

## 10. Spec deltas

- [ ] 10.1 `openspec/changes/a29-brownfield-survey-and-batch/specs/chatops-manager/spec.md` ADDs the three verb-related requirements.
- [ ] 10.2 `openspec/changes/a29-brownfield-survey-and-batch/specs/orchestrator-cli/spec.md` ADDs the survey handler, batch handler, AND config requirements.
- [ ] 10.3 `openspec/changes/a29-brownfield-survey-and-batch/specs/project-documentation/spec.md` ADDs the docs requirement.

## 11. Verification

- [ ] 11.1 `cargo test` passes.
- [ ] 11.2 `openspec validate a29-brownfield-survey-and-batch --strict` passes.
- [ ] 11.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
- [ ] 11.4 Manual verification on an unspecced public repo (a small OSS project the operator forked, OR a previously-unspecced internal project):
  - Run `@<bot> brownfield-survey <repo>`. Inspect the resulting capability list for quality.
  - If unsatisfactory, re-run with refined guidance.
  - Run `@<bot> send it` in the thread. Observe one spec PR open per iteration.
  - Inspect a sample of the resulting specs for accuracy AND scope.
  - Try `@<bot> revise <text>` on one of the PRs to verify the standard revision loop still works.
