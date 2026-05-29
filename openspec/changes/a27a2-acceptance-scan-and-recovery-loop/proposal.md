## Why

`a27a0-outcome-tools-replace-stdout-sentinels` made outcome tools the canonical path AND left the stdout-sentinel parser as a one-cycle deprecated fallback. `a27a1-iteration-request-and-continuation-context` added the iteration-request channel so honest scope-overflow has a structured outlet. Together they remove the two excuses an agent had for narrative deferral. Neither change closes the last hole: **an agent that exits without calling any outcome tool AND without emitting any sentinel.**

This is the exact failure mode that produced the recent production-corrosive behavior (`a26-oss-fork-support` task 2.3, `a27-thread-daemon-paths` tasks 1.x–4.x). The agent:

1. Completes some tasks.
2. Leaves the remainder unchecked in `tasks.md`.
3. Writes a narrative "Deferred:" section in the final-answer text.
4. Exits zero.

Today's classifier sees: exit-zero AND a non-empty diff. It returns `ExecutorOutcome::Completed`. The PR opens with unchecked tasks AND the narrative apology buried in the implementation-notes comment. The agent has no incentive to call `outcome_request_iteration` because exiting is easier AND the daemon accepts the result.

This change closes the hole with two complementary mechanisms:

1. **Acceptance scan.** After the implementer subprocess exits AND BEFORE `Completed` is finalized, the daemon scans the workspace's `tasks.md` for unchecked items. If unchecked items are present AND no outcome tool was called, the run is NOT classified as Completed.

2. **Recovery loop.** Instead of failing the run immediately, the daemon re-prompts the same Claude session (via `claude --resume <session_id>`) with a structured message that names the unchecked items AND directs the agent to call exactly one outcome tool. The agent gets one shot to recover: either it marks the tasks complete in `tasks.md` (it had actually finished but forgot to update the file) AND calls `outcome_success`, OR it calls `outcome_request_iteration` (the honest signal), OR it calls `outcome_spec_needs_revision` (sandbox blocker). If the recovery turn ALSO fails to call an outcome tool, the run is classified as `Failed { reason: "acceptance check failed; recovery loop did not produce a structured outcome" }` AND the operator sees both the original AND the recovery transcripts in the run log.

The recovery loop transforms narrative deferral from "the easy path" into "the path that triggers operator-visible attention." The agent's incentive flips: calling `outcome_request_iteration` honestly takes less effort than getting yelled at by the daemon AND failing the run.

This change ALSO removes the deprecated stdout-sentinel parser. By a27a2:

- The MCP outcome tools have been the canonical path for two release cycles (a27a0 + a27a1).
- The recovery loop covers the "agent forgot to call a tool" case structurally (no need for a stdout fallback to catch it).
- Any operator still running a pre-a27a0 implementer prompt has had two cycles to notice the `legacy stdout sentinel matched` deprecation warnings from a27a0.

The parser, the `OUTCOME_SENTINEL_TAG` constant, the placeholder-detection refinement on the post-exit path, AND the deprecation-warning log line ALL go away. `prompts/implementer.md`'s DEPRECATED section (the residual stdout-sentinel example retained for the deprecation window in a27a0) ALSO goes away.

## What Changes

**Acceptance scan applies ONLY to the implementer flow** (`Executor::run` against a real change directory). It does NOT apply to `run_revision` (no `tasks.md` to scan — the change is archived; the workspace operates on a PR diff), `run_triage`, `run_chat_triage`, `run_brownfield_draft`, `run_scout`, OR `run_changelog`. Those flows have different success criteria (audit findings produced, triage decision reached, scout report written, etc.) AND retain today's exit-status-based classification.

**Acceptance scan implementation.** After `classify_outcome` returns `ExecutorOutcome::Completed` for an implementer run, `Executor::run` reads `<workspace>/openspec/changes/<change>/tasks.md` AND counts unchecked tasks (lines matching `^- \[ \]` at any indentation). The scan ignores nested checklists inside fenced code blocks, NUMBERED-task sub-bullets that are not themselves task entries, AND the canonical task-id format prefix patterns. If the unchecked count is zero, the run is finalized as `Completed`. If the unchecked count is non-zero AND no `outcome_*` tool call was recorded against this `(workspace_basename, change)` during the run, the recovery loop fires.

If `tasks.md` is absent OR unparseable, the scan SKIPS (treats as if zero unchecked) — defensive default; an absent or corrupt tasks.md is its own diagnostic AND a change without tasks.md is exotic enough that the polling loop's existing validation will catch it elsewhere.

