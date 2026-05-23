## 1. ExecutorOutcome variant

- [x] 1.1 Add `SpecNeedsRevision { unimplementable_tasks: Vec<UnimplementableTask>, revision_suggestion: String }` to the `ExecutorOutcome` enum (wherever it currently lives — likely `autocoder/src/executor/mod.rs`). Add the `UnimplementableTask` struct alongside with fields `task_id: String`, `task_text: String`, `reason: String`. Both types derive `Debug`, `Clone`, `PartialEq` so tests can pattern-match cleanly.
- [x] 1.2 Update the `Executor` trait's documentation comment to mention the new variant: "Returns `SpecNeedsRevision` when one or more tasks in tasks.md require capabilities outside the executor's sandbox. The agent flags upfront, before starting implementation."

## 2. Claude CLI executor: outcome parsing

- [x] 2.1 In `autocoder/src/executor/claude_cli.rs`, extend the outcome-sentinel parser. Today the parser handles whatever tags exist (`AUTOCODER-OUTCOME` JSON block, AskUser sentinel, etc.). Add recognition for `{"type":"spec_needs_revision","unimplementable_tasks":[...],"revision_suggestion":"..."}`. Parse into the new `SpecNeedsRevision` variant.
- [x] 2.2 Tolerate partial/malformed payloads: if the JSON doesn't deserialize (missing fields, unknown type), log a WARN naming the parse failure and fall back to treating the run as Failed with reason "agent emitted unparseable SpecNeedsRevision sentinel: <excerpt>". An unparseable sentinel is the agent's fault; we shouldn't crash the daemon.
- [x] 2.3 Tests:
  - `parse_spec_needs_revision_sentinel_round_trips` — feed the parser a well-formed sentinel; assert the variant + fields.
  - `parse_spec_needs_revision_missing_required_field_falls_back_to_failed` — feed a sentinel missing `unimplementable_tasks`; assert Failed with parse-error reason.
  - `parse_spec_needs_revision_with_empty_task_list_treated_as_invalid` — empty list shouldn't trigger the outcome; fall back.

## 3. Implementer prompt template update

- [x] 3.1 Locate the bundled implementer prompt template (search for `include_str!("../../prompts/implementer.md")` or similar in the executor module). Add a new section near the top of the prompt — BEFORE the "implement these tasks" instruction — titled "Pre-flight: flag unimplementable tasks." The section text:

  > Before starting any implementation, scan tasks.md. If any task requires capabilities outside your sandbox, DO NOT begin work. Examples of unimplementable tasks:
  >
  > - `sudo` against a real host (useradd, systemctl, apt install, etc.)
  > - Tools known to be absent (actionlint, shellcheck, jq unless explicitly available — verify via `command -v <tool>`)
  > - Real GitHub pushes (push tags, force-push to upstream branches not under your delegation)
  > - Browser interactions (`claude auth login`, OAuth flows, manual UI verification)
  > - VM or container spin-up (`docker run`, `vagrant up`, etc.)
  > - Smoke tests on real hardware or specific OS versions you don't have ("verify on Debian 12", "test on M2 Mac")
  > - Manual external observation ("confirm the deploy works in browser", "check the Grafana dashboard")
  >
  > If you find one or more such tasks, emit this sentinel at end-of-run and DO NOT modify any files:
  >
  > ```
  > === AUTOCODER-OUTCOME ===
  > {"type":"spec_needs_revision","unimplementable_tasks":[
  >   {"task_id":"<id-from-tasks-md>","task_text":"<verbatim quote>","reason":"<one-line why>"}
  > ],"revision_suggestion":"<free-form text describing what to change in tasks.md to make the spec verifiable>"}
  > ```
  >
  > The operator will review your assessment, edit tasks.md, and re-trigger the change. If you judge a task implementable when this section's examples suggest you flag it, proceed normally — your judgment about the specific task wins, but the bias should be conservative.

- [x] 3.2 The "bias should be conservative" framing acknowledges the false-positive risk. Better to flag a task the operator overrides than to push through an unimplementable one.

## 4. Marker file + queue filtering

