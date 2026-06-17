## MODIFIED Requirements

### Requirement: Triage routing classifies each report
Triage SHALL classify each report AND route it accordingly: a **Bug** (code has drifted from a specification that is itself correct) becomes an issues-lane candidate; a **Behavior change** (the report wants new or changed behavior) is routed to the changes lane as a proposal, NOT an issue; a **Question, invalid report, or duplicate** is declined or deduped with no work queued.

Parsing the triage agent's verdict SHALL be total over arbitrary agent output: the parser SHALL NOT panic on any byte sequence the agent produces, including non-ASCII (multi-byte UTF-8) text such as accented Latin, CJK, or emoji. Because the agent's output is steered by an untrusted, public-author report, a verdict containing such characters SHALL be parsed (yielding a verdict or `None`) without crashing the issue-ingestion lane.

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

#### Scenario: Non-ASCII verdict output is parsed without panicking
- **WHEN** the triage agent's verdict text contains a multi-byte UTF-8 character at a byte offset that falls inside a label-prefix match (e.g. a line of CJK or emoji text)
- **THEN** the verdict parser returns normally (a parsed verdict or `None`)
- **AND** the issue-ingestion lane does not crash
