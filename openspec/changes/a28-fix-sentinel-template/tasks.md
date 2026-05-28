## 1. Revise `prompts/implementer.md`

- [ ] 1.1 Replace the existing sentinel section (currently around lines 25-39 of the shipped file) with the new structure:
  - **Substitution instruction (before the example)**: a single paragraph naming the rule. Required text: "When you emit the sentinel below, REPLACE every value in the example with concrete data from THIS change. The angle-bracket-free example shows the shape; emitting it verbatim is a parse failure that triggers Failed-outcome handling AND eventually perma-stuck."
  - **Worked example (no placeholders)**: a complete, parseable JSON sentinel with realistic task ids AND prose. Use the example from the proposal (task `6.4`, "Manual: SSH into the production host...", with a concrete revision_suggestion). The example SHALL NOT contain any `<...>` markers.
  - **Field-by-field instruction**: a short list describing what to put in each field — `task_id` is the exact id from tasks.md; `task_text` is the verbatim text of the unimplementable task; `reason` is one line naming why it can't run in your sandbox; `revision_suggestion` is a concrete edit the operator can make to tasks.md.
  - **Self-check hint**: a final paragraph: "Before emitting, scan your sentinel for `<...>` patterns inside string values. If you see any, you have not substituted — re-read this section AND fix before emitting. The daemon detects this specific failure mode AND will surface it in the WARN log."
- [ ] 1.2 The sentinel format itself does NOT change (the JSON shape is the same `{"type":"spec_needs_revision","unimplementable_tasks":[...],"revision_suggestion":"..."}`); only the surrounding instructions + the example change.
- [ ] 1.3 Tests (manual + the per-`a24` PromptLoader unit tests): the embedded `prompts/implementer.md` parses cleanly AND a regression test confirms the worked-example JSON deserializes via `serde_json` to `SpecNeedsRevisionDetail` cleanly.

## 2. Parser-side placeholder detection

- [ ] 2.1 In `autocoder/src/executor/claude_cli.rs` (OR wherever the `SpecNeedsRevision` sentinel parse + fallback fires), extend the fallback path:
  - When `serde_json::from_str::<SpecNeedsRevisionDetail>(payload)` SUCCEEDS, scan each `task_id`, `task_text`, AND `reason` field for the regex `<[a-z][a-z0-9 _-]*>`. If any field matches, treat the sentinel as malformed (placeholder failure mode) AND fall through to the Failed-outcome path described below.
  - When `serde_json::from_str` FAILS outright (existing behavior), continue with the existing Failed-outcome path.
  - In both cases, emit the WARN log AND Failed-reason. For the placeholder failure mode specifically, the WARN log line AND Failed-reason include the diagnostic: `looks like un-substituted placeholders — the agent emitted the prompt's example verbatim instead of substituting concrete values; see prompts/implementer.md sentinel section`.
- [ ] 2.2 The regex is intentionally narrow (lowercase letters / digits / spaces / underscores / hyphens) to avoid matching legitimate `<...>` text in task descriptions (e.g., `<repo>` in a task verb syntax). Treat false positives as acceptable: a real task whose text happens to match the pattern triggers the WARN but the operator's diagnosis is unchanged (the message names the failure mode).
- [ ] 2.3 Tests:
  - Unit: a `SpecNeedsRevisionDetail` payload with literal `<id-from-tasks-md>` triggers placeholder detection; resulting Failed-reason contains the documented diagnostic text.
  - Unit: a well-formed sentinel (the proposal's worked example) parses cleanly AND does NOT trigger placeholder detection.
  - Unit: a sentinel with `<my-tool>` inside `task_text` (a legitimate-looking false positive) DOES trigger placeholder detection; the test asserts this is intentional behavior, not a defect.
  - Unit: a sentinel that fails `serde_json::from_str` outright (e.g., malformed JSON, missing `type` field) follows the existing fallback path with the original WARN text (no regression).

## 3. Spec deltas

- [ ] 3.1 `openspec/changes/a28-fix-sentinel-template/specs/executor/spec.md` ADDs the worked-example-mandate requirement.
- [ ] 3.2 `openspec/changes/a28-fix-sentinel-template/specs/orchestrator-cli/spec.md` ADDs the placeholder-detection requirement.

## 4. Verification

- [ ] 4.1 `cargo test` passes.
- [ ] 4.2 `openspec validate a28-fix-sentinel-template --strict` passes.
- [ ] 4.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
- [ ] 4.4 Manual verification:
  - Apply the prompt change locally.
  - Author a tasks.md with one obviously-unimplementable task (e.g., "Run `sudo systemctl restart nginx` on the production host").
  - Invoke the implementer in dry-run/test mode.
  - Confirm the emitted sentinel contains substituted values, NOT `<id-from-tasks-md>` or similar.
  - Authoring a deliberately-broken sentinel (e.g., via a hand-crafted test fixture) confirms the placeholder-detection WARN fires with the documented diagnostic.
