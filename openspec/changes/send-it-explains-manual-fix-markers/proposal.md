# Make the manual-fix `.needs-spec-revision.json` alerts explain themselves

## Why

A `.needs-spec-revision.json` marker has three causes, but only one of them is
`send it`-able. A `[in]` / `[canon]` / `[rules]` CONTRADICTION posts a TRACKED
revision thread that registers a `RevisionThreadState`, so `@<bot> send it` in
that thread runs the spec-revision executor. The other two causes — an
UNARCHIVABLE-DELTAS hold (a delta header that won't fold at archive time) and a
GATE-ERROR hold (a verifier gate that could not run) — are MANUAL-FIX holds:
`send it`'s revision executor cannot fix a delta-header/canon mismatch or a
broken gate, so those need a manual fix (see
`autocoder/src/polling_loop/alerts_throttle.rs:172-174` — "the gate-error AND
unarchivable-deltas markers keep the untracked path; those markers are NOT
tracked as revision threads").

The UX problem: all three causes use the same "spec needs revision" wording. The
alert body for a manual-fix marker advertises nothing about its being manual, so
an operator reasonably replies `@<bot> send it` out of habit from contradiction
threads — and gets the GENERIC untracked-thread refusal ("This reply is in a
thread autocoder is not tracking. The send it verb only acts in an
audit-notification, brownfield-survey, issue-candidate, or spec-revision
thread."). That refusal does not tell the operator that THIS alert is a
manual-fix category, nor what to do instead.

## What Changes

- The fix lands at ALERT-POST time, in the alert body — NOT in the `send it`
  routing. When autocoder posts an UNARCHIVABLE-DELTAS or GATE-ERROR
  `.needs-spec-revision.json` alert, the alert body SHALL state that the change is
  held for a MANUAL spec fix (naming the cause), that `@<bot> send it` cannot
  revise it, AND that the operator should fix it manually and then post
  `@<bot> clear-revision` to clear the hold.
- The contradiction-marker definition is refined to ALSO require an empty
  `unarchivable_deltas` array (today it only requires empty `unimplementable_tasks`
  AND empty `gate_error`), so an unarchivable-deltas marker is correctly treated
  as a manual-fix hold and records NO `RevisionThreadState`.
- The `send it` routing is UNCHANGED: a manual-fix thread records no
  `RevisionThreadState`, so a later `@<bot> send it` there falls through to the
  existing generic untracked-thread refusal. The operator already learned what to
  do from the alert body, so the generic refusal needs no new text.
- Scope stays tight: this does NOT make the unarchivable-deltas / gate-error
  markers revisable, adds NO new `send it` thread context, and touches NO
  `chatops-manager` requirement.

## Why this shape (vs. a targeted send-it refusal)

An earlier design recorded a parallel manual-fix thread mapping and routed
`send it` in those threads to a cause-specific refusal. That required MODIFYing
the generic untracked-thread refusal — whose four-set triggering condition AND
exact text are pinned independently in FOUR canonical requirements
(`orchestrator-cli`'s `send it verb in an audit thread...` plus three
`chatops-manager` routing requirements). A one-line UX improvement should not
require a consistent rewrite of a four-requirement contract. Putting the
explanation in the alert body delivers the same operator value (they are told the
fix up front) while touching exactly one requirement and zero of the pinned
refusal contract. The duplicated refusal contract is a separate consolidation
concern, tracked independently.

## Impact

- Affected specs: `orchestrator-cli` — ONE MODIFIED requirement
  (`Spec-revision contradiction alert is a tracked, discussable thread`): refine
  the contradiction-marker definition to require empty `unarchivable_deltas`, and
  require the manual-fix alert body to explain the manual fix. No `chatops-manager`
  change.
- Affected code: the alert posters for the unarchivable-deltas pre-flight AND the
  gate-error hold (compose the explanatory alert body; record NO
  `RevisionThreadState`). The inbound `send it` dispatcher is UNCHANGED.
- Independent change; it touches no requirement another in-flight change modifies.