**Recovery-loop prompt content.** The recovery turn appends a single user-message to the existing session via `claude --resume <session_id>` with the following structure:

```
Acceptance check failed: your run ended without finishing the change.

tasks.md still has unchecked items:
  - <task_id_1>: <task text>
  - <task_id_2>: <task text>
  ...

You did not call any outcome tool to conclude the session. Narrative
"Deferred:" notes in the final-answer text are not accepted; the
daemon enforces a structured outcome.

Decide which of the following applies AND call the corresponding tool:

1. The unchecked items are actually done in code — you forgot to mark
   tasks.md. Update tasks.md to check them, then call:
       outcome_success({ final_answer: "..." })

2. You completed part AND want another iteration to finish the rest.
   Call:
       outcome_request_iteration({
         completed_tasks: [...],
         remaining_tasks: [<unchecked list>],
         reason: "<concrete blocker>"
       })

3. The unchecked items are unimplementable in this sandbox. Call:
       outcome_spec_needs_revision({
         unimplementable_tasks: [...],
         revision_suggestion: "..."
       })

Do NOT exit without calling exactly one outcome tool. If you call one
AND it returns a validation error, fix the error AND retry the call.
```

**Recovery-loop cap: 1 retry.** A single recovery turn is permitted per run. If the recovery turn ALSO completes without an outcome tool call, the run is classified as `Failed { reason: "acceptance check failed; recovery loop did not produce a structured outcome" }`. The recovery turn's transcript is appended to the per-change run log so the operator can review the agent's reasoning. The cap prevents an infinite recovery loop on a genuinely confused agent AND keeps the operator-attention budget bounded.

