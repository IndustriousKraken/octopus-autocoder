## MODIFIED Requirements

### Requirement: Hybrid issue ingestion with maintainer promotion
The daemon SHALL ingest reported issues without giving public authors the ability to trigger code work. It SHALL triage reported GitHub issues read-only (reusing scout's issue read), classify AND dedup each against open AND archived issues, draft a candidate `issues/<slug>/`, AND post the candidate to chatops WITHOUT queuing it. A maintainer SHALL promote a candidate with a "send it" (reusing the audit send-it pattern); ONLY on promotion does the daemon write `issues/<slug>/` AND queue it. The public can REPORT but SHALL NOT TRIGGER code work — promotion is the authorization gate. The curated path (a009) is this path minus the auto-triage step.

The candidate notification SHALL be posted in a way that a later promotion reply can be matched to it: the daemon SHALL capture the posted message's `thread_ts` AND `channel` AND persist them on the candidate's stored state. A candidate whose thread was not captured (a degraded post) is simply not matchable by a reply — graceful degradation, never an error. The notification SHALL instruct the maintainer to reply `@<bot> send it` (the mention form that the verb recognizes), retaining the statement that nothing is written OR queued until they do.

Promotion SHALL be performed by a control-socket action reachable from the `send it` dispatcher. The action SHALL resolve the matched candidate, write `issues/<slug>/` (its `issue.md` AND `tasks.md`, plus the quarantined `report-body.md` for a public-origin candidate), AND flip the candidate's status to promoted; writing the unit IS the queue (the issues-lane walker picks up any ready `issues/<slug>/`). The action SHALL be idempotent: an already-promoted candidate writes nothing further AND reports that it is already promoted.

#### Scenario: A triaged report posts a candidate and queues nothing
- **WHEN** a reported issue is triaged
- **THEN** a candidate `issues/<slug>/` is drafted and posted to chatops
- **AND** nothing is written to `issues/` or queued

#### Scenario: Promotion writes and queues
- **WHEN** a maintainer "send it"s a posted candidate
- **THEN** the daemon writes `issues/<slug>/`
- **AND** queues it for the issues lane

#### Scenario: An unpromoted candidate does no work
- **WHEN** a candidate is posted but no maintainer promotes it
- **THEN** no issue is written or queued

#### Scenario: Duplicates are deduped
- **WHEN** a report duplicates an open or an archived issue
- **THEN** it is deduped AND no candidate is queued

#### Scenario: The candidate notification is matchable and instructs the mention form
- **WHEN** a candidate is posted to chatops
- **THEN** the posted message's `thread_ts` AND `channel` are persisted on the candidate's stored state
- **AND** the notification instructs the maintainer to reply `@<bot> send it`

#### Scenario: The promotion action writes, queues, and flips status
- **WHEN** the promotion control-socket action runs for a posted candidate
- **THEN** the daemon writes `issues/<slug>/` (including the quarantined `report-body.md` for a public-origin candidate)
- **AND** the candidate's stored status becomes promoted
- **AND** the written unit is ready for the issues-lane walker

#### Scenario: The promotion action is idempotent
- **WHEN** the promotion control-socket action runs for a candidate that is already promoted
- **THEN** no further filesystem write is performed
- **AND** the action reports that the candidate is already promoted
