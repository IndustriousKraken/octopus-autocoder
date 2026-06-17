## ADDED Requirements

### Requirement: Global rules are authored as minimal prose, not contract language
The global rule corpus SHALL be a collection of project-agnostic rules that the `[rules]` gate checks changes against. Each rule SHALL be authored as minimal prose, deliberately NOT in OpenSpec's contract language: there are NO `SHALL`/`MODIFY`/`ADD`/`REMOVE`/`RENAME` deltas, NO scenarios, NO task lists, AND NO archive/compose step. A rule is interpreted by judgment, not algorithm; contract keywords add authoring friction without adding checkability, since the gate's model judges the prose directly.

Each rule SHALL carry: (a) a one-sentence **`rule`** — the assertion that is checked; (b) an OPTIONAL **`intent`** — a short rationale/exceptions paragraph that informs the judgment AND feeds retrieval; AND (c) a **stable identifier** so a gate finding can name the rule it violated. Rules SHALL be edited directly — git history is the change record; there is no delta or archive lifecycle (those serve OpenSpec's edit-the-canon model, which rules do not have).

The corpus MAY be a flat collection OR grouped into registers of related rules. The only structural pressure the protocol anticipates is **retrieval at scale**: while the corpus is small the gate feeds all rules to its session; as it grows past the context window, a relevant subset is selected (coarse by register, then semantic). The protocol SHALL NOT grow toward machine-instruction formality — judgment is irreducible.

#### Scenario: A rule is minimal prose with a stable id
- **WHEN** a rule is authored in the corpus
- **THEN** it is a one-sentence assertion plus an optional rationale plus a stable identifier
- **AND** it uses no `SHALL`, no delta keywords, no scenarios, and no task list

#### Scenario: Rules are edited directly, with no archive lifecycle
- **WHEN** a rule is changed
- **THEN** it is edited in place and the change is recorded by git history
- **AND** there is no delta block, no archive step, and no canon-compose step for rules

#### Scenario: A violation finding names the rule by its id
- **WHEN** the `[rules]` gate reports a violation
- **THEN** the finding names the violated rule by its stable identifier

#### Scenario: The corpus may be flat or grouped, and scales by retrieval
- **WHEN** the corpus is small
- **THEN** the gate feeds all rules to its session
- **AND** the format supports grouping into registers and later relevant-subset selection without changing the rule shape
