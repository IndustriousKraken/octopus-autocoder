You are checking a single OpenSpec change for internal contradictions: requirements within this change that cannot all hold simultaneously. The change's spec-delta files (ADDED + MODIFIED + REMOVED + RENAMED blocks across every capability the change touches) are listed below; read each one with the `Read` tool.

A contradiction is when honoring requirement A would prevent honoring requirement B. Examples:
- A says "all secrets in env vars"; B says "the API key in config.yaml" (same change adds both)
- A caps an operation at N seconds; B describes a workflow that exceeds N seconds
- A enforces a default ("audits opt-in"); B's MODIFIED scenario contradicts the default ("audits always run")

NOT a contradiction:
- A says "feature X exists"; B says "feature Y exists" where X AND Y are different AND compatible
- Wording differences with no semantic conflict
- Different scenarios under the same requirement covering different cases (e.g. "happy path" + "error path") are not contradictory by virtue of being different

Read every delta block. Apply domain knowledge — a "5-minute workflow" IS longer than a "60-second cap" even if the math isn't spelled out; MongoDB IS NoSQL even if neither requirement says "NoSQL."

Be exhaustive, in a SINGLE pass. Evaluate EVERY requirement the change introduces or modifies against EVERY other requirement in the change, and report EVERY distinct contradiction you find — do NOT stop after the first. A single requirement may conflict with more than one other requirement; report each conflict as its own entry. The operator resolves the whole set at once, so reporting only the first forces a fresh revision round for every conflict you held back — a complete sweep now avoids a loop later. Completeness does not license invention: report every REAL conflict, but nothing that is not a genuine conflict.

For each contradiction, produce two distinct things:
- `summary`: ONE line stating WHY the two requirements conflict — what honoring one prevents in the other.
- `suggested_fix`: a concrete edit plan stating WHAT to change and HOW — which requirement(s) to ADD, MODIFY, RENAME, or REMOVE, with a short sketch of the resulting text. This is an actionable instruction the operator could apply, NOT a restatement of the conflict. For example: "MODIFY requirement 'X' so its cap reads 90s instead of 60s, matching requirement 'Y'" or "REMOVE the 'config.yaml' clause from requirement 'B' so it defers to requirement 'A''s env-var rule."

Also read the change's `tasks.md` (its path is listed with the spec-delta files below) and report any task whose EDIT TARGET is the canonical specs (`openspec/specs/`). The implementer implements CODE and TESTS only — a change's spec delta is folded into the canonical specs by `openspec archive` automatically, so a task that directs the implementer to apply the delta to `openspec/specs/` would make the archive abort on a duplicate requirement. Judging whether a task DIRECTS such an edit is a matter of reading it, not keyword-matching:

- REPORT (edit target IS the canonical specs): "Apply the ADDED Requirements block to openspec/specs/<cap>/spec.md", "Copy the MODIFIED requirement into canon", "Write the new requirement directly into the canonical spec".
- Do NOT report (canon is only context, or the target is the change's OWN delta): "Update docs/CHATOPS.md's verb documentation (the project-documentation requirements say the verbs are documented there)" — the edit target is a doc, canon is merely cited; "Add a scenario to openspec/changes/<slug>/specs/<cap>/spec.md" — that is the change's own delta, the legitimate place to author it; "Ensure the code matches the contract in openspec/specs/<cap>/spec.md" — a read-only reference, no edit directed at canon.

Report the exact task text (id + description) of each task you flag in `canon_editing_tasks`. When no task directs a canon edit, leave `canon_editing_tasks` empty (or omit it).

Work through the change OUT LOUD as you go: name each spec file as you read it, say what it requires, and narrate how you compare it against the others. Thinking on the page is encouraged — it does not interfere with the result, and reasoning each comparison aloud helps you catch conflicts you would otherwise miss.

Then — as YOUR FINAL ACTION, which you MUST take — call the `submit_contradictions` MCP tool exactly once, passing EVERY contradiction you found (an empty array if you found none) AND every canon-editing task you found (omit or leave empty if none):

```json
{ "contradictions": [{ "requirement_a": "...", "requirement_b": "...", "summary": "...", "suggested_fix": "..." }], "canon_editing_tasks": ["<offending task text>"] }
```

Your narration is NOT the result but it is used for looging and debugging — the daemon reads the outcome ONLY from the `submit_contradictions` tool call. Do NOT end your turn without making that call, even when you found nothing (call it with an empty `contradictions` array and no `canon_editing_tasks`).
