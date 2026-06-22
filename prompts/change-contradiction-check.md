If `OCTOPUS.md` exists at the repository root, read it before you start: it
states this repo's in-repo workflow protocols (the issues format, the OpenSpec
change format, the canon/archive ownership rules, and the gate model). When
`OCTOPUS.md` is absent, skip this with no further action.

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

Be exhaustive. Evaluate EVERY requirement the change introduces or modifies against EVERY other requirement in the change, and report EVERY distinct contradiction you find — do NOT stop after the first. A single requirement may conflict with more than one other requirement; report each conflict as its own entry. The operator resolves the whole set at once, so a partial list forces another revision round that a complete sweep would have avoided. Completeness does not license invention: report every REAL conflict, but do not pad the list with conflicts that are not genuine.

For each contradiction, produce two distinct things:
- `summary`: ONE line stating WHY the two requirements conflict — what honoring one prevents in the other.
- `suggested_fix`: a concrete edit plan stating WHAT to change and HOW — which requirement(s) to ADD, MODIFY, RENAME, or REMOVE, with a short sketch of the resulting text. This is an actionable instruction the operator could apply, NOT a restatement of the conflict. For example: "MODIFY requirement 'X' so its cap reads 90s instead of 60s, matching requirement 'Y'" or "REMOVE the 'config.yaml' clause from requirement 'B' so it defers to requirement 'A''s env-var rule."

When your analysis is complete, call the `submit_contradictions` MCP tool exactly once. The `contradictions` array carries one entry per distinct conflict — the example below shows a single entry to illustrate the SHAPE, but pass as many entries as you found (or an empty array if none):

```json
{ "contradictions": [{ "requirement_a": "...", "requirement_b": "...", "summary": "...", "suggested_fix": "..." }] }
```

Pass an empty `contradictions` array if you find none. Do NOT print the result to stdout — the daemon reads it ONLY from the `submit_contradictions` tool call.
