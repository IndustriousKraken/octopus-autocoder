# project-documentation — delta for a68-decompose-polling-loop

## ADDED Requirements

### Requirement: The orchestrator polling loop is decomposed by responsibility
The polling-loop orchestration SHALL be organized as a directory module whose submodules each own a single responsibility — rather than a single flat file that mixes all of them. Distinct responsibilities such as alert/notification posting, queue walking, waiting-change handling, pre-flight checks, review-context assembly, pull-request construction, rebuild iteration, audit-triage handling, proposal handling, AND outcome handling SHALL each reside in their own submodule. No submodule SHALL exceed the file-size budget (per `Source files and functions stay within a size budget`), AND a function that exceeds the function-size budget SHALL be split along its internal phases rather than left as one oversized body. Unit tests SHALL reside in sibling test module(s), NOT in a `#[cfg(test)] mod tests` block inside the orchestration source.

This requirement gives the project's largest historical bloat hotspot a durable structural contract: the architecture-brightline, drift, AND architecture-consultative audits verify it on every run, so the module cannot silently re-accrete into one file.

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
