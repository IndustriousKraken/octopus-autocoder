## ADDED Requirements

### Requirement: Repeated revision non-convergence recommends decomposition
The daemon SHALL track the number of CONSECUTIVE failed `send it` rounds for a change — a round being a `send it` whose spec-revision executor exhausts its bounded converge attempts with a contradiction remaining (the budget-exhausted outcome of `Send it in a revision thread runs the spec-revision executor`). The count SHALL be carried in or alongside the change's `.needs-spec-revision.json` marker, AND SHALL reset to zero when the change clears (a clean re-gate that opens a PR) OR when the marker is cleared (via `@<bot> clear-revision` or removal of the marker file).

When the consecutive-failure count reaches a configurable threshold (default 3), the budget-exhausted failure reply SHALL — IN ADDITION to naming the remaining contradiction as it already does — recommend that the operator DECOMPOSE the change into smaller changes, stating that a change failing repeated revision rounds is likely too large or too interconnected to converge via `send it`. This is additive to the existing failure reply: the operator MAY still `send it` again (the existing path is unchanged), but decomposition is presented as the recommended path after repeated non-convergence. Below the threshold the failure reply is UNCHANGED.

#### Scenario: At the threshold the reply recommends decomposition
- **WHEN** a change reaches the configured number of consecutive failed `send it` rounds (default 3)
- **THEN** the budget-exhausted failure reply names the remaining contradiction AND additionally recommends decomposing the change into smaller changes
- **AND** the reply states that repeated non-convergence indicates the change is likely too large or interconnected to converge via `send it`

#### Scenario: Below the threshold the reply is unchanged
- **WHEN** a change has fewer than the configured number of consecutive failed rounds
- **THEN** the failure reply names the remaining contradiction AND invites another `send it`, exactly as before
- **AND** it does NOT include the decomposition recommendation

#### Scenario: The consecutive-failure count resets when the change clears or is cleared
- **WHEN** a change's revision re-gates clean and a PR is opened, OR the change's `.needs-spec-revision.json` marker is cleared
- **THEN** the consecutive-failure count for that change resets to zero
- **AND** a subsequent first failure does not immediately trigger the decomposition recommendation
