# Tasks

## 1. Persist the revision branch across rounds

- [x] 1.1 At `send it` start (`revision_session.rs`), when a persisted revision branch from a prior round exists for this change, RESUME it instead of recreating it from base. When none exists, behave as today (create the revision branch from base).
- [x] 1.2 On the budget-exhausted branch (a contradiction remains after the bounded attempts), do NOT unconditionally `restore_base`. Instead apply the regression guard (task 2) to decide persist vs discard.

## 2. Regression guard

- [x] 2.1 Compare the change-internal contradiction set AFTER the round to the set BEFORE it (using the `ContradictionIdentity` the executor already computes for survivor detection). The round did NOT regress iff no contradiction identity is present after that was absent before.
- [x] 2.2 If the round did not regress, PERSIST the accumulated edits on the revision branch (commit them to that branch) so the next `send it` resumes from them. Never commit to the base branch outside a PR.
- [x] 2.3 If the round regressed (a new contradiction identity appeared), DISCARD the round's edits — revert to the prior persisted state (or base if none was persisted) — so a regression is not locked in.

## 3. Unchanged terminal semantics

- [x] 3.1 Keep all existing terminal behaviors intact: a clean re-gate opens a PR and reports the link; an unreadable thread refuses (no PR, no blind revision); a scope/edit violation discards; a gate that could-not-run is terminal. The `.needs-spec-revision.json` marker remains until a clean re-gate; no PR opens until clean.

## 4. Tests

- [x] 4.1 A non-regressing budget-exhausted round persists its edits on the revision branch, and the next `send it` resumes from the persisted state (the deltas are NOT reset to base between rounds).
- [x] 4.2 A round that introduces a new contradiction identity is discarded — the persisted state reverts to the prior round (or base if first), so the regression is not carried forward.
- [x] 4.3 The base branch is never committed to outside a PR; the marker persists until a clean re-gate; no PR is opened on a failed round.
