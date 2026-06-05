# code-reviewer — delta for a005-aggregate-reviewer-revisions

## ADDED Requirements

### Requirement: `auto_revise` is a tri-state defaulting to block
The reviewer's `auto_revise` config SHALL accept three values — `block`, `actionable`, `off` — and default to `block`. It governs when a review's actionable concerns are forwarded (aggregated, per the orchestrator-cli `Reviewer-initiated revisions from one review dispatch as a single run` requirement) to the revision dispatcher:

- **`block`** (default): auto-revise fires only when the review's effective verdict is `Block`. Combined with the `Security-critical findings yield a Block verdict` requirement, security-critical findings still auto-fix (they Block), while non-`Block` `Concerns` are advisory — surfaced to the operator, not silently rewritten.
- **`actionable`**: auto-revise fires on any actionable concern regardless of verdict (the prior fire-regardless-of-verdict behavior).
- **`off`**: no auto-revise.

For backward compatibility the legacy boolean SHALL map: `true` → `actionable`, `false` → `off`. The change in default behavior is from the prior `false`/off to `block` (auto-revise now fires on a `Block` verdict by default).

#### Scenario: Default block does not revise on a Concerns verdict
- **WHEN** `auto_revise` is unset (default `block`) AND a review returns `Concerns` with actionable non-security concerns
- **THEN** no auto-revision is dispatched
- **AND** the concerns are surfaced for the operator to act on with `@<bot> revise`

#### Scenario: Default block revises on a Block verdict
- **WHEN** `auto_revise` is `block` AND a review's effective verdict is `Block`
- **THEN** the review's actionable concerns are forwarded as one aggregated revision run

#### Scenario: actionable restores fire-regardless-of-verdict
- **WHEN** `auto_revise: actionable` AND a review returns `Concerns` with actionable concerns
- **THEN** the concerns are forwarded as one aggregated revision run (regardless of the non-`Block` verdict)

#### Scenario: Legacy boolean maps
- **WHEN** `auto_revise: true` is configured
- **THEN** it is treated as `actionable`
- **AND** `auto_revise: false` is treated as `off`
