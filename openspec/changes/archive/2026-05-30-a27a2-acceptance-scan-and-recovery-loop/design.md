# Design

## Decisions to lock in

### D1. Acceptance scan is implementer-flow-only. Other `classify_outcome` callers are unchanged.

Audit/triage/scout/brownfield-draft/changelog-stylist flows go through `classify_outcome` but do NOT operate against a per-change `tasks.md`. They produce other artifacts (audit findings posted to PR comments, triage decisions written to channel, scout reports written to a brownfield-discovery file, etc.). Applying a tasks.md-based acceptance check would mis-fire on every one of them — every triage run would "fail acceptance" because there's no tasks.md to scan.

`run_revision` is borderline. It operates against a change that has been archived; `tasks.md` no longer exists at `openspec/changes/<change>/tasks.md` (it lives in `archive/<date>-<change>/tasks.md`). The revision's job is targeted edits to the PR diff, not implementing a fresh task list. Applying the acceptance scan would either fail to find tasks.md (skip) OR scan the wrong file (the archived version, which IS the canonical task list but is also already-checked from the original run). Either way the signal is wrong.

The clean rule: acceptance scan fires only in `Executor::run` (the implementer-first-pass path). The other entry points retain today's classification verbatim.

### D2. Acceptance scan fires AFTER `classify_outcome` returns `Completed`, NOT before.

The scan is the gatekeeper between `Completed` (which opens the PR) AND finalization. Other outcomes (`AskUser`, `Failed`, `SpecNeedsRevision`, `IterationRequested`) bypass the scan — those are already structured outcomes; second-guessing them serves no purpose.

This ordering means: an agent that completes some tasks AND emits `outcome_request_iteration` is correctly classified as `IterationRequested` by a27a1's path, AND the acceptance scan never runs. The scan only fires when the agent's signal would otherwise be "Completed via diff-presence heuristic" (the same path that today produces narrative deferrals).

### D3. Outcome-tool call AT ALL trumps acceptance scan. Even `outcome_success` with unchecked tasks.

An agent that calls `outcome_success` while `tasks.md` has unchecked items is making an explicit assertion ("I'm done") AND the daemon respects it. Two reasons:

- The agent's structured signal is more authoritative than the daemon's heuristic. If the agent says "done" AND the daemon disagrees, the right resolution is to surface the disagreement to the operator (via the PR diff inspection AND the eventual code reviewer) rather than override the agent.
- The acceptance scan would otherwise punish agents that mark `outcome_success` while leaving a low-priority cosmetic task unchecked — which is a perfectly reasonable judgment call AND the operator can fix in revision.

The scan ONLY fires when the agent exited without calling ANY outcome tool — the narrative-deferral path. This is the case the scan exists to catch.

### D4. Recovery loop uses `claude --resume <session_id>`. One retry only.

