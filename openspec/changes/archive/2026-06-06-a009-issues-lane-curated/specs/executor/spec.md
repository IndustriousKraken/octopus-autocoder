# executor — delta for a009-issues-lane-curated

## ADDED Requirements

### Requirement: Issue-flavored implementer prompt verifies against existing canon
When the executor runs an issue (an `issues/<slug>/` unit, NOT a change), it SHALL use an issue-flavored implementer prompt that instructs: fix the code to match the EXISTING specification; do NOT invent or write a spec change; AND if the fix actually requires new or changed behavior, report that the item belongs in the changes lane (kick it back) rather than altering any spec. The prompt SHALL be loaded through the uniform PromptLoader AND declare its override field via the nested naming convention. Acceptance for an issue run SHALL be verified against the existing canon, not a spec delta.

#### Scenario: An issue run uses the issue-flavored prompt
- **WHEN** the executor runs an `issues/<slug>/` unit
- **THEN** it uses the issue-flavored implementer prompt (fix-to-existing-spec framing)
- **AND** not the change implementer prompt

#### Scenario: A behavior-change fix is kicked back to changes
- **WHEN** an issue's fix would require new or changed behavior
- **THEN** the run reports that the item belongs in the changes lane
- **AND** it does NOT modify any spec

#### Scenario: Acceptance is evaluated against canon
- **WHEN** an issue run completes
- **THEN** its acceptance is evaluated against the existing specification, not a spec delta
