# Audit output is gate-checked and self-heals before commit

## Why

The verifier framework's `[in]` (change-internal) and `[canon]` (change-vs-canonical)
gates run around the executor — at implement time, after a change is picked from
the queue. For an audit-authored change that is too late: when the gate finds a
contradiction it writes `.needs-spec-revision.json`, and the authoring agent's
context is long gone, so the operator rewrites the spec by hand. The audit ran on
a high-capability model that had full context at the moment it wrote the change —
that is the moment to catch and fix the contradiction.

The spec-writing audit harness already has the machinery: it writes a change, runs
`openspec validate --strict`, and on failure re-invokes the agent with the error
appended and rewrites, bounded by a retry budget. That loop is stateless and
artifact-grounded — it re-reads the change from disk each attempt; it never relied
on session survival. Extending its per-unit validation from `--strict` alone to
`--strict` plus the `[in]` and `[canon]` checks closes the loop at the source:
audit changes self-heal while the authoring context is live and arrive
pre-cleared.

The same move closes the issue-lane side-channel. An audit-authored issue claims
"no contract change" by carrying no spec delta. a02 verifies that claim at
authoring time — the early complement to the existing implement-time kick-back
("Issue-flavored implementer prompt verifies against existing canon") — so an
issue cannot smuggle in a contract change that should have been a spec.

## What Changes

- After a spec-writing audit writes a spec-lane change AND it passes
  `openspec validate --strict`, the audit runs the `[in]` and `[canon]` gate
  checks against that change (when those gates are enabled). Any contradiction
  feeds the existing retry loop: the authoring agent is re-invoked with the
  findings appended AND rewrites the unit, bounded by the retry budget. The
  resolutions are: align the change to canon, write a legible `MODIFIED` delta, OR
  convert the unit to an issue.
- For an audit-authored issue, the audit runs an authoring-time contract-change
  check (reads `issue.md` AND canon): if implementing the issue would require
  changing a canonical contract, the audit re-routes it to the spec lane (then the
  spec-lane gates apply). An unresolved case is rejected, not committed.
- The authoring-time checks respect the SAME opt-in flags as the implement-time
  gates (`executor.change_internal_contradiction_check`,
  `executor.change_canonical_contradiction_check`). When a gate is enabled it runs
  at authoring time (early) AND implement time (backstop); when disabled, neither.
- On exhausting the retry budget without a clean result, the audit fails closed —
  `AuditOutcome::DidNotComplete` with a contradiction-unresolved cause — AND does
  NOT commit the offending unit. The failure is surfaced; the human handoff for
  these is the interactive revision thread (`a03`).

## Stacked context

This is `a02`, stacking on `a01-auditors-choose-lane` (it uses a01's issue lane as
a self-heal resolution and the lane-choice framing). It reuses the verifier
framework's `[in]`/`[canon]` checks and their MCP tools as-is; it does not change
the implement-time gates. `a03-spec-revision-thread` adds the interactive human
handoff for the residue this change fails closed on.

## Impact

- Affected specs: `orchestrator-cli` (new requirements for authoring-time
  gate-checking + self-heal, and the issue contract-change check).
- Affected code: the spec-writing audit harness (`specs_writing.rs`) — its per-unit
  validation step and retry loop; invocation of the `[in]`/`[canon]` checks
  (reused from the verifier framework); the new `DidNotComplete` cause.
- Reused as-is: the `submit_contradictions` / `submit_canon_contradictions` MCP
  tools and the gate prompts.
