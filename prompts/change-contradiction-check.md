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

Work through the change OUT LOUD as you go: name each spec file as you read it, say what it requires, and narrate how you compare it against the others. Thinking on the page is encouraged — it does not interfere with the result, and reasoning each comparison aloud helps you catch conflicts you would otherwise miss.

Then — as YOUR FINAL ACTION, which you MUST take — call the `submit_contradictions` MCP tool exactly once, passing EVERY contradiction you found in a single array (an empty array if you found none):

```json
{ "contradictions": [{ "requirement_a": "...", "requirement_b": "...", "summary": "...", "suggested_fix": "..." }] }
```

Your narration is NOT the result but it is used for looging and debugging — the daemon reads the outcome ONLY from the `submit_contradictions` tool call. Do NOT end your turn without making that call, even when you found nothing (call it with an empty `contradictions` array).
