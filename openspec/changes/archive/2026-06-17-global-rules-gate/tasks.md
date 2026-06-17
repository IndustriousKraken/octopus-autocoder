# Tasks

## 1. Rule corpus + protocol

- [x] 1.1 Define the global rule corpus layout: a directory of rule files (flat, or grouped into register subdirectories), each rule = a one-sentence `rule` + optional `intent` + a stable id. No deltas, no scenarios, no tasks, no archive. Document the protocol in `docs/` per the `project-documentation` requirement.
- [x] 1.2 Add config for the corpus location: `executor.global_rules.corpus` (a local path OR a git repo the daemon resolves/clones). Resolve + validate it at startup.

## 2. The `[rules]` gate (corpus-parameterized sibling of `[canon]`)

- [x] 2.1 Factor the `[canon]` check into a corpus-parameterized core (reads change deltas + a corpus → findings), then instantiate it twice: `[canon]` (project canon) and `[rules]` (the global rule corpus). Do NOT fork the `[canon]` logic.
- [x] 2.2 Register `[rules]` in the verifier-gate framework as a pre-executor gate alongside `[in]`/`[canon]`. It runs read-only via `agentic_run` with `ORCH_MCP_ROLE = global_rules_check`.
- [x] 2.3 Feed the rule corpus to the session (all rules at small scale); leave a retrieval seam for relevant-subset selection when the corpus outgrows the context window. Support flat and grouped corpora.
- [x] 2.4 On non-empty findings: write `.needs-spec-revision.json` (semantic-finding shape — empty `unimplementable_tasks`, no `gate_error`) with `revision_suggestion` naming each violated rule by id; post the `SpecNeedsRevision` alert; halt the queue walk; do not invoke the executor.
- [x] 2.5 Fail-closed: a session/strategy/submission failure holds the change with a `gate_error` marker + the "gate FAILED TO RUN — change held" alert, `[rules]`-labeled. An empty submission is a clean pass.
- [x] 2.6 Default-deny verdict ledger: add a `[rules]` slot (initialized `PENDING`; a stub writes `DISABLED` when the gate is off; `PASS`/`FAIL`/`FAILED_TO_RUN` on run) AND add `[rules]` to the BLOCKING set so the executor is invoked only when `[in]`/`[canon]`/`[rules]` are `PASS` or `DISABLED`. Render the `[rules]` verdict in the PR ledger section like the other gates.

## 3. Config + fail-fast

- [x] 3.1 Add `executor.global_rules_check` (`ContradictionCheckMode`, default `disabled`) and `executor.global_rules_check_llm` (model block, parallel to `change_canonical_contradiction_check_llm`).
- [x] 3.2 Startup validation: enabling `global_rules_check` without BOTH `global_rules_check_llm` AND a resolvable `global_rules.corpus` fails fast with a named error (mirroring the `[canon]` fail-fast).

## 4. `submit_rule_violations` MCP tool

- [x] 4.1 Advertise `submit_rule_violations` only for `ORCH_MCP_ROLE = global_rules_check`; schema `{ violations: [{ rule_id, summary }] }`; relay via a56 `record_submission`; schema-invalid is a correctable tool error. Register the role's schema with the submission store.
- [x] 4.2 A no-submission session consumes as empty at the tool layer; the `[rules]` gate caller applies the fail-closed policy.
- [x] 4.3 Spec/doc consistency (no behavior change — the gate callers already fail closed): reconcile the `[in]` and `[canon]` MCP tool requirements (`submit_contradictions`, `submit_canon_contradictions`) so their tool-layer wording reads "missing submission consumed as empty; the gate's fail-closed policy decides," removing the stale "fail-open / WARN-and-proceed" descriptions that contradict the pre-executor gates' fail-closed posture.

## 5. Local + server

- [x] 5.1 Confirm the `verify` subcommand (which runs "the enabled spec-checking gates" generically) picks up `[rules]` with no change there; the spec-box config carries the model block + corpus location.
- [x] 5.2 Server-side: `[rules]` runs pre-executor in the polling loop as the enforcement guarantee, same as `[in]`/`[canon]`.

## 6. Tests

- [x] 6.1 Default-disabled → no `[rules]` session.
- [x] 6.2 Enabled + a change violating a seeded rule → `submit_rule_violations` with the rule id; marker written naming the rule; executor not invoked; halt. (Assert behavior/state.)
- [x] 6.3 Enabled + a clean change → empty submission → proceeds to executor, no marker.
- [x] 6.4 Session/submission failure → fail-closed hold (`gate_error`), `[rules]`-labeled; never "no violations".
- [x] 6.5 Enabled without model OR corpus → startup fails fast.
- [x] 6.6 `submit_rule_violations` advertised only for `global_rules_check`; schema-invalid is correctable; missing consumed as empty.

## 7. Docs

- [x] 7.1 Document the rule protocol and authoring (one-sentence rule + optional intent + id; no contract language) and how to point `executor.global_rules.corpus` at the corpus; note the gate runs locally via `verify` and server-side as the guarantee.
