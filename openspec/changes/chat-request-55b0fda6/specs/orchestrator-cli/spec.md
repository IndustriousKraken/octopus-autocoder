## ADDED Requirements

### Requirement: The spec-revision executor retains the needs-spec-revision marker until the revision lands
When `@<bot> send it` runs the spec-revision executor (per "Send it in a revision thread runs the spec-revision executor") AND a clean re-gate opens the revision PR, the daemon SHALL NOT clear that change's local `.needs-spec-revision.json` marker. The marker SHALL remain, holding the change out of `list_pending`, until the revision is applied to the base branch AND an operator clears it (via `@<bot> clear-revision`, per the clear-revision verb) — at which point the change re-enters `list_pending` and re-gates cleanly against the now-revised base deltas.

This is the deliberate distinction from "A successfully applied revision clears the change's needs-spec-revision marker": THAT requirement clears the marker because the `@<bot> revise` dispatcher applies the revision to an open PR on the AGENT BRANCH, which the polling loop's "Skip iteration when an open PR exists for the agent branch" check queries — so the open PR parks the whole repository AND the marker's hold is redundant. The spec-revision executor instead opens its PR on a DEDICATED per-change branch (`<agent-branch>-spec-revision-<change-slug>`) that the agent-branch open-PR check does NOT query, so that PR does NOT park the loop. If the marker were cleared at revision-PR-open time, the change would re-enter `list_pending` AND the next iteration's queue walk would re-run the `[in]` / `[canon]` gate against the change's still-unmerged, contradictory base-branch deltas — burning gate tokens AND immediately re-writing the marker it just cleared. Retaining the marker is what actually holds the change until its revision lands.

A revision PR closed WITHOUT merging leaves the marker in place, correctly keeping the still-contradictory change held; the marker's clear path for this flow is operator-driven (`@<bot> clear-revision`), consistent with the marker's canonical operator-cleared semantics. This requirement does NOT change the existing revision behavior in which the executor opens the PR on the per-change branch, flips the thread status to `Acted`, AND reports the PR link in the thread — it changes ONLY that the marker is no longer deleted on that path.

#### Scenario: A clean re-gate opens the revision PR but retains the marker
- **GIVEN** a change with a `.needs-spec-revision.json` marker present AND an operator `send it`s its revision thread
- **WHEN** the spec-revision executor's revision passes the re-run `[in]` and `[canon]` checks AND the executor opens the revision PR on the per-change branch
- **THEN** the daemon does NOT delete the change's `.needs-spec-revision.json` marker
- **AND** the change remains excluded from `list_pending` (the marker's hold is intact)
- **AND** the revision thread's status still flips to `Acted` AND the PR link is reported in the thread

#### Scenario: The held change is not re-gated while the revision PR is open
- **GIVEN** the spec-revision executor has opened a revision PR for a change AND retained its `.needs-spec-revision.json` marker
- **WHEN** a subsequent polling iteration runs AND the revision PR (on `<agent-branch>-spec-revision-<change-slug>`) is still open AND unmerged
- **THEN** the marker keeps the change out of `list_pending`, so the `[in]` / `[canon]` gate is NOT re-run against that change
- **AND** no new `.needs-spec-revision.json` marker write NOR revision alert is produced for it

#### Scenario: The operator clears the marker after merging the revision PR
- **WHEN** the operator merges the revision PR AND then runs `@<bot> clear-revision <repo> <change>`
- **THEN** the marker is removed AND the change re-enters `list_pending`
- **AND** the next iteration re-runs the `[in]` / `[canon]` gate against the now-revised base-branch deltas, which pass, AND the change proceeds to the executor
