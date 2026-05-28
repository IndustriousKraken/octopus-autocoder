## ADDED Requirements

### Requirement: OPERATIONS.md and CHATOPS.md document the queue-blocking change and the ignore verbs
`docs/OPERATIONS.md`'s "Perma-stuck change detection" section SHALL describe the new queue-blocking behavior. `docs/OPERATIONS.md` SHALL also include a Queue-blocking-policy section (or extend the existing one) enumerating every marker that blocks the queue AND noting that `.ignore-for-queue.json` downgrades any of them. `docs/CHATOPS.md`'s operator-recovery-commands section SHALL document the two new verbs (`ignore-and-continue` AND `clear-ignore`) with example reply shapes.

#### Scenario: OPERATIONS.md perma-stuck section names the new queue-blocking behavior
- **WHEN** an operator reads `docs/OPERATIONS.md`'s perma-stuck section
- **THEN** a paragraph describes the new behavior: a `.perma-stuck.json` marker blocks subsequent pending changes in the same repo
- **AND** the paragraph names the escape hatch (`@<bot> ignore-and-continue`) AND when an operator might want it (sibling changes that don't depend on the perma-stuck one)
- **AND** cross-links to `docs/CHATOPS.md` for the verb syntax

#### Scenario: OPERATIONS.md enumerates the four blocking-marker categories
- **WHEN** an operator reads `docs/OPERATIONS.md`'s queue-blocking-policy discussion
- **THEN** the section enumerates the four markers that block the queue: `.in-progress*` (AskUser waiting), `.needs-spec-revision.json` (agent-flagged or `a17`-flagged), `.perma-stuck.json`, AND any extension markers future specs may add
- **AND** the section notes that `.ignore-for-queue.json` downgrades any of them

#### Scenario: CHATOPS.md documents the two new verbs with examples
- **WHEN** an operator reads `docs/CHATOPS.md`'s operator-recovery-commands section
- **THEN** rows for `ignore-and-continue` AND `clear-ignore` appear in the verbs table
- **AND** each verb has an example reply (happy path AND refusal path)
- **AND** the section cross-links back to OPERATIONS.md for the underlying queue-blocking model
