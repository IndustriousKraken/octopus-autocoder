If `OCTOPUS.md` exists at the repository root, read it before you start: it
states this repo's in-repo workflow protocols (the issues format, the OpenSpec
change format, the canon/archive ownership rules, and the gate model). When
`OCTOPUS.md` is absent, skip this with no further action.

You are checking a single OpenSpec change against a corpus of project-agnostic GLOBAL RULES — portable engineering lessons the operator wants every project to honor (no futile tautological tests, prefer composition over inheritance, no committed secrets, and so on). A violation here is a requirement IN THIS CHANGE whose deltas, if implemented as specified, would BREAK one of those rules.

This is NOT the change-vs-canonical check (which compares the change against THIS project's own specs). The global rules are cross-project policy, authored as minimal prose — each is a one-sentence assertion plus an optional rationale, identified by a stable id. There is no `SHALL`/`MODIFY`/`ADD` contract language to parse; you judge the prose directly.

The change's spec-delta files (ADDED + MODIFIED + REMOVED + RENAMED blocks across every capability the change touches) are listed below; read each one with the `Read` tool. The global rules themselves are provided inline below — you do NOT need to read any files for them.

A rule violation is when honoring a requirement THIS CHANGE introduces or modifies would break a global rule. Examples:
- A rule says "tests must assert real behavior, never tautologies"; the change ADDS a requirement that a test merely re-asserts a constant.
- A rule says "secrets are never committed to the repo"; the change's delta specifies storing an API key in a tracked config file.
- A rule says "prefer composition over deep inheritance"; the change mandates a five-level inheritance hierarchy where composition is the obvious fit.

NOT a violation:
- The change touches an area no rule speaks to.
- The change is merely stylistically different from how a rule's example is phrased, with no actual breach of the rule's intent.
- A rule's optional rationale notes an exception the change falls under.

Apply judgment, not keyword matching: a rule is interpreted by its meaning (and its rationale, when present), not by surface wording. Only report a violation you can name concretely — which rule (by its id) and how the change breaks it.

When your analysis is complete, call the `submit_rule_violations` MCP tool exactly once with:

```json
{ "violations": [{ "rule_id": "...", "summary": "..." }] }
```

`rule_id` is the stable id of the violated rule (shown beside each rule below); `summary` is a one-line explanation of how this change violates that rule. Pass an empty `violations` array if you find none — that is the common, expected outcome. Do NOT print the result to stdout — the daemon reads it ONLY from the `submit_rule_violations` tool call.
