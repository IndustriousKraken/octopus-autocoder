# The spec-revision executor persists incremental progress across `send it` rounds

## Why

The spec-revision executor (`Send it in a revision thread runs the spec-revision
executor`) runs a bounded converge loop within ONE `send it`, accumulating fixes
on a revision branch. But across rounds it makes ZERO persisted progress: each
`send it` recreates the revision branch from base
(`revision_session.rs` `recreate_branch`), and on budget exhaustion it discards
everything (`restore_base`: `reset --hard` + `clean -fd` + checkout base). So every
`send it` restarts from the same contradictory base.

For a change with more independent contradictions than one converge budget can
clear — and where the `[in]` gate surfaces only a few per run — this never
converges: a round fixes a few, the gate reveals a few more, the budget runs out,
all edits revert. Observed in production: a change failed ~seven rounds over two
days, on-disk deltas unchanged from round one. The operator's careful per-round
guidance was applied and then thrown away each time.

Persisting progress across rounds makes convergence MONOTONIC: each `send it`
builds on the prior round's surviving fixes instead of restarting. Canon already
forbids committing a spec revision to the BASE branch outside the PR (human PR
review is the merge gate) — but it is silent on the revision branch, so progress
can persist THERE without weakening the merge gate.

## What Changes

- On budget exhaustion with a contradiction remaining, the executor SHALL persist
  the round's accumulated spec-delta edits on the REVISION BRANCH (never the base
  branch — human PR review remains the sole merge gate), AND the next `send it`
  SHALL resume from that persisted revision branch rather than recreating it from
  base. Fixes accumulate across rounds.
- Regression guard: the executor SHALL persist a round's edits ONLY if the round
  did not INCREASE the change-internal contradiction set (no contradiction identity
  present after the round that was absent before it). If the round increased the
  set, the executor SHALL DISCARD that round's edits (reverting to the prior
  persisted state, or base if none was persisted), so a regression is never locked
  in.
- The `.needs-spec-revision.json` marker remains until a clean re-gate; no PR opens
  until clean. None of the existing terminal behaviors (clean re-gate → PR,
  unreadable-thread refusal, scope violation, gate could-not-run) change their
  PR/marker semantics.

## Impact

- Affected specs: `orchestrator-cli` — one ADDED requirement. It is consistent
  with `Send it in a revision thread runs the spec-revision executor` (still no PR
  on a failed round; still names the remaining contradiction; never commits to the
  base branch outside the PR) — it fills the previously-unspecified question of
  what happens to the round's EDITS, replacing the implicit discard with guarded
  persistence on the revision branch.
- Affected code: `autocoder/src/polling/revision_session.rs` — the
  `recreate_branch`-from-base at `send it` start (resume the persisted branch when
  present) and the `restore_base` budget-exhausted branch (persist-or-discard by
  the regression guard), plus the contradiction-set comparison the guard needs
  (the executor already tracks `ContradictionIdentity` survivors).
- Composes with `revision-nonconvergence-nudges-decomposition` (that change governs
  the REPLY + a failure counter; this one governs the EDITS): persistence lets
  moderate changes converge; the nudge catches the genuinely-too-large ones.
- DESIGN NOTE for review: the regression guard's "did the contradiction set grow?"
  test depends on the `[in]` gate reporting comparable identities round to round.
  Because the gate surfaces a subset per run, the guard is conservative (it
  compares identities, not an absolute count) — confirm this policy before merge.
