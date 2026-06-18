## ADDED Requirements

### Requirement: Issues lane parks a non-progressing issue
The issues lane SHALL NOT re-attempt the same issue indefinitely. The issues walker SHALL track a per-issue consecutive-failure counter (its own lane state, per the independent-lane-walkers requirement) AND, once an issue stops making progress, SHALL PARK it: write a `.perma-stuck.json` marker into `issues/<slug>/`, exclude the issue from selection while the marker is present, AND post an operator-visible chatops alert. The threshold SHALL be the existing `executor.perma_stuck_after_failures` value (no new configuration). The operator unparks an issue by removing the marker, exactly as for a parked change.

Progress is defined by outcome:
- A RETRYABLE failure (executor error, a `Completed` outcome that left the workspace unmodified, an unsupported iteration request, OR a precondition-unmet outcome) SHALL increment the counter; the issue is parked when the counter reaches `executor.perma_stuck_after_failures`.
- An outcome that retrying cannot resolve — the agent escalating a question (the issues lane does not escalate) OR the agent kicking the fix back to the changes lane (it requires a behavior change) — SHALL park the issue IMMEDIATELY (a single attempt, not the full threshold). Immediate parking on kick-back also stops the kick-back notice from re-posting on every pass.
- A daemon-shutdown abort SHALL NOT count toward the threshold (operator-initiated shutdown is not an issue failure).

Parking SHALL be fail-loud, never silent: the chatops alert names the issue, the attempt count, AND the last reason, so the lane is never silently re-attempting an issue NOR silently abandoning one. Completion (the fix landed AND the issue archived) SHALL clear both the counter AND the marker, so a later issue reusing the slug starts clean. The marker file SHALL reuse the `.perma-stuck.json` name already excluded via `.git/info/exclude`, so it is gitignored at any depth AND survives the per-iteration branch reset AND `git clean`.

#### Scenario: A repeatedly failing issue is parked after the threshold
- **WHEN** an issue's fix fails on `executor.perma_stuck_after_failures` consecutive passes
- **THEN** a `.perma-stuck.json` marker is written into `issues/<slug>/`
- **AND** an operator-visible chatops alert names the issue, the attempt count, AND the last reason

#### Scenario: A parked issue is skipped until the operator removes the marker
- **WHEN** an `issues/<slug>/` carries a `.perma-stuck.json` marker
- **THEN** the issue is excluded from selection (it is not worked)
- **AND** removing the marker makes the issue selectable again

#### Scenario: An escalated issue is parked immediately
- **WHEN** the agent escalates a question while working an issue
- **THEN** the issue is parked on that single attempt (not after the full threshold)
- **AND** the operator is alerted

#### Scenario: A kicked-back issue is parked immediately and not re-reported
- **WHEN** the agent reports that an issue requires a behavior change (a kick-back to the changes lane)
- **THEN** the issue is parked on that single attempt
- **AND** the kick-back notice is not re-posted on subsequent passes

#### Scenario: A daemon-shutdown abort does not count toward the threshold
- **WHEN** an issue's session is aborted by the daemon's shutdown cascade
- **THEN** the issue's consecutive-failure counter is not incremented
- **AND** the issue is not parked for the abort

#### Scenario: Completion clears the counter and the marker
- **WHEN** an issue's fix completes AND the issue is archived
- **THEN** the per-issue failure counter is cleared
- **AND** no `.perma-stuck.json` marker remains for that slug
