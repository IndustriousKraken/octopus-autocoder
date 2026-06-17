# Design

## D1 — Invocation site: the existing retry loop

`specs_writing.rs` already loops per attempt: clean prior dirs → run the authoring
agent → snapshot new unit dirs → validate each → on failure, append the error to
the prompt addendum and retry, up to the retry budget; commit the validated set.
a02 extends the per-unit validation from one check (`openspec validate --strict`)
to three for a spec-lane change: `--strict`, then the `[in]` gate check, then the
`[canon]` gate check. A finding from any of them is appended to the same addendum
and drives the same retry. No new loop, no new state machine — one more kind of
validation failure feeding the mechanism that already exists.

## D2 — Self-heal is stateless and artifact-grounded

The retry re-invokes the authoring agent on a fresh prompt (the original prompt
plus the findings addendum); it does not resume a session. The unit on disk is the
context — the agent re-reads it, re-reads canon, and rewrites. This is the
existing `--strict` retry behavior; a02 only widens what counts as a finding. No
session id is stored and nothing is persisted between attempts beyond the addendum
and the on-disk unit.

## D3 — Resolutions per lane; re-routing is allowed

- **Spec lane** (`[in]`/`[canon]` finding): align the change to canon (reuse
  canonical vocabulary), write a legible `MODIFIED` delta of the contradicted
  requirement, OR convert the unit to an issue (a01's lane).
- **Issue lane** (contract-change finding): convert the unit to a spec-lane change.

A re-routed unit is re-checked under its new lane's checks in a subsequent attempt
(an issue re-routed to a spec runs `--strict` + `[in]` + `[canon]`; a change
converted to an issue runs the contract-change check). Re-routing consumes a retry
attempt like any other rewrite.

## D4 — One config knob per gate, two invocation points

The authoring-time checks are governed by the SAME flags as the implement-time
gates: `executor.change_internal_contradiction_check` and
`executor.change_canonical_contradiction_check` (and their `_llm` model blocks).
When a gate is enabled it runs at authoring time (early, self-healing) AND at
implement time (the unchanged backstop). When disabled, it runs at neither point.
This keeps a single operator-facing knob per check and avoids an audit-time check
the operator did not opt into. The startup fail-fast for "enabled without model
config" is unchanged and now also covers the authoring-time use.

## D5 — Bounded, fail-closed

The retry budget is the existing `max_validation_retries`; a contradiction finding
consumes attempts the same way a `--strict` failure does. On exhaustion the audit
does NOT commit the offending unit and resolves to
`AuditOutcome::DidNotComplete` with a contradiction-unresolved cause, conforming to
the fail-closed audit framework (which enumerates its causes as "at least" a set,
so a new cause extends rather than contradicts it). The failure is surfaced via the
existing audit-failure chatops path; the human handoff is `a03`.

## D6 — The issue-lane contract-change check

An issue has no spec delta, so the delta-reading `[in]`/`[canon]` checks do not
apply to it directly. Instead the audit runs an authoring-time contract-change
check: an `agentic_run` session reads `issue.md` AND the relevant canon and judges
whether implementing the issue would require changing a canonical contract. This is
the same judgment the implementer applies at run time ("Issue-flavored implementer
prompt verifies against existing canon", which kicks an issue back to the changes
lane), pulled forward to authoring time so the re-route happens before the unit is
committed and queued, not after an implementer run. The two are complementary: the
authoring check catches the common case early; the implementer kick-back remains
the backstop.

## D7 — Cost

When `[canon]` is enabled, an audit-authored spec-lane change is checked twice (once
at authoring, once at implement). The authoring run self-heals; the implement run
then confirms a clean result. Given the per-run proposal cap (default 2) and
periodic audit cadence, the extra session is negligible, and it buys changes that
do not bounce back to the operator. a02 adds no skip-the-backstop optimization; the
implement-time gate is unchanged.

## D8 — Legibility guard (the measurement-hacking boundary)

Self-heal SHALL NOT make a `[canon]` finding vanish by silently bending the
contradicted requirement to fit. It prefers aligning the change to canon or
converting to an issue; when the correct resolution genuinely is to change a
canonical contract, it writes a `MODIFIED` delta AND states the contract change
plainly in the proposal's rationale (carried from a01's legibility rule), so the PR
reviewer sees a deliberate contract change rather than a laundered one.
