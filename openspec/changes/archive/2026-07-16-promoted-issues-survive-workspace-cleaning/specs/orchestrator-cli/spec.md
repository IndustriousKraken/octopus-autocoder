## MODIFIED Requirements

### Requirement: Hybrid issue ingestion with maintainer promotion
The daemon SHALL ingest reported issues without giving public authors the ability to trigger code work. It SHALL triage reported GitHub issues read-only (reusing scout's issue read), classify AND dedup each against open AND archived issues, draft a candidate `issues/<slug>/`, AND post the candidate to chatops WITHOUT queuing it. A maintainer SHALL promote a candidate with a "send it" (reusing the audit send-it pattern); ONLY on promotion does the daemon write the issue unit AND queue it. The public can REPORT but SHALL NOT TRIGGER code work — promotion is the authorization gate. The curated path (a009) is this path minus the auto-triage step.

The candidate notification SHALL be posted in a way that a later promotion reply can be matched to it: the daemon SHALL capture the posted message's `thread_ts` AND `channel` AND persist them on the candidate's stored state. A candidate whose thread was not captured (a degraded post) is simply not matchable by a reply — graceful degradation, never an error. The notification SHALL instruct the maintainer to reply `@<bot> send it` (the mention form that the verb recognizes), retaining the statement that nothing is written OR queued until they do.

Promotion SHALL be performed by a control-socket action reachable from the `send it` dispatcher. The action SHALL resolve the matched candidate AND write the issue unit in the form appropriate to its origin: a CURATED candidate (carrying no untrusted body) as the default single file `issues/<slug>.md` (a description plus an optional `## Tasks` checklist); a PUBLIC-ORIGIN candidate as the directory form `issues/<slug>/` (its `issue.md` AND `tasks.md`, plus the quarantined `report-body.md`) so the untrusted body stays a separate file from the maintainer-approved task. The action SHALL flip the candidate's status to promoted. The action SHALL be idempotent: an already-promoted candidate writes nothing further AND reports that it is already promoted.

**The promoted candidate record is the durable queue entry; the workspace unit is its materialization.** The candidate's stored state (which already carries the full drafted `issue_md`, `tasks_md`, AND `report_body`) SHALL be the source of truth for a promoted-but-not-yet-completed issue. The workspace unit is a materialized copy: because it sits as loose files in a mutable working tree, any workspace-cleaning path (dirty-tree recovery, workspace wipe, re-clone, another feature's failure cleanup) can destroy it, and that destruction SHALL NOT lose the queued issue. Each polling iteration, BEFORE issues-lane enumeration, the daemon SHALL reconcile: for every stored candidate of the repository with status promoted whose unit is absent from `issues/` (in either form) AND absent from `issues/archive/` (no entry whose name ends with the slug, in either form), the daemon SHALL re-materialize the unit from the record — identical content, identical form — logging a WARN naming the slug and that the previously-materialized unit had disappeared. A unit found in `issues/archive/` is complete and SHALL NOT be re-materialized. Deleting the candidate record file is the operator's tombstone: a promoted issue whose record is removed is never re-materialized. Reconciliation applies to pre-existing promoted records on first run after upgrade, so units destroyed before this requirement existed are healed without re-ingestion.

#### Scenario: A triaged report posts a candidate and queues nothing
- **WHEN** a reported issue is triaged
- **THEN** a candidate `issues/<slug>/` is drafted and posted to chatops
- **AND** nothing is written to `issues/` or queued

#### Scenario: Promotion writes and queues
- **WHEN** a maintainer "send it"s a posted candidate
- **THEN** the daemon writes the issue unit in the form appropriate to its origin (a single file `issues/<slug>.md` for a curated candidate, OR `issues/<slug>/` for a public-origin candidate)
- **AND** queues it for the issues lane

#### Scenario: A curated candidate is promoted as a single file
- **WHEN** the promotion action runs for a curated candidate (carrying no untrusted body)
- **THEN** the daemon writes the single file `issues/<slug>.md` (description plus an optional `## Tasks` checklist), NOT a directory
- **AND** the written unit is ready for the issues-lane walker

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
- **THEN** the daemon writes the issue unit in the form appropriate to origin (a single file `issues/<slug>.md` for a curated candidate, OR `issues/<slug>/` including the quarantined `report-body.md` for a public-origin candidate)
- **AND** the candidate's stored status becomes promoted
- **AND** the written unit is ready for the issues-lane walker

#### Scenario: The promotion action is idempotent
- **WHEN** the promotion control-socket action runs for a candidate that is already promoted
- **THEN** no further filesystem write is performed
- **AND** the action reports that the candidate is already promoted

#### Scenario: A destroyed unit is re-materialized on the next iteration
- **WHEN** a promoted candidate's workspace unit is destroyed before the issues lane works it (e.g. a dirty-tree recovery, workspace wipe, or another feature's failure cleanup removed the untracked files)
- **THEN** the next polling iteration's reconciliation re-writes the unit from the candidate record — identical content, identical form
- **AND** a WARN names the slug and the disappearance
- **AND** the issues lane selects and works it normally

#### Scenario: An archived issue is not resurrected
- **WHEN** a promoted candidate's fix has completed AND its unit moved to `issues/archive/`
- **THEN** reconciliation does NOT re-materialize the unit
- **AND** no WARN fires for it

#### Scenario: Deleting the candidate record retires the issue
- **WHEN** an operator deletes a promoted candidate's record file from the candidate store
- **AND** the workspace unit is (or later becomes) absent
- **THEN** reconciliation never re-materializes that issue

#### Scenario: Pre-existing destroyed promotions are healed on upgrade
- **WHEN** the daemon starts with this behavior for the first time AND the candidate store holds promoted records whose units are absent from both `issues/` and `issues/archive/`
- **THEN** the first iteration per repository re-materializes those units
- **AND** each re-materialization logs the WARN, so the operator can audit what had been silently lost