The session-resume mechanism is already in use by the AskUser flow (`Executor::resume` calls `claude --resume <session_id>` with the operator's answer appended). The recovery loop uses the same primitive with a different appended message.

Why one retry only:
- A single explicit recovery prompt covers the "I forgot" failure mode (the most common). Beyond one, we're not getting structured outcomes — we're failing to communicate.
- A multi-turn recovery loop on a confused agent burns wall-clock budget AND token budget that the operator would rather spend on a fresh run.
- Operator visibility: one recovery turn means the operator's log shows two transcripts (original + recovery). Five recovery turns would be five transcripts, which buries the diagnostic signal in noise.

The cap is fixed at 1. A future change MAY make it configurable; this requirement does NOT bind on configurability.

### D5. Recovery prompt names the unchecked items explicitly AND names all three outcome-tool options.

The prompt is the agent's last chance to converge. Two design choices for its content:

- **Named unchecked items**, not just "tasks.md has unchecked items." Names AND verbatim task text give the agent the specific signal it needs. The agent might say "oh, I forgot 3.2 — it's actually done in code, I just didn't update tasks.md" AND mark it. Without the explicit naming, the agent has to re-read tasks.md, which is extra work AND may not converge.
- **All three outcome-tool options listed.** The agent might not realize which tool applies. Listing all three with their criteria ("forgot to mark / honest scope-overflow / unimplementable") covers the decision tree explicitly.

The prompt's tone is informational, NOT scolding. "The daemon enforces a structured outcome" is a constraint statement, not a moral judgment. Agents on a recovery turn are already in a low-throughput state; piling on emotional weight slows convergence.

### D6. Acceptance scan parses `^- \[ \]` lines. Ignores fenced code blocks.

The scanner reads `tasks.md` line by line, tracking whether the cursor is inside a `\`\`\`` fenced block. Lines matching the regex `^[ \t]*- \[ \] ` outside fenced blocks count as unchecked. Lines matching `^[ \t]*- \[x\] ` are checked AND ignored. The scanner does NOT attempt to parse task IDs OR task text — it just counts AND extracts the trailing text for the recovery prompt.

The scanner's output for the recovery prompt is a list of `(line_number, trailing_text)` pairs. The recovery prompt formats them as bullets.

Edge cases handled:
- Nested checkboxes (`  - [ ] subtask`) count as separate unchecked items.
- Empty fenced blocks (no closing ` ``` `) are treated as "infinite fence" — content after an unclosed fence is ignored. tasks.md with an unclosed fence is malformed AND the scan's "skip on absent/unparseable" rule kicks in.
- Task lines with weird whitespace (tabs, trailing spaces) are normalized for the trailing-text extraction.

### D7. Legacy stdout sentinel removal is hard. No further fallback.

By a27a2:
- Two release cycles have elapsed since outcome tools became canonical (a27a0 + a27a1).
- The acceptance scan + recovery loop cover the "agent forgot to call a tool" case structurally.
- Any operator running a stale implementer prompt that emits stdout sentinels would see the recovery turn fire on every run AND the agent would converge on outcome-tool calls.

The removal is mechanical:
- Delete `OUTCOME_SENTINEL_TAG`, `extract_outcome_sentinel`, `try_parse_spec_needs_revision`, `excerpt_for_reason`.
- Delete the classifier's stdout-sentinel branch (lines ~1115–1138 in claude_cli.rs as of this writing).
- Delete the `legacy stdout sentinel matched` log line.
- Delete the test fixtures asserting stdout-sentinel parsing (kept their replacement test fixtures from a27a0 alive; the legacy fixtures are dead code).
- Delete the DEPRECATED section from `prompts/implementer.md`.

The classifier's new shape (post-deletion):

```
consume_outcome → AskUser marker → timeout → exit status → diff-presence heuristic → Completed
```

The timeout precedence preserved from a27a0's "Tool-recorded outcomes take precedence" requirement (step 3 of its ordering) continues to apply; the modified version of that requirement drops the no-longer-relevant step 4.

## Open questions for the implementer

- **Recovery turn's log capture.** The recovery turn produces its own stdout/stderr stream AND its own `result` event. The implementer SHOULD append this to the existing per-change run log (per a20a2's split-log layout — stream content goes in the `.stream.log`, summary content in the `.log`) with a clear divider line (`=== RECOVERY TURN ===`) so the operator can navigate the two phases.
- **Session-resume timeout.** The recovery turn's wall-clock budget is bounded by what? Options: the same per-run timeout (90 minutes by default) starts fresh OR shares budget with the original run. The implementer SHOULD pick the simpler shape (fresh budget for the recovery turn, capped at 1 retry) AND document it.
- **tasks.md parser corner cases.** The implementer SHOULD test against tasks.md fixtures from the a27 stack itself (this change's tasks.md, a27a0's, a27a1's) to ensure the parser handles real-world content. The implementer SHOULD also test against a tasks.md with no checkboxes at all (some schemas might not use checkboxes); the right behavior is "zero unchecked" → no acceptance failure.
- **PR-comment composition with recovery.** Today's polling-loop posts an `## Agent implementation notes` comment with the original run's `final_answer`. When recovery fires, the operator presumably wants BOTH the original AND recovery `final_answer` content visible. The implementer SHOULD concatenate them in the PR comment with a `### Recovery turn` heading separating them. This is a polishing detail; the implementer MAY ship a single-phase comment in v1 AND open a follow-on for the dual-phase comment.

## Stack completion

This change closes the three-change stack. After archive:

- Outcome signaling is via MCP tools (no more stdout sentinels).
- Iteration sequences are structurally supported (no more narrative deferrals).
- Recovery on acceptance failure is automatic (one-shot, with operator-visible diagnostics).
- The implementer prompt enforces "no narrative deferral" both prescriptively (text) AND structurally (acceptance scan).

Future improvements (not in scope here):
- Configurable iteration cap (today: fixed at 5).
- Configurable recovery cap (today: fixed at 1).
- Migration of non-implementer flows to `outcome_success` as the explicit completion signal (today: they continue to rely on exit-status + clean-diff heuristics, which is fine but less self-documenting).
- Dual-phase PR comment composition (the open question above).
- Iteration-sequence summary in the FINAL `Completed` PR body (combining all prior iterations' `final_answer` content with the recovery turn's content if applicable).
