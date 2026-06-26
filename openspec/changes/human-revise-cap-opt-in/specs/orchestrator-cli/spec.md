## RENAMED Requirements

- FROM: `Human-initiated PR revisions are rate-capped per PR`
- TO: `Human-initiated PR revisions are optionally rate-capped per PR`

## MODIFIED Requirements

### Requirement: Human-initiated PR revisions are optionally rate-capped per PR
The daemon SHALL support an OPTIONAL per-PR cap on the number of human-initiated `@<bot> revise` triggers it acts on, OFF by default. The per-PR limit reads from `executor.max_revise_triggers_per_pr`, which SHALL be an OPTIONAL value (`Option<u32>`) defaulting to NONE (unlimited) — mirroring the opt-in re-review cap (`reviewer.max_code_reviews_per_pr`). When it is NONE (the default), an authorized human `@<bot> revise` trigger SHALL ALWAYS be acted on: it is never counted against a cap AND never declined for cap reasons, preserving the invariant that an operator's deliberate revision always processes. When it is set to a positive `N`, the daemon SHALL bound the count at `N`: the count is tracked in the existing per-PR state file, AND a further `@<bot> revise` trigger once the count has reached `N` SHALL be declined with exactly one notice AND SHALL NOT invoke the executor.

This cap is independent of the auto-revision cap (`executor.max_auto_revisions_per_pr`, which bounds reviewer-initiated revisions) AND the re-review cap (`reviewer.max_code_reviews_per_pr`). It applies only to revisions triggered by an authorized human comment (per `GitHub comment-sourced verbs require an authorized commenter`).

#### Scenario: Default is unlimited so human revises always process
- **WHEN** `executor.max_revise_triggers_per_pr` is unset (the default `None`) AND authorized `@<bot> revise` triggers arrive on a PR
- **THEN** every such trigger invokes the executor
- **AND** none is declined for cap reasons, regardless of how many have already been made on that PR

#### Scenario: Revision under the cap proceeds
- **WHEN** `executor.max_revise_triggers_per_pr` is set to a positive `N` AND an authorized `@<bot> revise` trigger arrives AND the PR's recorded human-revise count is below `N`
- **THEN** the executor is invoked for the revision
- **AND** the PR's human-revise count increments by one

#### Scenario: Revision at the cap is declined without invoking the executor
- **WHEN** `executor.max_revise_triggers_per_pr` is set to a positive `N` AND an authorized `@<bot> revise` trigger arrives AND the PR's recorded human-revise count has reached `N`
- **THEN** the executor is NOT invoked
- **AND** the daemon posts exactly one notice that the per-PR revise cap is reached
- **AND** the count does not increment further

#### Scenario: Human and auto revision caps are independent
- **WHEN** a human cap is configured AND the auto-revision cap (`executor.max_auto_revisions_per_pr`) is exhausted on a PR
- **THEN** an authorized human `@<bot> revise` still proceeds while the human cap (`executor.max_revise_triggers_per_pr`) has headroom
- **AND** exhausting the human cap does not change the auto-revision count
