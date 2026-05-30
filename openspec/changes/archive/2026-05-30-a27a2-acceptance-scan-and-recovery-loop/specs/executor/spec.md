## REMOVED Requirements

### Requirement: Sentinel emission instructions in the implementer prompt include a concrete worked example AND a self-check hint

The premise of this requirement — the stdout `=== AUTOCODER-OUTCOME ===` sentinel format documented in `prompts/implementer.md` — no longer exists. Outcome signaling moved to MCP tools in `a27a0-outcome-tools-replace-stdout-sentinels`; the deprecation window covered by `a27a0`'s `Legacy stdout-sentinel scan is deprecated...` requirement closes with this change. The bundled implementer prompt no longer contains a sentinel section, so the structural-elements discipline (substitution instruction + worked example + self-check hint) has no anchor. The `a27a0` requirement `Implementer prompt documents the outcome tools by name AND uses them as the canonical end-of-run signal` (after this change's MODIFIED block) is the canonical replacement for prompt-content discipline AND covers the equivalent quality bar via the tool-documentation discipline.

### Requirement: Timeout classification takes precedence over sentinel extraction; sentinel scan is scoped to deliberate-emission content

The premise of this requirement — that a stdout-sentinel scan exists AND its scope must be narrowed to deliberate-emission content — no longer exists. The sentinel scan is removed in this change (per the `a27a0` ordering requirement's MODIFIED form below, which drops the stdout-sentinel step). Timeout precedence as a classifier-ordering concern is preserved by the `a27a0` `Tool-recorded outcomes take precedence over all heuristic classification in classify_outcome` requirement (after this change's MODIFIED block): the ordering still checks `outcome.timed_out` BEFORE proceeding to exit-status classification, AND the same narrowing rationale (a timed-out run did not reach deliberate end-of-run) carries forward implicitly.

### Requirement: Legacy stdout-sentinel scan is deprecated; matches emit an operator-visible warning during the transition cycle

The deprecation window opened by `a27a0` closes with this change. The stdout-sentinel scan AND its deprecation-warning log line are removed. Operators running stale implementer prompts that still emit `=== AUTOCODER-OUTCOME ===` blocks now hit the acceptance scan + recovery loop introduced below; the recovery turn directs the agent to call the canonical outcome tools instead.

## MODIFIED Requirements

### Requirement: Tool-recorded outcomes take precedence over all heuristic classification in `classify_outcome`

The executor's outcome-dispatch path (`classify_outcome` in the CLI-wrapping executor backend) SHALL consult the daemon's outcome store via a `consume_outcome` control-socket action BEFORE applying any other classification step. The ordering is:

1. **Tool-recorded outcome lookup.** The classifier sends a `consume_outcome` action keyed by `(workspace_basename, change)`. When the daemon returns a recorded outcome:
   - A `Success` variant maps to `ExecutorOutcome::Completed { final_answer }` using the recorded `final_answer`.
   - A `SpecNeedsRevision` variant maps to the existing `ExecutorOutcome::SpecNeedsRevision { ... }` shape.
   - An `IterationRequest` variant maps to `ExecutorOutcome::IterationRequested { ... }` per the `a27a1` cap-enforcement rules.
   - The classifier returns the mapped outcome immediately. No further heuristic is applied.
2. **AskUser marker check** (unchanged from canonical executor behavior).
3. **Timeout precedence.** When `outcome.timed_out` is `true` AND no tool-recorded outcome was returned, the classifier returns `Failed { reason: "timeout" }` (OR the canonical timeout-reason format).
4. **Exit-status path** (unchanged).
5. **Layer-2 stdout heuristic + Completed fallback** (unchanged).

The legacy stdout-sentinel scan that previously sat between steps 3 AND 4 (per the original `a27a0` ordering) is REMOVED in this change. The acceptance scan + recovery loop introduced below replace its role: the only narrative-deferral path the classifier still produces (Completed via diff-presence heuristic) is gated by the acceptance scan in `Executor::run`'s post-classification step.

The precedence rule is anchored in the semantics of the signal: a tool-recorded outcome is the agent's deliberate, schema-validated end-of-run emission. It is more authoritative than ANY inferred state (timeout flag, exit status, stdout content). A run that called an outcome tool AND subsequently timed out is classified by the outcome, not the timeout.

