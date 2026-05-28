## Why

The implementer prompt template at `prompts/implementer.md` (lines 28-33 of the current shipped version) specifies the `SpecNeedsRevision` outcome sentinel using angle-bracket placeholders:

```
=== AUTOCODER-OUTCOME ===
{"type":"spec_needs_revision","unimplementable_tasks":[
  {"task_id":"<id-from-tasks-md>","task_text":"<verbatim quote>","reason":"<one-line why>"}
],"revision_suggestion":"<free-form text describing what to change in tasks.md to make the spec verifiable>"}
```

No accompanying instruction tells the agent to substitute concrete values for the angle-bracket placeholders. The agent (Claude CLI in production) treats the template as the literal output format AND emits it verbatim, with `<id-from-tasks-md>`, `<verbatim quote>`, AND the other markers unsubstituted. The daemon's sentinel parser deserializes the payload, finds the placeholder text in fields it expects to be real values, fails to parse cleanly, AND falls back to `Failed { reason: "agent emitted unparseable SpecNeedsRevision sentinel: <excerpt>" }` per the existing canonical "Malformed outcome sentinel falls back to Failed" scenario.

Two consecutive Failed outcomes triggers `perma-stuck`, blocking the change AND the queue. The agent's `spec_needs_revision` intent IS correct (the underlying change `a21-canonical-spec-rag-via-mcp` had a real implementability gap — addressed separately by revising a21's spec deltas), but its sentinel emission is broken AT THE TEMPLATE LEVEL — no future change can correctly trigger spec_needs_revision until the template is fixed.

This affects any future spec_needs_revision path: even when an operator deliberately authors a change with an unimplementable task to test the escape hatch, the sentinel emits placeholders AND becomes a Failed outcome instead of a clean spec-revision marker.

## What Changes

**Revise `prompts/implementer.md`'s sentinel section to anchor the agent's emission with a concrete worked example AND an explicit substitution instruction.** The change is content-only in `prompts/implementer.md`; no API surface or executor logic changes.

The revised sentinel section structure:

1. **Instruction paragraph** explicitly naming substitution: "When you emit the sentinel, REPLACE every value in the example below with concrete data from this change. The example is a pattern; emitting it verbatim is a parse failure."
2. **Worked example** showing what a real sentinel looks like, with realistic task ids AND prose:
   ```
   === AUTOCODER-OUTCOME ===
   {"type":"spec_needs_revision","unimplementable_tasks":[
     {"task_id":"6.4","task_text":"Manual: SSH into the production host and verify systemctl status autocoder","reason":"executor sandbox has no real SSH credentials and no production host access"}
   ],"revision_suggestion":"Replace task 6.4 with a unit test that mocks systemctl-status output, OR move the live-host check to docs/SMOKE.md as an operator step rather than an implementer task."}
   ```
3. **Validation hint** to help the agent self-check: "Before emitting, scan your sentinel for any `<...>` patterns. If you see angle-bracket text inside string values, you have not substituted — the daemon will reject the sentinel as a parse failure."

**Establish a canonical pattern for sentinel templates in implementer prompts** so future sentinel additions (and operator-authored override templates) follow the same shape: instruction + worked example + validation hint. The pattern is documented in a new requirement in the executor capability.

**Parser-side detection of the placeholder failure mode.** When the daemon's `SpecNeedsRevision` parser encounters a payload whose `task_id`, `task_text`, OR `reason` field contains `<...>` patterns that look like un-substituted placeholders (regex: `<[a-z][a-z0-9 _-]*>`), the WARN log SHALL name the specific failure mode (`looks like un-substituted placeholders — see prompts/implementer.md`) instead of just `unparseable sentinel: <excerpt>`. The Failed outcome's reason string gains the same hint. This makes the operator's diagnosis instant when the prompt regresses in the future.

This change does NOT alter the canonical "Malformed outcome sentinel falls back to Failed" scenario — that remains the behavior. The change adds a clearer log AND error message for the placeholder-specific case.

## Impact

- **Affected specs:**
  - `executor` — ADDED requirement: `Sentinel emission instructions in the implementer prompt include a concrete worked example AND a self-check hint`.
  - `orchestrator-cli` — ADDED requirement: `SpecNeedsRevision parser detects un-substituted placeholders AND surfaces a clear failure mode`. This is additive to the canonical "Malformed outcome sentinel falls back to Failed" scenario (which remains the catch-all); the new requirement narrows the WARN log AND Failed-reason for the specific placeholder failure mode.
- **Affected code:**
  - `prompts/implementer.md` — revise the sentinel section per the proposed structure.
  - `autocoder/src/executor/claude_cli.rs` (OR wherever the `SpecNeedsRevision` parse fallback fires) — extend the parse-failure handler to detect un-substituted placeholders AND emit the enhanced WARN + Failed-reason text.
- **Operator-visible behavior:**
  - Future spec-revision-warranted changes produce parseable sentinels (the prompt no longer emits placeholders).
  - If a placeholder regression ever sneaks back (operator authors a custom prompt template AND copies the example without substitution guidance), the WARN log AND alert immediately name the failure mode, cutting diagnosis time.
  - No new config knobs.
- **Breaking:** no. Operator-customized implementer prompt templates remain valid; this change updates only the bundled default AND adds detection that helps operators whose customizations regress.
- **Acceptance:** `cargo test` passes; `openspec validate a28-fix-sentinel-template --strict` passes. Tests:
  - Unit test: a fixture sentinel payload with literal `<id-from-tasks-md>` triggers the placeholder-detection path; the resulting Failed-reason names the failure mode.
  - Unit test: a well-formed sentinel parses cleanly per existing behavior (no regression).
  - Manual: re-trigger `a21-canonical-spec-rag-via-mcp` after the prompt revision lands; verify the agent either implements cleanly (a21 is now revised) OR emits a parseable spec_needs_revision sentinel (if the agent finds any remaining gap).
