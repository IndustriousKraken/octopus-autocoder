## ADDED Requirements

### Requirement: The spec-revision executor holds the per-repo busy marker for its run
When the daemon runs a `send it` spec-revision (the spec-revision executor that drains a revision-execute request and runs the bounded edit → re-gate converge loop), it SHALL acquire/stamp the per-repo busy marker for the DURATION of that revision, recording the change slug being revised, AND SHALL release the marker when the revision completes via ANY terminal path: a clean re-gate that opens a PR, the converge budget exhausted with a contradiction remaining, a scope/edit-guardrail violation, a gate that could-not-run, an unreadable discussion thread, OR an error.

This brings the revision under the same per-repo concurrency control as a normal pass (per `Per-repo busy marker prevents concurrent work`): a normal pass and a spec-revision SHALL NOT run concurrently on one workspace. It also makes an in-flight revision VISIBLE to the `status` verb: because the `currently:` line is computed from the per-repo busy marker (per `Status reply always shows live workspace snapshot`), and that requirement's branching renders a stamped marker whose `change` is non-empty as `working on <change> (started <age> ago)`, a `status` issued while a revision is editing or re-gating SHALL reflect the change under revision rather than reporting `idle`.

Before this requirement the spec-revision executor stamped no marker, so a `status` issued during an active revision read `currently: idle` — a false negative the marker now prevents (consistent with the existing rule that status MUST NOT report `idle` when a marker is stamped). This requirement does NOT add a revision-specific `currently:` variant; surfacing the change via the existing change-non-empty branch is sufficient to remove the false-idle.

#### Scenario: A revision in flight is not reported as idle
- **WHEN** the spec-revision executor is actively revising change `c03-adaptive-selection` (editing its spec deltas or re-running the `[in]` / `[canon]` gates)
- **AND** an operator issues `status <repo>` for that repo
- **THEN** the per-repo busy marker is stamped with `c03-adaptive-selection`, so the `currently:` line reads `working on c03-adaptive-selection (started <age> ago)` per the live-busy-marker branching
- **AND** the `currently:` line does NOT read `idle`

#### Scenario: The marker is released on every terminal path
- **WHEN** a revision completes — whether by a clean re-gate opening a PR, the converge budget exhausting with a contradiction remaining, a scope/edit-guardrail violation, a gate that could-not-run, an unreadable discussion thread, OR an error
- **THEN** the per-repo busy marker is released
- **AND** a subsequent `status` reflects the post-revision state (idle, or the next unit of work) with no lingering revision marker

#### Scenario: A pass and a revision do not run concurrently on one workspace
- **WHEN** a spec-revision holds the per-repo busy marker for a repo
- **THEN** a normal pass for that repo cannot acquire the marker until the revision releases it, per `Per-repo busy marker prevents concurrent work`
