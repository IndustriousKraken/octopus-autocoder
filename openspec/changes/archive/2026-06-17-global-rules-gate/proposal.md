# Global-rules gate: enforce cross-project rules at the change envelope

## Why

The operator accumulates portable, project-agnostic engineering rules — hard-won
lessons that should hold across every project autocoder works (no futile tautological
tests, prefer composition, no committed secrets, and so on). Today there is nowhere
to declare them and nothing to enforce them. The `[canon]` gate already does the
exact shape of work needed: an agentic session compares a change against a corpus and
reports where it conflicts. A global-rules check is that same machinery pointed at a
different corpus — the global rule corpus instead of the project's canonical specs.

Making it a gate (not a separate project) means it runs in both places that matter:
locally via `verify` (fast feedback before push) and server-side as the enforcement
guarantee that no change in any repo violates the global rules. The rule corpus is a
durable, shared asset — the same corpus a future vigilance layer would consume — so
nothing here is throwaway.

## What Changes

- **A minimal prose rule protocol** (`project-documentation`): the global rule corpus
  is a collection of project-agnostic rules, each a one-sentence assertion + an
  optional rationale + a stable id. Deliberately NOT OpenSpec contract language — no
  `SHALL`/`MODIFY`/`ADD` deltas, no scenarios, no task lists, no archive step. A rule
  is interpreted by judgment; contract keywords add authoring friction without adding
  checkability. Rules are edited directly (git history is the change record). The only
  structural pressure the protocol anticipates is **retrieval at scale**, not
  formalization.
- **The `[rules]` gate** (`orchestrator-cli`): a corpus-parameterized sibling of
  `[canon]` — same `agentic_run` read-only machinery and fail-closed posture, but the
  comparison corpus is the global rule corpus and each finding names the violated rule
  (by its id) rather than a canonical requirement. The verifier-gate framework grows
  from three named gates to four (`[rules]` is pre-executor, alongside `[in]`/`[canon]`).
- **The `submit_rule_violations` MCP tool** (`executor`): the per-role submission tool
  the `[rules]` session uses, paralleling `submit_canon_contradictions`.
- It runs server-side (the enforcement guarantee) AND locally via `verify` (which
  already runs "the enabled spec-checking gates" generically, so it picks `[rules]` up
  with no change there).

## Impact

- Affected specs: `project-documentation` (the rule protocol), `orchestrator-cli` (the
  verifier-framework grows to four gates; the new `[rules]` gate requirement),
  `executor` (the `submit_rule_violations` tool).
- Affected code: a new opt-in flag `executor.global_rules_check` + model block
  `executor.global_rules_check_llm` + corpus location config; the `[rules]` gate
  realized as the corpus-parameterized form of the `[canon]` check; the
  `submit_rule_violations` MCP role.
- Independent of the audit stack and the other in-flight changes; pairs with
  `verify-subcommand` (the local runner) but does not depend on it — the gate runs
  server-side regardless.
