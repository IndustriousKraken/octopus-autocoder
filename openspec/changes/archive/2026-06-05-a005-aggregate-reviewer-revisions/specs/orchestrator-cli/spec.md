# orchestrator-cli — delta for a005-aggregate-reviewer-revisions

## ADDED Requirements

### Requirement: Reviewer-initiated revisions from one review dispatch as a single run
All `<!-- reviewer-revision -->` requests produced by a single review SHALL be collected and dispatched as ONE revision run — one executor invocation carrying every concern from that review together — rather than one run per request. The aggregated run SHALL count as exactly ONE increment against the auto-revision cap (`executor.max_auto_revisions_per_pr`), AND SHALL post one operator-visible summary of the concerns it is addressing.

This replaces the per-comment loop (`revisions.rs`, `for comment in comments`) for reviewer-initiated revisions: instead of an executor run + cap increment per `<!-- reviewer-revision -->` comment, the dispatcher groups a review's reviewer-revision comments and issues a single revision. The aggregated run sees all concerns in one warm pass, so related fixes are made together AND a concern already satisfied by an earlier fix in the same batch does not become a separate no-op run.

Human `@<bot> revise <text>` comments are unaffected — each is an explicit operator request and is dispatched as the operator wrote it (subject to the existing human-revise cap).

#### Scenario: A multi-concern review dispatches one revision run
- **WHEN** a single review produces N `<!-- reviewer-revision -->` requests (N ≥ 2)
- **THEN** the dispatcher issues exactly ONE revision run carrying all N concerns
- **AND** the auto-revision cap is incremented by exactly one
- **AND** one summary of the addressed concerns is posted

#### Scenario: Duplicate concerns in one review are fixed once
- **WHEN** a review's requests include two that target the same code (e.g. the same function refactor worded twice)
- **THEN** the single aggregated run addresses them together
- **AND** there is no second run that evaluates to "no change made"

#### Scenario: Human revise comments are still per-request
- **WHEN** an operator posts `@<bot> revise <text>` on the PR
- **THEN** it is dispatched as its own revision (not aggregated with reviewer-initiated requests)
- **AND** it is bounded by the human-revise cap, not the auto-revision cap