**Session-resume mechanism.** The recovery turn uses `claude --resume <session_id>` (the same mechanism used by today's `Executor::resume` for the AskUser flow). The session_id is captured from the JSON-streaming `result` event of the original run AND threaded into the recovery launch via `ResumeHandle`-equivalent plumbing. The recovery turn's MCP `.mcp.json` is the same as the original run's (outcome tools available, askuser available, query_canonical_specs available).

**Anti-narrative-deferral text in the implementer prompt.** `prompts/implementer.md` gains an explicit section near the top:

> Do NOT narrate "Deferred:" sections in your final-answer text. The daemon enforces a structured outcome via the outcome tools. If you have remaining work, call `outcome_request_iteration`. If a task is genuinely unimplementable, call `outcome_spec_needs_revision`. If you finished, call `outcome_success`. Narrative deferral was previously the path of least resistance AND produced corrosive PR shipping; the acceptance scan now catches it AND triggers a recovery turn that fails the run if you persist.

The text is operator-visible motivation, not just instruction — agents trained on the prompt context learn the failure mode AND the recovery mechanism, which calibrates the "should I just exit?" decision toward the structured path.

**Legacy stdout-sentinel removal.** The following are deleted:

- `autocoder/src/executor/claude_cli.rs`:
  - The `OUTCOME_SENTINEL_TAG` constant (`"=== AUTOCODER-OUTCOME ==="`).
  - The `extract_outcome_sentinel` function.
  - The `try_parse_spec_needs_revision` function.
  - The `excerpt_for_reason` helper (used only by the sentinel parse-failure reason path).
  - The classifier's stdout-sentinel branch AND the `legacy stdout sentinel matched` deprecation warning.
  - The test fixtures asserting stdout-sentinel parsing.
- `prompts/implementer.md`:
  - The DEPRECATED section retained in a27a0 for the deprecation window.
  - Any remaining reference to the `=== AUTOCODER-OUTCOME ===` block.

The implementer prompt's "Outcome tools" section (added in a27a0) AND the worked-example documentation for `outcome_spec_needs_revision`, `outcome_request_iteration`, AND `outcome_success` (the per-tool one-line purpose + when-to-use) remain.

## Impact

- **Affected specs:**
  - `executor` — REMOVED: the canonical "Sentinel emission instructions in the implementer prompt include a concrete worked example AND a self-check hint" requirement (its premise — the stdout sentinel format — no longer exists). REMOVED: the canonical "Timeout classification takes precedence over sentinel extraction; sentinel scan is scoped to deliberate-emission content" requirement (its scope-narrowing is moot — no sentinel scan exists; timeout precedence is preserved as part of the new classifier-ordering requirement below). REMOVED: a27a0's "Legacy stdout-sentinel scan is deprecated; matches emit an operator-visible warning during the transition cycle" requirement (the deprecation window has closed). MODIFIED: a27a0's "Tool-recorded outcomes take precedence over all heuristic classification in `classify_outcome`" requirement — the ordering drops the legacy stdout-sentinel scan step; timeout precedence remains. MODIFIED: a27a0's "Implementer prompt documents the outcome tools by name AND uses them as the canonical end-of-run signal" requirement — the DEPRECATED-stdout-section scenario is dropped (the section itself is removed). ADDED: acceptance scan for the implementer flow, recovery loop primitive, anti-narrative-deferral prompt discipline.
- **Affected code:**
  - `autocoder/src/executor/claude_cli.rs` — `Executor::run` gains a post-classification acceptance-scan step AND the recovery-loop branch. The classifier's stdout-sentinel branch AND related helpers are deleted. The classifier ordering simplifies to: consume_outcome → askuser marker → timeout → exit status → diff-presence-or-Layer-2-heuristic-or-Completed.
  - `prompts/implementer.md` — anti-narrative-deferral section added; DEPRECATED sentinel section deleted; deprecation note on the outcome-tools section deleted.
  - New helper module (or function) for the tasks.md unchecked-task scan. Scope: ~50 lines of parsing + tests against known-good AND known-bad tasks.md fixtures.
- **Operator-visible behavior:**
  - Implementer runs that ship with unchecked tasks AND no outcome tool call trigger the recovery loop. `journalctl` shows `acceptance check failed; entering recovery turn for change <X>` followed by the recovery transcript.
  - Recovery turns produce one of: a clean `Completed` (tasks marked + `outcome_success`), `IterationRequested` (honest scope-overflow), `SpecNeedsRevision` (sandbox blocker), OR `Failed { reason: "acceptance check failed; recovery loop did not produce a structured outcome" }`.
  - No more `legacy stdout sentinel matched` deprecation warnings. The log line is removed.
  - Non-implementer flows (`run_triage`, `run_chat_triage`, `run_brownfield_draft`, `run_scout`, `run_changelog`, `run_revision`) are UNCHANGED. Their classification path skips the acceptance scan entirely.
- **Backward compatibility:** the legacy stdout sentinel parser is REMOVED. An operator still running a pre-a27a0 implementer prompt would emit stdout sentinels that the daemon no longer parses; the acceptance scan would catch the unchecked tasks AND trigger the recovery loop. The recovery turn would direct the agent to call outcome tools, AND on a current Claude CLI the agent would do so. Effectively the recovery loop IS the backward-compat path for stale prompts.
- **Dependencies:** a27a0 AND a27a1 must be merged first. This change removes a27a0's deprecation-warning requirement AND modifies a27a0's classifier-ordering requirement.
- **Acceptance:** `cargo test` passes; `openspec validate a27a2-acceptance-scan-and-recovery-loop --strict` passes. Tests:
  - Implementer run with all tasks checked AND `outcome_success` called: classifies as Completed; no acceptance scan triggered.
  - Implementer run with unchecked tasks AND `outcome_success` called: classifies as Completed (agent's structured signal wins over the acceptance scan; the agent chose to call success even with unchecked items, AND that's its call). No recovery loop.
  - Implementer run with unchecked tasks AND no outcome tool call: triggers recovery loop. Recovery turn calls `outcome_success` → final classification is Completed. Run log contains both transcripts.
  - Implementer run with unchecked tasks AND no outcome tool call: triggers recovery loop. Recovery turn calls `outcome_request_iteration` → final classification is IterationRequested (with iteration_number from a27a1's marker increment).
  - Implementer run with unchecked tasks AND no outcome tool call: triggers recovery loop. Recovery turn ALSO does not call any outcome tool → final classification is Failed with the canonical reason text.
  - `run_revision` with unchecked-tasks-in-archive AND no outcome tool call: acceptance scan SKIPS; classification proceeds via the normal post-classify path (Completed or otherwise per today's behavior).
  - `run_triage` / `run_scout` / `run_brownfield_draft` / `run_changelog`: acceptance scan SKIPS regardless of tasks.md content (these flows don't operate against a per-change tasks.md).
  - tasks.md parsing: unchecked-task counter correctly identifies `- [ ] task`, `  - [ ] subtask`, ignores `- [x] done`, ignores `[ ]` inside fenced code blocks.
  - Legacy stdout sentinel: an implementer run that emits `=== AUTOCODER-OUTCOME ===\n{...}` in stdout AND has unchecked tasks AND does not call any outcome tool triggers the acceptance scan + recovery loop (the legacy sentinel is no longer parsed). The recovery turn directs the agent to call an outcome tool.
