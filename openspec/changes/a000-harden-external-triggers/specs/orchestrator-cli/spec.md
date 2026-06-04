# orchestrator-cli — delta for a000-harden-external-triggers

## ADDED Requirements

### Requirement: GitHub comment-sourced verbs require an authorized commenter
Before dispatching ANY verb parsed from a GitHub pull-request or issue comment — including `@<bot> revise` AND `@<bot> code-review`, AND any future comment-sourced verb — the daemon SHALL authorize the commenter. This gate is a precondition on the dispatch of every such verb; a verb whose comment fails authorization SHALL NOT reach its verb-specific handling (the `revise` and `code-review` requirements describe what happens *after* a comment is authorized).

Authorization SHALL pass when EITHER:
- (a) the comment's GitHub `author_association` is in the configured `github.command_authorization.allowed_associations` set — default `[OWNER, MEMBER, COLLABORATOR]`, which are exactly the associations carrying write/triage permission on the repository; OR
- (b) the comment author's `login` is in the configured `github.command_authorization.allowed_users` list (for trusted individuals who are not formal collaborators).

A comment that parses as a verb but whose author is NOT authorized SHALL be **dropped before dispatch** (default-deny): no executor, reviewer, or other billed/LLM work is invoked, the seen-marker IS advanced past the comment so it does not re-fire on subsequent polling cycles, AND the drop is logged at INFO with the author `login` AND `author_association`. When `github.command_authorization.decline_comment` is `true`, the daemon SHALL post exactly one decline reply per dropped trigger; when it is `false` (the default), the daemon SHALL NOT reply (avoiding comment spam AND reply/feedback loops).

`author_association` values (`OWNER`, `MEMBER`, `COLLABORATOR`, `CONTRIBUTOR`, `FIRST_TIME_CONTRIBUTOR`, `FIRST_TIMER`, `NONE`) come from the GitHub comments API. An absent OR unrecognized association is treated as unauthorized. The bot's own comments are filtered before this check (existing behavior, unchanged). This gate is the GitHub analog of the Slack channel allowlist (`Drop-before-dispatch inbound filters`); the Slack path is unaffected by this requirement.

#### Scenario: Authorized association dispatches the verb
- **WHEN** an open PR has a comment with `author_association: COLLABORATOR` whose body parses as `@<bot> revise <text>` (OR `@<bot> code-review`)
- **THEN** the commenter is authorized AND the verb proceeds to its verb-specific handling

#### Scenario: Unauthorized association is dropped before any work
- **WHEN** an open PR has a comment with `author_association: NONE` whose body parses as `@<bot> revise <text>` (OR `@<bot> code-review`)
- **THEN** no executor or reviewer run is invoked
- **AND** the seen-marker is advanced so the comment does not re-fire
- **AND** the drop is logged with the author `login` AND association

#### Scenario: Configured allowlisted user is authorized regardless of association
- **WHEN** a comment author's `login` is listed in `github.command_authorization.allowed_users` AND the comment parses as a verb
- **THEN** the commenter is authorized even when their `author_association` is `NONE` or `CONTRIBUTOR`

#### Scenario: Missing or unknown association is default-denied
- **WHEN** a verb-parsing comment has no `author_association` OR an unrecognized value, AND the author is not in `allowed_users`
- **THEN** the commenter is unauthorized AND the trigger is dropped before dispatch

#### Scenario: Decline reply is posted only when configured
- **WHEN** an unauthorized verb-parsing comment is dropped AND `github.command_authorization.decline_comment: true`
- **THEN** the daemon posts exactly one decline reply for that comment
- **AND** when `decline_comment: false` (default), the daemon posts no reply

#### Scenario: Slack verbs are unaffected
- **WHEN** an operator posts a verb in an allowlisted Slack channel
- **THEN** dispatch proceeds under the Slack channel-allowlist filters with no `author_association` check (this requirement governs GitHub comment-sourced verbs only)

### Requirement: Human-initiated PR revisions are rate-capped per PR
The daemon SHALL bound the number of human-initiated `@<bot> revise` triggers it acts on per pull request, to cap cost AND abuse independent of requester. The per-PR limit SHALL read from `executor.max_revise_triggers_per_pr` (default `10`). The count is tracked in the existing per-PR state file. When the cap is reached, a further `@<bot> revise` trigger on that PR SHALL be declined with exactly one notice AND SHALL NOT invoke the executor.

This cap is independent of the existing auto-revision cap (`executor.max_auto_revisions_per_pr`, which bounds reviewer-initiated revisions) AND the re-review cap (`reviewer.max_code_reviews_per_pr`). It applies only to revisions triggered by an authorized human comment (per `GitHub comment-sourced verbs require an authorized commenter`).

#### Scenario: Revision under the cap proceeds
- **WHEN** an authorized `@<bot> revise` trigger arrives AND the PR's recorded human-revise count is below `executor.max_revise_triggers_per_pr`
- **THEN** the executor is invoked for the revision
- **AND** the PR's human-revise count increments by one

#### Scenario: Revision at the cap is declined without invoking the executor
- **WHEN** an authorized `@<bot> revise` trigger arrives AND the PR's recorded human-revise count has reached `executor.max_revise_triggers_per_pr`
- **THEN** the executor is NOT invoked
- **AND** the daemon posts exactly one notice that the per-PR revise cap is reached
- **AND** the count does not increment further

#### Scenario: Human and auto revision caps are independent
- **WHEN** the auto-revision cap (`executor.max_auto_revisions_per_pr`) is exhausted on a PR
- **THEN** an authorized human `@<bot> revise` still proceeds while the human cap (`executor.max_revise_triggers_per_pr`) has headroom
- **AND** exhausting the human cap does not change the auto-revision count
