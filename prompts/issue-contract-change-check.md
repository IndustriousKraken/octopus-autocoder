If `OCTOPUS.md` exists at the repository root, read it before you start: it
states this repo's in-repo workflow protocols (the issues format, the OpenSpec
change format, the canon/archive ownership rules, and the gate model). When
`OCTOPUS.md` is absent, skip this with no further action.

You are checking a single OpenSpec ISSUE for a hidden contract change against the project's EXISTING canonical specs. An issue is a correction to code that is ALREADY correctly specified (a bug fix or a behavior-preserving refactor). It carries NO spec delta — the absence of a `specs/` directory is its contract that implementing it changes no canonical contract.

Your job is to judge ONE thing: would implementing this issue REQUIRE changing a canonical contract? This is the same judgment the implement-time issue kick-back applies ("if the fix actually requires NEW or CHANGED behavior, it belongs in the changes lane, not the issues lane"), pulled forward to authoring time so the unit is routed correctly BEFORE it is committed.

An issue is HONEST (no contract change) when its fix brings drifted code back into conformance with what the canonical specs in `openspec/specs/` ALREADY say the code should do. The spec is already correct; only the code is wrong. Fixing the code to match the existing spec is a behavior-preserving correction and requires NO contract change.

An issue REQUIRES a contract change when satisfying its report would force the code to behave differently than a canonical requirement says it should — i.e. you could not implement the fix without ALSO changing what the spec mandates (a new behavior, a changed default, a removed guarantee, a different value than canon locks). Such an item belongs in the spec lane (`openspec/changes/<slug>/`) as a legible `MODIFIED`/`ADDED` delta, NOT in the issues lane.

Examples that REQUIRE a contract change (report them):
- The issue asks to change a default the canonical spec fixes ("retry 5 times" where canon SHALL "retry 3 times").
- The issue asks to add a new behavior canon does not describe and that changes an observable contract.
- The issue asks to relax or remove a guarantee canon mandates.

NOT a contract change (an honest issue — report nothing):
- The code returns the wrong value and the fix makes it return what canon already specifies.
- A refactor that preserves all observed behavior the specs describe.
- A missing error-path the spec already requires but the code omits.

Read the issue's `issue.md` (its report, diagnosis, AND acceptance criteria) AND the canonical specs under `openspec/specs/<capability>/spec.md` that cover the behavior the issue touches (via `Read`, or via `query_canonical_specs` when that tool is available). Apply domain knowledge.

When your analysis is complete, call the `submit_canon_contradictions` MCP tool exactly once with:

```json
{ "contradictions": [{ "change_requirement": "...", "canonical_capability": "...", "canonical_requirement": "...", "summary": "..." }] }
```

Each entry names ONE contract change the issue would require: `change_requirement` is the behavior the issue's fix would introduce or alter; `canonical_capability` is the capability slug of the affected canonical spec (the `<capability>` in `openspec/specs/<capability>/spec.md`); `canonical_requirement` is the title of the canonical requirement that would have to change; `summary` is a one-line explanation of why implementing the issue cannot be done without changing that contract. Pass an EMPTY `contradictions` array when the issue is honest (no contract change required) — that is the common case. Do NOT print the result to stdout — the daemon reads it ONLY from the `submit_canon_contradictions` tool call.
