# Tasks

## 1. Acceptance-scan helper

- [ ] 1.1 Add a new helper (`autocoder/src/executor/acceptance_scan.rs` OR a function in `claude_cli.rs`) that parses `tasks.md` line-by-line AND returns a `Vec<UncheckedTask>` where `UncheckedTask` carries the line number AND the trailing text (everything after `- [ ] `).
- [ ] 1.2 The parser tracks fenced-block state (` ``` ` toggles in/out of "skip" mode) AND ignores checkbox-shaped content inside fenced blocks.
- [ ] 1.3 The parser counts `^[ \t]*- \[ \] ` AS unchecked, AND `^[ \t]*- \[x\] ` (case-insensitive on `x`) AS checked-and-ignored.
- [ ] 1.4 Unit-test: a tasks.md with three unchecked AND four checked items returns three UncheckedTask entries with the correct trailing text.
- [ ] 1.5 Unit-test: a tasks.md with a fenced code block containing `- [ ] foo` returns zero unchecked (fenced content ignored).
- [ ] 1.6 Unit-test: a tasks.md with no checkbox-shaped content returns zero unchecked.
- [ ] 1.7 Unit-test: an absent tasks.md OR unreadable tasks.md returns zero unchecked (defensive default — the caller treats this as scan-skipped).

## 2. Acceptance-scan dispatch in `Executor::run`

- [ ] 2.1 After `classify_outcome` returns AND BEFORE finalizing the outcome, branch on the variant:
  - For `Completed`: invoke the acceptance scan AND the recovery-loop dispatcher (if applicable).
  - For `AskUser`, `Failed`, `SpecNeedsRevision`, `IterationRequested`: bypass the acceptance scan AND return the classified outcome unchanged.
- [ ] 2.2 The acceptance scan ONLY runs in `Executor::run`. The other entry points (`run_revision`, `run_triage`, `run_chat_triage`, `run_brownfield_draft`, `run_scout`, `run_changelog`) do NOT invoke the scan.
- [ ] 2.3 When the scan returns zero unchecked: finalize as `Completed` (today's behavior).
- [ ] 2.4 When the scan returns >0 unchecked items AND no outcome tool was called during the run: fire the recovery loop.

## 3. Recovery-loop primitive

- [ ] 3.1 Capture the session_id from the original run's JSON-streaming `result` event AND thread it into `Executor::run`'s post-classification context. The session_id is the input to `claude --resume`.
- [ ] 3.2 Build the recovery-turn prompt content from the canonical text in the executor capability deltas, substituting `<task_id>: <task text>` for each unchecked item.
- [ ] 3.3 Launch `claude --resume <session_id>` with the recovery prompt as the user-message input AND the same MCP config as the original run.
- [ ] 3.4 The recovery turn's subprocess runs with a fresh wall-clock budget equal to the per-run timeout (a fresh start; documented in design.md).
- [ ] 3.5 Append the recovery turn's stdout/stderr to the existing per-change run log with a clear divider:
  - In the summary log (`.log`): `=== RECOVERY TURN ===` followed by the recovery turn's `final_answer`.
  - In the stream log (`.stream.log`): `=== RECOVERY TURN ===` followed by the recovery turn's `[tool_use]` / `[tool_result]` / `[assistant]` lines.
- [ ] 3.6 After the recovery turn exits, call `classify_outcome` on its subprocess result.
- [ ] 3.7 If the recovery turn produced a structured outcome (via consume_outcome): return that outcome. Acceptance scan is NOT re-run.
- [ ] 3.8 If the recovery turn produced no structured outcome: return `ExecutorOutcome::Failed { reason: "acceptance check failed; recovery loop did not produce a structured outcome" }`.
- [ ] 3.9 Recovery loop fires AT MOST ONCE per `Executor::run` invocation. A recovery turn that itself produces unchecked tasks AND no outcome tool call does NOT fire a second recovery turn.

## 4. Legacy stdout-sentinel removal

- [ ] 4.1 Delete `OUTCOME_SENTINEL_TAG` constant from `claude_cli.rs`.
- [ ] 4.2 Delete `extract_outcome_sentinel` function.
- [ ] 4.3 Delete `try_parse_spec_needs_revision` function.
- [ ] 4.4 Delete `excerpt_for_reason` helper (used only by the sentinel parse-failure reason path).
- [ ] 4.5 Delete the classifier's stdout-sentinel branch (the `if let Some(source) = sentinel_source ...` block AND its dispatch).
- [ ] 4.6 Delete the `legacy stdout sentinel matched` `tracing::warn!` call site.
- [ ] 4.7 Delete the test fixtures that asserted stdout-sentinel parsing (`parse_spec_needs_revision_sentinel_round_trips` AND any others built around the sentinel JSON).
- [ ] 4.8 Delete the test fixtures that asserted the legacy-deprecation warning (a27a0 added these; they're dead in a27a2).
- [ ] 4.9 Verify the classifier's new shape is exactly: consume_outcome → AskUser → timeout → exit status → diff-presence/Layer-2/Completed. Add an explicit ordering test (one fixture per branch).

## 5. Prompt updates

- [ ] 5.1 Add the anti-narrative-deferral section to `prompts/implementer.md` near the top (above the existing pre-flight outcome-tool section). Text follows the canonical wording in the executor capability deltas.
- [ ] 5.2 Delete the DEPRECATED `=== AUTOCODER-OUTCOME ===` sentinel section that a27a0 retained for the deprecation window.
- [ ] 5.3 Delete any remaining references to `=== AUTOCODER-OUTCOME ===` from `prompts/implementer.md`.
- [ ] 5.4 The "Outcome tools" section retains all three tools (`outcome_success`, `outcome_spec_needs_revision`, `outcome_request_iteration`) with their names AND one-line purposes. No further schema inlining (per a27a0's documentation discipline).

## 6. Integration tests

- [ ] 6.1 Integration test: implementer run with all tasks checked AND `outcome_success` called → Completed, no recovery.
- [ ] 6.2 Integration test: implementer run with unchecked tasks AND `outcome_success` called → Completed, no recovery (agent's signal wins).
- [ ] 6.3 Integration test: implementer run with unchecked tasks AND no outcome tool call → recovery turn fires. Recovery calls `outcome_success` → final Completed.
- [ ] 6.4 Integration test: same setup, recovery calls `outcome_request_iteration` → final IterationRequested (with iteration_number computed per a27a1 cap rules).
- [ ] 6.5 Integration test: same setup, recovery turn ALSO produces no outcome tool call → final Failed with the canonical reason.
- [ ] 6.6 Integration test: `run_revision` with unchecked tasks AND no outcome tool call → no recovery (the scan is skipped); classification proceeds per today's path.
- [ ] 6.7 Integration test: `run_triage` / `run_scout` / `run_brownfield_draft` / `run_changelog`: acceptance scan does not fire regardless of workspace content.
- [ ] 6.8 Integration test: an implementer run that emits a legacy `=== AUTOCODER-OUTCOME ===` stdout block AND has unchecked tasks AND does not call any outcome tool → the stdout sentinel is NOT parsed (no Completed/SpecNeedsRevision shortcut); the acceptance scan fires; recovery turn directs the agent to call an outcome tool.

## 7. Validation

- [ ] 7.1 `cargo test` passes.
- [ ] 7.2 `cargo clippy` produces no NEW warnings against the existing baseline.
- [ ] 7.3 `openspec validate a27a2-acceptance-scan-and-recovery-loop --strict` passes.