When the daemon's `consume_outcome` action returns `None` (no outcome was recorded), the classifier proceeds to step 2 AND the existing canonical behavior is preserved exactly.

#### Scenario: Tool-recorded `Success` outcome takes precedence over diff-presence heuristic
- **WHEN** the classifier runs for a change whose daemon outcome store contains a `Success` outcome from a prior `outcome_success` tool call
- **AND** the workspace has a non-empty diff (would otherwise trigger today's Completed-via-diff-presence path with possibly different `final_answer` content)
- **THEN** the classifier returns `ExecutorOutcome::Completed { final_answer: <recorded final_answer> }`
- **AND** the recorded `final_answer` (NOT a heuristically-extracted alternative) is the outcome's content
- **AND** the daemon's outcome store entry for this `(workspace_basename, change)` is cleared (drained by `consume_outcome`)

#### Scenario: Tool-recorded `SpecNeedsRevision` outcome takes precedence over timeout
- **WHEN** the classifier runs for a change whose daemon outcome store contains a `SpecNeedsRevision` outcome (the agent called `outcome_spec_needs_revision` AND then was killed by the wall-clock timeout before clean exit)
- **AND** `outcome.timed_out` is `true`
- **THEN** the classifier returns `ExecutorOutcome::SpecNeedsRevision { ... }` populated from the recorded payload
- **AND** the timeout flag is NOT used
- **AND** no `Failed { reason: "timeout" }` outcome is produced

#### Scenario: Absent tool-recorded outcome falls through to AskUser → timeout → exit-status path
- **WHEN** the classifier runs for a change whose daemon outcome store contains no entry (the agent did not call any outcome tool)
- **AND** `outcome.timed_out` is `false`
- **AND** no AskUser marker is present
- **THEN** the classifier's `consume_outcome` call returns `None`
- **AND** the classifier proceeds through the simplified ordering (AskUser → timeout → exit status → diff-presence/Completed)
- **AND** no stdout-sentinel scan is attempted (the legacy path has been removed)

### Requirement: Implementer prompt documents the outcome tools by name AND uses them as the canonical end-of-run signal

The bundled `prompts/implementer.md` template SHALL contain an "Outcome tools" section that:

- Names all three outcome tools: `outcome_success`, `outcome_spec_needs_revision`, AND `outcome_request_iteration`.
- Provides a one-line purpose statement for each tool.
- Directs the agent to call `outcome_success` (with the agent's end-of-run summary as `final_answer`) at the end of a successful implementation run, BEFORE exiting.
- Directs the agent to call `outcome_spec_needs_revision` for the pre-flight unimplementable-task case.
- Directs the agent to call `outcome_request_iteration` (per `a27a1`) when honest scope-overflow means another iteration is needed.
- Notes that input-validation errors from any outcome tool are recoverable: the model receives the error as the tool-call result AND can retry the call with corrected fields in the same session.

The section SHALL NOT inline the full input schemas; the MCP `tools/list` response is the canonical schema source AND duplicating it in the prompt creates a maintenance hazard. Tool names + one-line purposes are sufficient: a model that knows the tool exists AND its purpose can attempt the call AND converge via tool-error feedback if its argument shape is wrong.

The legacy stdout-sentinel section (the `=== AUTOCODER-OUTCOME ===` block AND its DEPRECATED-prefixed retention from `a27a0`) is REMOVED. The implementer prompt SHALL NOT contain any reference to `=== AUTOCODER-OUTCOME ===`, the legacy `spec_needs_revision` JSON sentinel format, OR the substitution-instruction-plus-worked-example structural-elements discipline that bound the sentinel section.

Operator-customizable override prompts (loaded via the uniform `PromptLoader` per `a24`'s spec) MAY use any structure the operator prefers — the canonical rule binds the bundled default only.

#### Scenario: Bundled prompt names all three outcome tools
- **WHEN** a maintainer reads `prompts/implementer.md`
- **THEN** the prompt contains an "Outcome tools" section
- **AND** the section names `outcome_success`, `outcome_spec_needs_revision`, AND `outcome_request_iteration`
- **AND** each tool has a one-line purpose statement

#### Scenario: Bundled prompt's outcome-tool example deserializes cleanly
- **WHEN** an automated test extracts any JSON-shaped example from the prompt's outcome-tool sections AND deserializes it into the corresponding tool-argument Rust type
- **THEN** the deserialization succeeds without error
- **AND** every string field contains a concrete value (no angle-bracket markers, no template variables)

#### Scenario: Stdout-sentinel section is removed from the bundled prompt
- **WHEN** a maintainer reads `prompts/implementer.md`
- **THEN** the prompt contains NO occurrence of the string `=== AUTOCODER-OUTCOME ===`
- **AND** the prompt contains NO section describing the legacy `spec_needs_revision` stdout-block format
- **AND** the prompt contains NO DEPRECATED-prefixed retention of the legacy section

## ADDED Requirements

### Requirement: Acceptance scan rejects implementer runs that ship unchecked tasks without a structured outcome

`Executor::run` (the implementer-first-pass entry point, against a real change directory) SHALL apply an acceptance scan AFTER `classify_outcome` returns AND BEFORE finalizing the outcome. The scan SHALL fire ONLY when:

1. The classified outcome is `ExecutorOutcome::Completed`.
2. The run did NOT produce a tool-recorded outcome (`consume_outcome` returned `None` during classification — i.e. the agent exited without calling any outcome tool).

If either condition does NOT hold, the scan SHALL be skipped AND the classified outcome SHALL be returned unchanged.

When the scan fires, it SHALL count unchecked tasks in `<workspace>/openspec/changes/<change>/tasks.md`. Parsing rules:

- Lines matching `^[ \t]*- \[ \] ` outside fenced code blocks count as unchecked.
- Lines matching `^[ \t]*- \[x\] ` (case-insensitive on `x`) count as checked AND are ignored.
- Content inside ` ``` ` fenced blocks is ignored entirely.
- The parser extracts the trailing text (everything after `- [ ] `) for each unchecked line, paired with the source line number.

If `tasks.md` is absent OR unparseable, the scan SHALL treat the unchecked count as zero (defensive default — absent/corrupt tasks.md is its own diagnostic AND the polling loop's existing validation catches it elsewhere).

When the unchecked count is zero, the classified `Completed` outcome SHALL be returned unchanged. When the unchecked count is non-zero, the recovery loop (per the requirement below) SHALL fire.

The acceptance scan SHALL NOT fire in `run_revision`, `run_triage`, `run_chat_triage`, `run_brownfield_draft`, `run_scout`, OR `run_changelog`. Those flows do not operate against a per-change `tasks.md` in the implementer sense; their existing classification path is preserved.

#### Scenario: All tasks checked AND outcome_success called — no scan triggered
- **WHEN** `Executor::run` finishes a run where the agent called `outcome_success` AND `tasks.md` has zero unchecked items
- **THEN** the classified outcome is `Completed` via the tool-outcome precedence path
- **AND** the acceptance scan does NOT fire (condition 2: tool-recorded outcome was produced)
- **AND** the finalized outcome is `Completed` unchanged

#### Scenario: Unchecked tasks AND outcome_success called — no scan triggered
- **WHEN** `Executor::run` finishes a run where the agent called `outcome_success` AND `tasks.md` has unchecked items
- **THEN** the classified outcome is `Completed` via the tool-outcome precedence path
- **AND** the acceptance scan does NOT fire (condition 2: tool-recorded outcome was produced)
- **AND** the finalized outcome is `Completed` unchanged (the agent's structured signal wins over the daemon's heuristic disagreement)

#### Scenario: No outcome tool call AND zero unchecked tasks — Completed unchanged
- **WHEN** `Executor::run` finishes a run where no outcome tool was called AND `tasks.md` has zero unchecked items
- **AND** the diff-presence heuristic classifies the outcome as `Completed`
- **THEN** the acceptance scan fires (condition 1 met, condition 2 met — both triggers true)
- **AND** the scan returns zero unchecked items
- **AND** the finalized outcome is `Completed` unchanged

#### Scenario: No outcome tool call AND unchecked tasks present — recovery loop fires
- **WHEN** `Executor::run` finishes a run where no outcome tool was called AND `tasks.md` has unchecked items (e.g. `- [ ] 3.1 thread Arc<DaemonPaths> through polling_loop::run`)
- **AND** the diff-presence heuristic would have classified the outcome as `Completed`
- **THEN** the acceptance scan fires AND returns the non-zero unchecked-item list
- **AND** the recovery loop (per the requirement below) is invoked

#### Scenario: Absent tasks.md does not trigger acceptance failure
- **WHEN** `Executor::run` finishes a run AND `<workspace>/openspec/changes/<change>/tasks.md` does NOT exist
- **THEN** the scan treats the unchecked count as zero
- **AND** no recovery loop fires
- **AND** the classified outcome is returned unchanged

#### Scenario: `run_revision` does NOT trigger acceptance scan
- **WHEN** `Executor::run_revision` finishes a run for an archived change (whose `tasks.md` lives under `archive/<date>-<change>/`, NOT under `openspec/changes/<change>/`)
- **THEN** the acceptance scan does NOT fire regardless of workspace content
- **AND** the classification path proceeds via the existing canonical behavior

#### Scenario: Non-implementer flows do NOT trigger acceptance scan
- **WHEN** `run_triage`, `run_chat_triage`, `run_brownfield_draft`, `run_scout`, OR `run_changelog` finishes a run
- **THEN** the acceptance scan does NOT fire regardless of workspace content
- **AND** the classification path proceeds via the existing canonical behavior

### Requirement: Recovery loop re-prompts the same Claude session on acceptance failure; one retry only

When the acceptance scan returns a non-zero unchecked-item list, `Executor::run` SHALL launch a single recovery turn against the original session via `claude --resume <session_id>` (the same mechanism `Executor::resume` uses for AskUser-flow resumption).

The recovery turn's input SHALL be a structured user-message constructed from the canonical template:

```
Acceptance check failed: your run ended without finishing the change.

tasks.md still has unchecked items:
  - <line_text_1>
  - <line_text_2>
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

The `<line_text_*>` substitutions are the trailing text from each unchecked-item line extracted by the acceptance scan. The list SHALL include every unchecked item the scan returned, in source-order.

The recovery turn SHALL run with the same MCP config (outcome tools available) AND a fresh wall-clock budget equal to the per-run timeout. Within the recovery turn the existing classifier ordering applies: a tool-recorded outcome wins over any heuristic.

After the recovery turn exits, `classify_outcome` SHALL classify its result. If the recovery turn produced a tool-recorded outcome (one of `outcome_success`, `outcome_spec_needs_revision`, `outcome_request_iteration`), that outcome SHALL be returned as `Executor::run`'s final result. The acceptance scan SHALL NOT re-fire on the recovery turn's result.

If the recovery turn did NOT produce a tool-recorded outcome, `Executor::run` SHALL return `ExecutorOutcome::Failed { reason: "acceptance check failed; recovery loop did not produce a structured outcome" }` (exact wording REQUIRED so operators can grep AND scripts can match).

The recovery loop SHALL fire AT MOST ONCE per `Executor::run` invocation. A recovery turn whose own output triggers acceptance failure does NOT fire a second recovery turn.

The recovery turn's stdout/stderr stream SHALL be appended to the per-change run log with a clear divider line. In the summary log: `=== RECOVERY TURN ===` followed by the recovery turn's `final_answer`. In the stream log: `=== RECOVERY TURN ===` followed by the recovery turn's `[tool_use]` / `[tool_result]` / `[assistant]` lines.

#### Scenario: Recovery turn calls outcome_success — final Completed
- **WHEN** the acceptance scan fires AND the recovery turn launches via `claude --resume <session_id>`
- **AND** the agent in the recovery turn marks the unchecked tasks complete in `tasks.md` AND calls `outcome_success({ final_answer: "..." })`
- **THEN** the recovery turn's `consume_outcome` returns a `Success` outcome
- **AND** `Executor::run` returns `Completed { final_answer: <recovery's final_answer> }`
- **AND** the run log contains both the original transcript AND the recovery transcript with the `=== RECOVERY TURN ===` divider

#### Scenario: Recovery turn calls outcome_request_iteration — final IterationRequested
- **WHEN** the acceptance scan fires AND the recovery turn launches
- **AND** the agent calls `outcome_request_iteration({ completed_tasks: [...], remaining_tasks: [...], reason: "..." })`
- **THEN** the recovery turn's `consume_outcome` returns an `IterationRequest` outcome
- **AND** `Executor::run` returns `IterationRequested { ..., iteration_number: <computed per a27a1 rules> }`
- **AND** the run log contains both transcripts

#### Scenario: Recovery turn produces no outcome tool call — final Failed
- **WHEN** the acceptance scan fires AND the recovery turn launches
- **AND** the agent in the recovery turn produces no `outcome_*` tool call AND exits
- **THEN** `Executor::run` returns `Failed { reason: "acceptance check failed; recovery loop did not produce a structured outcome" }`
- **AND** the run log contains both transcripts so the operator can review the agent's reasoning across both phases

#### Scenario: Recovery loop fires at most once per run
- **WHEN** the recovery turn's own result triggers acceptance scan conditions (Completed via diff-presence AND no outcome tool call AND unchecked tasks still present)
- **THEN** a SECOND recovery turn is NOT launched
- **AND** `Executor::run` returns `Failed { reason: "acceptance check failed; recovery loop did not produce a structured outcome" }`

### Requirement: Implementer prompt forbids narrative deferral AND describes the acceptance-scan + recovery-loop enforcement

The bundled `prompts/implementer.md` template SHALL contain an "Anti-narrative-deferral" section near the top of the prompt (above the existing pre-flight outcome-tool section). The section SHALL:

- Direct the agent NOT to narrate "Deferred:" sections in the final-answer text.
- State that the daemon enforces a structured outcome via the outcome tools (`outcome_success`, `outcome_request_iteration`, `outcome_spec_needs_revision`).
- Describe the acceptance scan: at end-of-run, `tasks.md` is scanned for unchecked items; if any are found AND no outcome tool was called, a recovery turn fires.
- Describe the recovery turn: it appends a structured message to the session naming the unchecked items AND requesting an outcome-tool call. The recovery turn has one retry; a recovery turn that ALSO does not call an outcome tool produces a Failed run.

The section's tone is informational, NOT scolding. The text SHALL motivate the structural enforcement (narrative deferral was previously the path of least resistance AND produced corrosive PR shipping) so an agent reading the prompt understands WHY the channel exists AND how to use the right tool the first time.

Canonical text the bundled prompt SHALL produce (section heading + body — the heading SHALL be a top-level prompt section but is rendered here without the `##` markdown prefix to avoid confusing the spec parser):

```
Anti-narrative-deferral discipline

Do NOT narrate "Deferred:" sections in your final-answer text. The
daemon enforces a structured outcome via the outcome tools (see the
"Outcome tools" section below). If you have remaining work, call
`outcome_request_iteration`. If a task is genuinely unimplementable,
call `outcome_spec_needs_revision`. If you finished, call
`outcome_success`. Narrative deferral was previously the path of
least resistance AND produced corrosive PR shipping (unchecked tasks
AND apologetic prose buried in the PR comment); the acceptance scan
now catches this AND triggers a recovery turn that fails the run if
you persist.

At end-of-run, the daemon scans tasks.md for unchecked items. If
unchecked items are present AND you did not call any outcome tool,
the daemon launches a recovery turn that re-prompts you with the
list of unchecked items AND directs you to call exactly one outcome
tool. The recovery turn has one retry; if it ALSO produces no
outcome-tool call, the run is classified as Failed.
```

Operator-customizable override prompts MAY remove OR rewrite this section — the canonical rule binds the bundled default only. Operators who remove this guidance see the structural enforcement (acceptance scan + recovery loop) continue to apply, but their custom implementer agents may not know to expect it.

#### Scenario: Bundled prompt contains the anti-narrative-deferral section
- **WHEN** a maintainer reads `prompts/implementer.md`
- **THEN** the prompt contains an "Anti-narrative-deferral discipline" section near the top (above the pre-flight outcome-tool section)
- **AND** the section names all three outcome tools (`outcome_success`, `outcome_request_iteration`, `outcome_spec_needs_revision`)
- **AND** the section describes both the acceptance scan AND the recovery turn

#### Scenario: Bundled prompt's canonical text matches the requirement
- **WHEN** an automated test extracts the "Anti-narrative-deferral discipline" section from `prompts/implementer.md`
- **THEN** the extracted text matches the canonical text specified above (the structural elements: warning + tool list + acceptance-scan description + recovery-turn description)
