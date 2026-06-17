## MODIFIED Requirements

### Requirement: Source files and functions stay within a size budget
The project SHALL treat source-file AND function length as a maintainability budget, not merely a metric an audit happens to report. A source file SHOULD stay at or under a target of roughly **500 lines** AND a function at or under roughly **50 lines**. These are judgment targets, NOT hard caps: genuinely cohesive, single-responsibility code MAY exceed them when splitting would add indirection without reducing complexity — the test is cohesion, not the line count.

The budget is surfaced **advisorily** by mechanisms that SAMPLE rather than exhaustively enumerate, so it never functions as a per-file or per-function gate. The `architecture_advisor` audit examines a bounded set of the longest files over a configurable pain threshold AND, by cohesion judgment, recommends refactoring the worst offenders — it samples the most over-budget FILES (not every file) and reasons about files rather than individual functions. Code review independently notes an over-budget file OR function when a pass enlarges it. Because the advisor samples the worst offenders rather than guaranteeing coverage, a file or function over the budget is a maintainability signal informing prioritization — NOT a defect the project is obligated to have surfaced on any given run, AND NOT, on its own, grounds to block a pull request or a change from archiving. Duplicated logic — near-identical function bodies, OR one intent reimplemented across files — is likewise a maintainability concern; because it spans the whole tree rather than one file, surfacing it is a corpus-level concern, not the per-file advisor's.

This requirement is the single canonical home of the size budget; the `architecture_advisor` audit AND the `Reviewer flags files and functions that breach the size brightline` requirement reference it rather than restating the thresholds.

#### Scenario: A file far past the threshold is eligible for an advisory recommendation
- **WHEN** a source file's length is well past the file-line threshold AND it is among the longest files the `architecture_advisor` samples
- **THEN** the advisor MAY recommend refactoring it by cohesion judgment, AND code review notes it when a pass enlarges it
- **AND** the size finding does not, on size alone, block a pull request or a change from archiving

#### Scenario: A cohesive file may exceed the target by judgment
- **WHEN** a file exceeds the ~500-line target but implements a single cohesive responsibility that splitting would only fragment into indirection
- **THEN** exceeding the target is not, by itself, a structural defect
- **AND** the `architecture_advisor` audit leaves it unflagged (size without a cohesion problem)

#### Scenario: Duplicated logic is a structural defect
- **WHEN** two or more functions share a near-identical body, OR one intent is reimplemented across files
- **THEN** the duplication is treated as a structural defect to be addressed
- **AND** because it spans files, surfacing it is a corpus-level concern rather than the per-file advisor's

### Requirement: The orchestrator polling loop is decomposed by responsibility
The polling-loop orchestration SHALL be organized as a directory module whose submodules each own a single responsibility — rather than a single flat file that mixes all of them. Distinct responsibilities such as alert/notification posting, queue walking, waiting-change handling, pre-flight checks, review-context assembly, pull-request construction, rebuild iteration, audit-triage handling, proposal handling, AND outcome handling SHALL each reside in their own submodule. No submodule SHALL exceed the file-size budget (per `Source files and functions stay within a size budget`), AND a function that exceeds the function-size budget SHALL be split along its internal phases rather than left as one oversized body. Unit tests SHALL reside in sibling test module(s), NOT in a `#[cfg(test)] mod tests` block inside the orchestration source.

This requirement gives the project's largest historical bloat hotspot a durable structural contract. The `architecture_advisor` surfaces a regression when the orchestration's files grow large enough to rank among the worst offenders it samples, AND code review flags a pass that re-accretes responsibilities into one file — advisory backstops that make silent re-accretion less likely, NOT a per-run coverage guarantee.

#### Scenario: Orchestration responsibilities live in separate submodules
- **WHEN** the polling-loop orchestration is evaluated
- **THEN** each distinct responsibility (alert posting, queue walking, waiting-change handling, pre-flight checks, review-context assembly, PR construction, rebuild, audit-triage, proposals, outcome handling) resides in its own submodule rather than in a single flat file
- **AND** no submodule exceeds the file-size budget

#### Scenario: No orchestration function exceeds the function budget
- **WHEN** a function in the polling-loop orchestration would exceed the function-size budget
- **THEN** it is split along its internal phases into smaller functions
- **AND** none of the resulting functions exceeds the function-size budget

#### Scenario: Tests are not inline in the orchestration source
- **WHEN** the polling-loop orchestration source is evaluated
- **THEN** its unit tests reside in sibling test module(s), not in a `#[cfg(test)] mod tests` block inside the orchestration source

#### Scenario: The relocated suite carries no wording-assertion tests
- **WHEN** the polling-loop test suite is evaluated against the `Tests assert behavior or derivation, never message wording` requirement
- **THEN** no test asserts a hand-authored substring of a shipped alert, notification, PR-body, OR marker message
- **AND** message-content intent is carried by requirement prose (verified by the drift audit), not by unit-test substring checks

## REMOVED Requirements

### Requirement: OPERATIONS.md describes the `.brightline-ignore` file and CHATOPS.md cross-links from `send it`

**Reason:** The `.brightline-ignore` file is removed with the duplicate-signature
metric that was its only consumer, so there is nothing for OPERATIONS.md to
document or for CHATOPS.md's `send it` section to cross-link. The OPERATIONS.md
and CHATOPS.md architecture-audit sections are rewritten for `architecture_advisor`
(advisory, recommendation-based, issue-by-default) as part of this change's docs
tasks; that replacement is general documentation, not a `.brightline-ignore`
subsection, so no successor requirement is added here.
