## MODIFIED Requirements

### Requirement: Send it in a revision thread runs the spec-revision executor
`@<bot> send it` in a revision thread SHALL run the spec-revision executor: a write-scoped agentic session that edits the flagged change's spec deltas to resolve the contradiction, then re-runs the `[in]` AND `[canon]` checks against the revised change before producing any output.

The executor SHALL be grounded in two sources: the operator's direction, reconstructed from the thread transcript, AND the change's CURRENT contradiction set, read from the `.needs-spec-revision.json` marker (per "The spec-revision marker carries the current contradiction set"). The executor SHALL address EVERY contradiction the marker currently records, not only one.

Because the operator's direction lives in the thread, the executor SHALL read the transcript with a bounded retry; if the transcript still cannot be read, the executor SHALL NOT revise blind — it SHALL open NO PR AND report in the thread that it could not read the discussion so the operator can retry. A revision SHALL never be performed against an empty discussion.

The executor MAY re-edit and re-run the gates up to a small bounded number of attempts within a single `send it`, accumulating fixes on the revision branch, so a change with multiple contradictions is resolved in one `send it` rather than one per round. On a clean re-gate the executor SHALL open a PR carrying the change's spec-delta revision AND report the PR link in the thread; when the bounded attempts are exhausted AND a contradiction remains, the executor SHALL open NO PR AND report the remaining contradiction in the thread (the operator may discuss further AND `send it` again). When the SAME conflicting requirement survives the bounded attempts, the report SHALL name that specific requirement AND that the revision is not clearing it, rather than an identical generic failure, so a persistent non-convergence is legible rather than an opaque loop.

The executor SHALL NOT commit a spec revision to the base branch outside the PR — human review of the PR is the merge gate — AND SHALL NOT auto-edit a `tasks.md` to dodge the executor's unimplementable-tasks flag (that separate marker keeps its operator-authored invariant). The revision is to the change's spec deltas to achieve canon-consistency, performed under operator direction (the thread) AND human review (the PR).

#### Scenario: Send-it revises, re-gates clean, and opens a PR
- **WHEN** an operator `send it`s a revision thread AND the executor's revision passes the re-run `[in]` and `[canon]` checks
- **THEN** the executor opens a PR carrying the change's spec-delta revision
- **AND** it reports the PR link in the thread
- **AND** it does not merge the PR or commit the revision to the base branch outside the PR

#### Scenario: A revision that still contradicts opens no PR and reports back
- **WHEN** the executor's revision still fails the re-run `[in]` or `[canon]` check
- **THEN** no PR is opened
- **AND** the remaining contradiction is reported in the thread so the operator can discuss further and `send it` again

#### Scenario: The unimplementable-tasks invariant is preserved
- **WHEN** the spec-revision executor runs
- **THEN** it revises the change's spec deltas to resolve the contradiction
- **AND** it does NOT auto-edit a `tasks.md` to make an unimplementable-tasks flag pass (that marker's operator-authored flow is untouched)

#### Scenario: An unreadable thread aborts rather than revising blind
- **WHEN** an operator `send it`s a revision thread AND the thread transcript cannot be read after the bounded retry
- **THEN** the executor opens NO PR AND does not run a revision against an empty discussion
- **AND** it reports in the thread that it could not read the discussion so the operator can retry

#### Scenario: The executor resolves every contradiction the marker records
- **WHEN** the marker records more than one contradiction AND an operator `send it`s
- **THEN** the executor's revision addresses every recorded contradiction, not only the first
- **AND** a single `send it` can clear a marker that records multiple contradictions

#### Scenario: A bounded converge loop resolves multiple contradictions in one send it
- **WHEN** the first edit's re-gate still finds a contradiction AND the bounded attempt budget is not exhausted
- **THEN** the executor re-edits AND re-gates again within the same `send it`, accumulating fixes
- **AND** it opens a PR once a re-gate is clean, without requiring the operator to `send it` again

#### Scenario: Persistent non-convergence names the stuck requirement
- **WHEN** the same conflicting requirement survives the bounded attempts
- **THEN** the thread report names that specific requirement AND states the revision is not clearing it
- **AND** it does not repeat an identical generic "still fails" message that hides which requirement is stuck

## ADDED Requirements

### Requirement: The spec-revision marker carries the current contradiction set
The `.needs-spec-revision.json` marker SHALL be the durable record of what currently contradicts in a flagged change, kept current as the contradiction is worked, so a revision is always grounded in the present contradiction rather than a stale one. The marker is first written by the gate or pre-flight that flags the change. When a `@<bot> send it` re-gate still finds contradictions, the daemon SHALL refresh the marker's recorded contradiction set to that re-gate's CURRENT findings, replacing the prior set, so a subsequent revision attempt — including one that cannot read the chat thread — is grounded in the current contradiction rather than the original pre-flight finding.

The marker SHALL record the contradictions with enough structure to enumerate each distinct conflict — the conflicting requirement, and for a `[canon]` finding the conflicting canonical requirement's capability — so the executor can address each one. The refresh SHALL be best-effort: a failure to write the marker is logged AND does not change the revision outcome. The refresh updates the marker's findings; it does NOT clear the marker (clearing remains its own concern) AND does NOT commit it (the marker remains gitignored runtime state, never carried into the PR).

#### Scenario: A re-gate that still contradicts refreshes the marker
- **WHEN** a `send it` re-gate still finds one or more contradictions
- **THEN** the daemon refreshes `.needs-spec-revision.json` so its recorded contradiction set reflects that re-gate's current findings, replacing the prior set
- **AND** the refresh does not open or block a PR, and a failure to write it does not change the revision outcome

#### Scenario: A later revision attempt is grounded in the refreshed marker
- **WHEN** a subsequent `send it` runs after the marker was refreshed — including when the chat thread cannot be read
- **THEN** the executor is grounded in the marker's current findings, not the original pre-flight narrative
- **AND** it does not re-attempt a fix for a contradiction that the prior re-gate already superseded
