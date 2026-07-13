## ADDED Requirements

### Requirement: Open spec-revision PR parks its change in the queue without re-running the gate
When the blocking-markers gate processes a pending change that has NO `.needs-spec-revision.json` marker, it SHALL also query GitHub for an open PR whose head branch is `<agent_branch>-spec-revision-<change-slug>`. When such a PR exists, the change is treated as parked — same blocking result and same queue-walk halt as when the marker is present — and the spec gate SHALL NOT be re-run for that change in this iteration.

The park persists across iterations until the spec-revision PR is merged or closed. On merge, the updated spec lands on the base branch; the gate re-runs normally on the next iteration against the new spec. On close without merge, the gate re-runs on the next iteration; if the spec still needs revision the marker is re-written by the gate as before.

On a forge query error for the spec-revision branch, the gate SHALL fail open (log a WARN, do NOT treat the change as parked) — consistent with the existing open-PR-check error policy: a transient GitHub failure must not permanently park a change.

This requirement works in tandem with `A successfully applied revision clears the change's needs-spec-revision marker`. That requirement ensures the marker is deleted when the revision PR opens; this requirement ensures the deleted marker is not immediately re-written by the gate running on the next iteration.

#### Scenario: Change is parked by an open spec-revision PR, not by the marker
- **WHEN** a pending change has no `.needs-spec-revision.json` marker
- **AND** an open PR exists on branch `agent-q-spec-revision-<change>`
- **THEN** the blocking-markers gate returns a blocking/halt-queue-walk result for that change
- **AND** the spec gate does NOT run for that change in this iteration
- **AND** no `.needs-spec-revision.json` marker is written

#### Scenario: Marker present — forge call for spec-revision PR is skipped
- **WHEN** a pending change already has a `.needs-spec-revision.json` marker
- **THEN** the blocking-markers gate returns blocking on the marker (short-circuit)
- **AND** no forge call is made for the spec-revision branch (the marker check is the early-exit)

#### Scenario: Park resolves on spec-revision PR close or merge
- **WHEN** the spec-revision PR is merged or closed
- **THEN** the next iteration finds no open spec-revision PR and no marker for that change
- **AND** the gate runs normally on the updated spec (merge) or unchanged spec (close)
- **AND** if the spec still needs revision after a close-without-merge, the gate re-writes the marker

#### Scenario: Forge query error for spec-revision branch fails open
- **WHEN** the GitHub query for an open PR on `agent-q-spec-revision-<change>` returns a transport error or non-2xx status
- **THEN** the gate logs a WARN naming the failure
- **AND** proceeds as if no spec-revision PR exists (does NOT park the change)
- **AND** does NOT block execution or halt the queue walk on this error alone
