# executor — delta for a010-issues-lane-hybrid-ingestion

## ADDED Requirements

### Requirement: Public issue body is quarantined as untrusted data in the implementer prompt
When an issue originates from a public author, the implementer prompt SHALL embed the issue body as DATA inside a robust delimiter — NOT a markdown fence the body can break out of — with an explicit untrusted-report framing. The task AND scope SHALL come from the lane and the maintainer-approved classification, NEVER from the body. Single-pass substitution SHALL prevent `{{token}}` expansion of placeholder-looking text inside the body.

#### Scenario: The body is embedded as untrusted data
- **WHEN** a public-origin issue is run
- **THEN** its body is placed in a delimited untrusted-data region distinct from the instruction region
- **AND** the delimiter is not a markdown fence the body can break out of

#### Scenario: Body instructions do not become the task
- **WHEN** the issue body contains instruction-like text
- **THEN** the task is taken from the maintainer-approved classification, not from the body

#### Scenario: No token expansion inside the body
- **WHEN** the issue body contains `{{token}}`-looking text
- **THEN** it is not expanded during prompt construction