- [x] 4.1 In `autocoder/src/queue.rs`, define `const NEEDS_REVISION_FILE: &str = ".needs-spec-revision.json";`. Add `pub fn is_needs_spec_revision_marked(workspace: &Path, change: &str) -> bool` returning whether the marker exists at `<workspace>/openspec/changes/<change>/.needs-spec-revision.json`.
- [x] 4.2 Extend `list_pending`'s existing per-change filter (which currently excludes perma-stuck-marked changes) to also exclude needs-spec-revision-marked ones. The simplest implementation: combine the two checks via OR. Add a unit test confirming a change with `.needs-spec-revision.json` is excluded just like a perma-stuck-marked change.
- [x] 4.3 In `autocoder/src/workspace.rs::ensure_initialized`, append `.needs-spec-revision.json` to the `.git/info/exclude` chain alongside `.perma-stuck.json` (which was added by `recover-dirty-workspace-mid-iteration`). Update the existing exclude test to assert the new line is present after init.

## 5. Marker writer

- [x] 5.1 In a new module `autocoder/src/spec_revision.rs` (or alongside `perma_stuck.rs`, following whatever pattern that module uses), add `pub fn write_marker(workspace: &Path, change: &str, outcome: &SpecNeedsRevisionDetail) -> Result<()>`. The function serializes the outcome details to JSON per the schema in the proposal, with `marked_at: Utc::now()` and the static `operator_action` sentence.
- [x] 5.2 Tests: round-trip a marker file (write then read) and assert all fields match.

## 6. Alert wiring

- [x] 6.1 Add `SpecNeedsRevision` variant to `AlertCategory` in `autocoder/src/alert_state.rs`. `label()` returns `"spec needs revision"`. Existing 24h throttle applies via `handle_predictable_failure`.
- [x] 6.2 Add `async fn maybe_post_spec_revision_alert(...)` to `polling_loop.rs` mirroring the existing perma-stuck-alert helper. The body assembly takes the change name, the list of flagged tasks, and the suggestion, and renders the exact format shown in the proposal. Best-effort post; WARN on failure; do not propagate.
- [x] 6.3 The function is called from the outcome-handling site (task 7.1) after the marker is written.

## 7. Polling-loop outcome handling

- [x] 7.1 In `handle_outcome` (or wherever `ExecutorOutcome` is dispatched), add an arm for `SpecNeedsRevision`. Sequence: (a) `queue::unlock(workspace, change)` to remove the `.in-progress` file (consistent with other Failed-equivalent outcomes); (b) `spec_revision::write_marker(workspace, change, &outcome_detail)`; (c) `maybe_post_spec_revision_alert(...)` with chatops ctx; (d) return `QueueStep::Failed { reason: "spec needs revision; marker written and operator alerted" }` so the queue walk halts (matching `halt-queue-walk-on-non-archive`).
- [x] 7.2 Important: SpecNeedsRevision must NOT increment the perma-stuck counter. It's an operator-action state, not a repeat-execution-failure state. The marker handles exclusion directly; the counter is irrelevant here.
- [x] 7.3 Tests:
  - `spec_needs_revision_writes_marker_and_alerts_and_halts_queue` — executor returns the new outcome; assert marker file exists at the expected path, exactly one chatops post fires under `SpecNeedsRevision` category, queue walk halts (no later changes processed).
  - `spec_needs_revision_does_not_increment_perma_stuck_counter` — `failure_state.json` for the change shows no count after the outcome fires.
  - `change_with_revision_marker_excluded_from_list_pending` — pre-place the marker; run an iteration; assert the executor is never invoked for that change.
  - `marker_removed_re_enables_change` — pre-place the marker, run an iteration (executor not called), then delete the marker, run another iteration; assert the executor IS called the second time.

## 8. README documentation

- [x] 8.1 Add a subsection to "Operating Notes" titled "Spec marked as needing revision." Content explains: what triggers the marker, what's in the marker file, what the chatops alert looks like, the operator workflow (edit tasks.md, commit, delete the marker), and the false-positive escape hatch (if you disagree with the agent's judgment, just delete the marker without editing — the change re-enters pending).
- [x] 8.2 Cross-reference from the existing perma-stuck section so operators see the two patterns are siblings.

## 9. Spec delta

- [x] 9.1 Add the ADDED requirement under `orchestrator-cli` named "Spec-needs-revision executor outcome + marker" per the proposal.

## 10. Verification

- [x] 10.1 `cargo test` passes.
- [x] 10.2 `openspec validate spec-needs-revision-outcome --strict` passes.
- [x] 10.3 Manual smoke test of the prompt update is NOT in the task list — that would be exactly the kind of buck-pass this spec is trying to prevent. The prompt update lands; the next time the agent encounters an unimplementable task in real production work, the contract activates. If the agent's flagging is too eager or too lax, the operator updates the prompt template via a follow-up change.
