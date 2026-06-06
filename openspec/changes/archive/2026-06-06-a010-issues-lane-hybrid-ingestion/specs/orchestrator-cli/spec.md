# orchestrator-cli — delta for a010-issues-lane-hybrid-ingestion

## ADDED Requirements

### Requirement: Hybrid issue ingestion with maintainer promotion
The daemon SHALL ingest reported issues without giving public authors the ability to trigger code work. It SHALL triage reported GitHub issues read-only (reusing scout's issue read), classify AND dedup each against open AND archived issues, draft a candidate `issues/<slug>/`, AND post the candidate to chatops WITHOUT queuing it. A maintainer SHALL promote a candidate with a "send it" (reusing the audit send-it pattern); ONLY on promotion does the daemon write `issues/<slug>/` AND queue it. The public can REPORT but SHALL NOT TRIGGER code work — promotion is the authorization gate. The curated path (a009) is this path minus the auto-triage step.

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

### Requirement: Triage routing classifies each report
Triage SHALL classify each report AND route it accordingly: a **Bug** (code has drifted from a specification that is itself correct) becomes an issues-lane candidate; a **Behavior change** (the report wants new or changed behavior) is routed to the changes lane as a proposal, NOT an issue; a **Question, invalid report, or duplicate** is declined or deduped with no work queued.

#### Scenario: A bug becomes an issue candidate
- **WHEN** triage classifies a report as a bug against a correct spec
- **THEN** it is drafted as an issues-lane candidate

#### Scenario: A behavior-change report routes to changes
- **WHEN** triage classifies a report as wanting new or changed behavior
- **THEN** it is routed to the changes lane as a proposal
- **AND** it is NOT written as an issue

#### Scenario: A question or duplicate is declined
- **WHEN** triage classifies a report as a question, invalid, or a duplicate
- **THEN** no work is queued
