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

### Requirement: `send it` verb in an audit thread schedules a triage executor run
The chatops listener SHALL recognize `@<bot> send it` (case-insensitive on `send it`) as the `SendItOnAudit` command ONLY when the message arrives with a non-empty `thread_ts` AND the `thread_ts` matches a tracked audit-thread state with `status: Open` OR `status: TriageFailed`. Same text outside a thread SHALL parse as the unknown-verb fallback (existing `?` reaction). When recognized, the dispatcher SHALL submit a `trigger_audit_action` control-socket action AND flip the audit-thread state's `status` to `TriagePending`. The next polling iteration drains the triage queue and runs the executor in triage mode.

#### Scenario: Send-it in tracked, open audit thread schedules triage
- **WHEN** an operator posts `@<bot> send it` as a thread reply where `thread_ts` matches an `AuditThreadState` with `status: Open` AND `posted_at` within the last 7 days
- **THEN** the dispatcher submits `trigger_audit_action` with the `thread_ts`
- **AND** the state file's `status` is updated to `TriagePending`
- **AND** the bot replies in the thread `✓ Triage scheduled for <audit_type> on <repo_url>. The next polling iteration will run it (~Nm).`

#### Scenario: Send-it in untracked thread is politely refused
- **WHEN** an operator posts `@<bot> send it` in a thread that matches no audit-thread, brownfield-survey, OR issue-candidate state
- **THEN** the bot replies `✗ This reply is in a thread autocoder is not tracking. The \`send it\` verb only acts in an audit-notification, brownfield-survey, or issue-candidate thread.`
- **AND** no control-socket action is submitted

#### Scenario: Send-it on stale audit thread is politely refused
- **WHEN** an operator posts `@<bot> send it` in a tracked thread whose `posted_at` is older than 7 days
- **THEN** the bot replies `✗ This audit's findings are too old to act on (>7d). Re-run the audit via @<bot> audit <type> <repo>.`
- **AND** the state file remains unchanged (the prune-stale-entries pass will eventually remove it)

#### Scenario: Send-it on already-acted thread is politely refused
- **WHEN** an operator posts `@<bot> send it` in a thread with `status: Acted` OR `status: TriagePending`
- **THEN** the bot replies `✗ This audit thread is already <status>. No new action taken.`
- **AND** no new triage is scheduled

#### Scenario: Send-it on TriageFailed thread re-attempts triage
- **WHEN** an operator posts `@<bot> send it` in a thread with `status: TriageFailed`
- **THEN** the dispatcher treats the request like the Open case (triage re-scheduled)
- **AND** the state's `status` is reset to `TriagePending` for the new attempt
