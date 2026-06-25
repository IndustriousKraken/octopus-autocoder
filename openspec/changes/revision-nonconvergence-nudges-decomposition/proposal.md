# Repeated revision non-convergence nudges the operator to decompose the change

## Why

When `@<bot> send it` exhausts its bounded converge attempts with a contradiction
remaining, the executor reports the remaining contradiction and invites the
operator to "discuss further AND `send it` again" (per `Send it in a revision
thread runs the spec-revision executor`). For a normal change this is the right
loop. But for a large, highly interconnected change it becomes a trap: the `[in]`
gate surfaces only a few contradictions per run, each `send it` is bounded, and —
because nothing is committed on failure — every round restarts from the same
contradictory base. The result is an opaque, indefinite loop. This was observed in
production: one change (`c03-adaptive-selection`) failed roughly seven `send it`
rounds over two days, each ~1.5–2h, with zero net committed progress, while the
reply each round still said "send it again."

The system should recognize when a change is not converging and tell the operator
the likely real fix — decompose the change into smaller changes — rather than
inviting another fruitless round. A change that fails repeated revision rounds is
almost always too large or too interconnected to converge through `send it`.

## What Changes

- The daemon SHALL track the number of CONSECUTIVE failed `send it` rounds for a
  change (a counter carried in / alongside the `.needs-spec-revision.json` marker),
  resetting it when the change clears (a clean re-gate opening a PR) or the marker
  is cleared.
- When that count reaches a configurable threshold (default 3), the
  budget-exhausted failure reply SHALL — in addition to naming the remaining
  contradiction as it does today — recommend DECOMPOSING the change into smaller
  changes (stating that a change failing repeated revision rounds is likely too
  large/interconnected to converge via `send it`). The operator MAY still `send it`
  again, but decomposition is presented as the recommended path.
- Below the threshold the reply is UNCHANGED (names the remaining contradiction,
  invites another `send it`).

## Impact

- Affected specs: `orchestrator-cli` — one ADDED requirement. It extends the
  failure-reply behavior of `Send it in a revision thread runs the spec-revision
  executor` without restating or contradicting it (that requirement still names the
  stuck requirement and still permits another `send it`).
- Affected code: the spec-revision executor failure path
  (`autocoder/src/polling/revision_session.rs`, the budget-exhausted branch), the
  marker schema/state for the per-change consecutive-failure counter, and counter
  reset on a clean re-gate / marker clear.
- Independent of `revision-persists-incremental-progress` (which changes what
  happens to the EDITS on failure); this change only governs the REPLY text and a
  counter. The two compose: persistence makes convergence possible for moderate
  changes; this nudge catches the genuinely-too-large ones.
- Configurable threshold; default 3.
