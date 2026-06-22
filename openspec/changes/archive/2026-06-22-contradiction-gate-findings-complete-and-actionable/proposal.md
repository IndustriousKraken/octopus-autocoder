# Contradiction gate findings are complete and actionable

## Why

The `[in]` (change-internal) and `[canon]` (change-vs-canonical) contradiction
gates have two output problems that together make a flagged change painful to
resolve:

1. **They report one contradiction at a time.** The submission plumbing already
   accepts an unbounded array of findings, but the prompts do not direct an
   exhaustive sweep — each shows a single-element JSON example, neither says
   "report every contradiction," and the `[canon]` prompt scopes its canon
   reading to "the same — or related — capabilities." So a change requirement
   that violates more than one invariant — often against a DIFFERENT canonical
   requirement in a DIFFERENT capability the first pass never read — surfaces its
   conflicts one per round, forcing a sequence of revisions that could have been
   one.

2. **The suggested revision is inscrutable.** The marker's `revision_suggestion`
   and the chatops alert re-list each finding's `summary` (which explains WHY two
   requirements conflict) plus a generic boilerplate footer. There is no field
   that says WHAT to change. An operator reading the first output cannot tell what
   the revision would actually do — it reads as a restatement of the problem.

Both are output-quality problems in the same two prompts, the same submission
schemas, and the same finding-rendering path. Having to revise more than once
should be the exception; and when a change IS flagged, the operator should be able
to see, from the first output, exactly what edit resolves it.

## What Changes

- **Exhaustive sweep.** Both gate prompts direct the agent to evaluate every
  requirement the change introduces or modifies against every applicable
  requirement and report EVERY distinct conflict — never stop at the first. The
  `[canon]` prompt's canon-reading scope widens from "the same or related
  capabilities" to every capability whose invariants the change's behavior bears
  on, so a cross-capability conflict is not missed. A single requirement that
  conflicts with multiple others yields one finding per conflict. The
  single-element JSON examples become illustrative of a set.

- **Concrete, actionable suggested fix.** Each finding carries a new
  `suggested_fix` field — a specific proposed edit (which requirement(s) to ADD /
  MODIFY / RENAME / REMOVE, with a sketch of the resulting text), distinct from
  the one-line `summary` of why the two conflict. The prompts direct the agent to
  produce the concrete edit plan, not a restatement. The marker and alert render
  the suggested fix prominently per finding, so the operator can tell what the
  revision would do from the first output.

- The existing precision guardrails are unchanged — most importantly that a
  MODIFIED delta is never a contradiction with its own same-titled canonical
  requirement.

## Impact

- Affected capability: `orchestrator-cli` — adds two requirements governing both
  the `[in]` and `[canon]` gates (completeness; actionable suggested fix). The
  gates' existing requirements are unchanged; these layer on top.
- Affected code: `prompts/change-contradiction-check.md` and
  `prompts/change-vs-canonical-check.md` (the embedded prompts); the
  `submit_contradictions` / `submit_canon_contradictions` MCP tool schemas and
  finding structs (add `suggested_fix`); the marker `revision_suggestion` builder
  and the chatops alert rendering. No change to the gate disposition (when a
  change is held) or to the fail-closed posture.
- Output quality is a prompt/model property (how complete the sweep is, how
  concrete the fix is). The testable surface is the plumbing: every submitted
  finding — and its `suggested_fix` — reaches the marker and alert without
  truncation. Recall and fix-concreteness are validated in operation, not by a
  unit test asserting prompt wording.
- Complementary to `revision-grounded-in-current-contradiction`, which makes the
  marker the durable source of truth and the executor converge; that change reuses
  these finding structs, so a `suggested_fix` recorded here flows into the marker
  it grounds the executor on.
