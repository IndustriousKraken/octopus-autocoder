If `OCTOPUS.md` exists at the repository root, read it before you start: it
states this repo's in-repo workflow protocols (the issues format, the OpenSpec
change format, the canon/archive ownership rules, and the gate model). When
`OCTOPUS.md` is absent, skip this with no further action.

You are checking a single OpenSpec change for contradictions against the project's EXISTING canonical specs. A contradiction here is a requirement IN THIS CHANGE that conflicts with a requirement that the project has ALREADY locked into canon — not a conflict the change has with itself (a separate check covers that).

A `## MODIFIED Requirements` delta REPLACES the same-titled canonical requirement when the change is archived. It is therefore EXPECTED to differ from the current canonical text — that difference IS the change, and it is the sanctioned mechanism for evolving canon. A MODIFIED delta is NEVER a contradiction with the canonical requirement that shares its title: the new version supersedes the old; the two never hold at once. Only report a conflict when honoring this change would violate a DIFFERENT canonical requirement that the change does NOT modify (or when an ADDED requirement conflicts with existing canon). Comparing a MODIFIED delta against its own same-titled canonical requirement and calling the difference a contradiction is the single most common false positive — do not make it.

The change's spec-delta files (ADDED + MODIFIED + REMOVED + RENAMED blocks across every capability the change touches) are listed below; read each one with the `Read` tool. The project's canonical specs live under `openspec/specs/<capability>/spec.md`; their paths are listed below too. Read the canonical specs of EVERY capability whose invariants the change's behavior bears on — not only the capability that shares the delta's name or is the most obviously related one. A requirement this change introduces can violate an invariant that lives in a SECOND capability (for example, a delta in the `executor` capability that re-specifies a timeout canon has locked in a `sandbox` capability requirement). Missing such a cross-capability conflict forces another revision round, so when in doubt about whether a capability's invariants are touched, read it. If a `query_canonical_specs` MCP tool is available, you MAY use it for focused retrieval of the canonical requirements most relevant to a delta (especially when canon is large); reading `openspec/specs/*/spec.md` directly works just as well when it is not.

A change-vs-canonical contradiction is when honoring a requirement THIS CHANGE introduces or modifies would violate a requirement that is ALREADY canonical. Examples:
- The change ADDS "secrets MAY live in config.yaml"; canon already SHALL "store all secrets in env vars only."
- The change's MODIFIED scenario asserts a default ("audits run on every iteration"); a canonical requirement forbids it ("audits are strictly opt-in").
- The change re-specifies a behavior canon has already locked elsewhere with an incompatible value (a cap of 5 minutes where canon SHALL cap at 60 seconds).

NOT a change-vs-canonical contradiction:
- The change ADDS a brand-new capability that canon says nothing about.
- The change MODIFIES a canonical requirement coherently (the delta IS the intended evolution of canon, and the two do not assert conflicting behavior simultaneously).
- Wording differences with no semantic conflict, or scenarios that cover different cases under the same requirement.

Read the relevant delta blocks AND the canonical requirements they bear on. Apply domain knowledge — a "5-minute workflow" IS longer than a "60-second cap" even if the math isn't spelled out; MongoDB IS NoSQL even if neither requirement says "NoSQL."

Be exhaustive. Evaluate EVERY requirement the change introduces or modifies against EVERY applicable canonical requirement across the capabilities you read, and report EVERY distinct conflict — do NOT stop after the first. A single change requirement may conflict with more than one canonical requirement, possibly across more than one capability; report each conflict as its own entry, naming the specific canonical requirement it conflicts with. The operator resolves the whole set at once, so a partial list forces another revision round that a complete sweep would have avoided. Completeness does not license invention: report every REAL conflict, but do not pad the list — and never count a MODIFIED delta against its own same-titled canonical requirement (see above).

For each contradiction, produce two distinct things:
- `summary`: ONE line stating WHY the change's requirement and the canonical requirement cannot both hold.
- `suggested_fix`: a concrete edit plan stating WHAT to change and HOW — which requirement(s) to ADD, MODIFY, RENAME, or REMOVE, with a short sketch of the resulting text. This is an actionable instruction, NOT a restatement of the conflict. For the common case the fix is usually one of two shapes, so say which and sketch the text: (a) "turn the contradicted canonical requirement into a coherent `## MODIFIED Requirements` delta of this change" — the change INTENDS to evolve canon, so re-express it as the sanctioned MODIFIED mechanism, sketching the new requirement text; OR (b) "align this change's requirement to canon's existing term/value" — the change unintentionally diverged, so amend the delta to match canon, sketching the corrected text.

When your analysis is complete, call the `submit_canon_contradictions` MCP tool exactly once. The `contradictions` array carries one entry per distinct conflict — the example below shows a single entry to illustrate the SHAPE, but pass as many entries as you found (or an empty array if none):

```json
{ "contradictions": [{ "change_requirement": "...", "canonical_capability": "...", "canonical_requirement": "...", "summary": "...", "suggested_fix": "..." }] }
```

`change_requirement` names the requirement in THIS CHANGE; `canonical_capability` is the capability slug of the conflicting canonical spec (the `<capability>` in `openspec/specs/<capability>/spec.md`); `canonical_requirement` is the title of the conflicting canonical requirement; `summary` is a one-line explanation of why the two cannot both hold; `suggested_fix` is the concrete edit plan described above. Pass an empty `contradictions` array if you find none. Do NOT print the result to stdout — the daemon reads it ONLY from the `submit_canon_contradictions` tool call.
