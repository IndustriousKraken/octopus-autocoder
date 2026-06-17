# Design

## D1 — The rule protocol: minimal prose, judgment-interpreted

A rule is interpreted by an LLM's judgment, not an algorithm — so the format carries
no machinery that exists to make text machine-parsable. Each rule is: a one-sentence
**`rule`** (the assertion the gate checks), an optional **`intent`** (a short
rationale/exceptions paragraph that informs the judgment and feeds future retrieval),
and a **stable id** (so a violation can name the rule). No `SHALL`, no `MODIFY`/`ADD`
deltas, no scenarios, no task list, no archive/compose step — those serve OpenSpec's
*edit-the-canon* lifecycle, which rules don't have (you edit a rule file directly; git
history is the change record).

The corpus may be flat files or grouped into registers of related rules. The single
growth axis the protocol plans for is **retrieval**: feed all rules to the gate while
the corpus is small; select a relevant subset (coarse by register, then semantic) once
it outgrows the context window. It never grows toward contract language.

## D2 — `[rules]` is `[canon]` with a different corpus

The `[canon]` gate is already "agentic session reads the change deltas + a corpus →
reports conflicts." `[rules]` is the same primitive with the corpus swapped (the global
rule corpus) and the finding re-pointed (it names a violated rule id, not a canonical
requirement). So the implementation should factor the `[canon]` check into a
corpus-parameterized core and instantiate it twice — `[canon]` (project canon) and
`[rules]` (global rules) — rather than fork it. Same read-only `agentic_run` sandbox,
same fail-closed posture, same marker/alert/halt mechanics.

It's a fourth gate in the framework rather than an extension of `[canon]` because the
two are genuinely distinct concerns: `[canon]` is project-internal consistency
(deltas vs this project's specs); `[rules]` is cross-project policy (deltas vs a global
corpus). Distinct corpora, distinct findings, distinct opt-in — so distinct gates, even
though they share machinery.

## D3 — Config and fail-closed

Three config inputs, mirroring `[canon]`: `executor.global_rules_check`
(`disabled` default / `enabled`), `executor.global_rules_check_llm` (the model), and
the corpus location (`executor.global_rules.corpus` — a path or repo the daemon and the
spec-box both have). Enabling the check without BOTH a model AND a resolvable corpus
fails fast at daemon startup (the same fail-fast `[canon]` uses for a missing model).

Per the verifier framework, `[rules]` FAILS CLOSED: a session error, an unregistered
strategy, a schema-rejected submission never corrected, or a session with no submission
holds the change (a structured `gate_error` marker, a `[rules]`-labeled "FAILED TO RUN —
change held" alert) rather than passing as "no violations." An empty submission is a
clean pass.

## D4 — Findings, marker, and the MCP tool

A violation marker is the same `.needs-spec-revision.json` shape `[canon]` writes (a
semantic finding: empty `unimplementable_tasks`, no `gate_error`), with
`revision_suggestion` naming each violated rule by id — so the interactive revision
thread (when it lands) handles a `[rules]` marker exactly like a `[canon]` one. The
session returns findings via a new `submit_rule_violations` MCP tool (role
`global_rules_check`), schema `{ violations: [{ rule_id, summary }] }`, paralleling
`submit_canon_contradictions`. The tool layer consumes a missing submission as empty;
the gate's fail-closed policy (D3) decides the outcome — the tool requirement does not
itself encode open-vs-closed.

## D5 — Runs in both places, one gate

Because `[rules]` is a verifier gate, it runs server-side pre-executor (the enforcement
guarantee: no change in any repo violates the global rules) AND locally via the `verify`
subcommand, which runs "the enabled spec-checking gates" generically and so picks up
`[rules]` with no change. Local = accelerator, server = guarantee — the same
feedback-vs-enforcement split as the other gates.
