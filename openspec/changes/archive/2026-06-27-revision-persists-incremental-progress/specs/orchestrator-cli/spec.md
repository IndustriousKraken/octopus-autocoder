## ADDED Requirements

### Requirement: The spec-revision executor persists incremental progress across send-it rounds
The spec-revision executor SHALL preserve forward progress across `send it` rounds instead of restarting from base each round. Concretely: when a `send it` exhausts its bounded converge attempts with a contradiction remaining, the executor SHALL persist the round's accumulated spec-delta edits on the REVISION BRANCH — NEVER the base branch (human review of the PR remains the sole merge gate, per `Send it in a revision thread runs the spec-revision executor`) — AND the next `send it` for that change SHALL resume from the persisted revision branch rather than recreating it from base. Fixes therefore accumulate across rounds, making convergence monotonic rather than restarting from the same contradictory base every round.

Persistence is GUARDED to avoid locking in a regression. The executor SHALL persist a round's edits ONLY when the round did not INCREASE the change-internal contradiction set — that is, when no change-internal contradiction identity is present after the round that was absent before it (using the same contradiction-identity comparison the executor already performs for survivor detection). When a round INCREASES the contradiction set, the executor SHALL DISCARD that round's edits, reverting to the prior persisted state (or to base when no prior round persisted), so a regression is never carried forward.

This requirement does NOT change any other terminal behavior of the spec-revision executor: a clean re-gate still opens a PR and reports the link; an unreadable thread still refuses without revising blind; a scope/edit-guardrail violation still discards; a gate that could-not-run is still terminal. The `.needs-spec-revision.json` marker SHALL remain until a clean re-gate, AND no PR SHALL open until a re-gate is clean. The only behavior this requirement changes is the fate of a non-regressing failed round's EDITS — previously discarded, now persisted on the revision branch for the next round to build on.

#### Scenario: A non-regressing failed round persists and the next send it resumes from it
- **WHEN** a `send it` exhausts its converge attempts with a contradiction remaining AND the round did not increase the change-internal contradiction set
- **THEN** the round's accumulated spec-delta edits are persisted on the revision branch (never the base branch)
- **AND** the next `send it` for that change resumes from the persisted revision branch rather than recreating it from base
- **AND** no PR is opened and the `.needs-spec-revision.json` marker remains

#### Scenario: A regressing round is discarded rather than persisted
- **WHEN** a failed round introduces a change-internal contradiction identity that was absent before the round
- **THEN** the round's edits are discarded, reverting to the prior persisted state (or to base when no prior round persisted)
- **AND** the regression is not carried forward into the next `send it`

#### Scenario: The merge gate and marker semantics are unchanged
- **WHEN** the executor persists progress on a failed round
- **THEN** it does NOT commit the revision to the base branch outside the PR, and it does NOT open a PR
- **AND** the `.needs-spec-revision.json` marker remains until a re-gate is clean, at which point a PR is opened for human review
