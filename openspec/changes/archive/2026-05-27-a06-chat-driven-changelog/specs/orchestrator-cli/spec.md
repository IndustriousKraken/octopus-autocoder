## ADDED Requirements

### Requirement: `changelog` chatops verb queues an LLM-styled CHANGELOG.md update via the standard triage path
The daemon SHALL accept a `ChangelogAction` over its Unix-domain control socket (submitted by the Slack inbound listener on `@<bot> changelog <repo-substring> [<args>]`). The action SHALL stamp a `ChangelogRequest` state file under `<state_dir>/changelog-requests/<request_id>.json`. On the next polling iteration AFTER the request is queued, the daemon SHALL: (a) run the `a05` deterministic extractor against the workspace's archive AND get the JSON output, (b) invoke the wrapped agent CLI with the embedded `prompts/changelog-stylist.md` system prompt + the JSON data as input, (c) validate the resulting diff's path scope (`CHANGELOG.md` AND optionally `openspec/changes/archive/<slug>/proposal.md` files; reject all others), (d) commit the diff to a `changelog-<short-hash>` branch, push it, AND open a single PR. The PR SHALL participate in the existing PR-comment revision loop without additional plumbing.

The stylist prompt SHALL instruct the agent to check for an existing `CHANGELOG.md` in the workspace root AND match its style if present, OR create a fresh file in the Keep a Changelog v1.1.0 format if absent. The agent SHALL also be permitted to propose `changelog:` frontmatter edits to source proposals when the changelog work surfaces a durable classification decision — but only when the operator's input (initial verb OR revision text) implies such a decision.

`ChangelogRequest` state files SHALL be pruned after 7 days regardless of terminal status (`Acted`, `Failed`, `InFlight`), parallel to the audit-thread and proposal-request pruning schedules.

#### Scenario: Verb queues a request and the next iteration produces a PR
- **WHEN** an operator types `@<bot> changelog coterie` in a watched channel
- **AND** the inbound listener parses the verb AND submits a `ChangelogAction`
- **THEN** the daemon writes a `ChangelogRequest` state file with `status: Pending`
- **AND** the bot posts `✓ Queued changelog request for <repo-url>. The next polling iteration will run it. Follow along in this thread.` as a top-level channel message
- **AND** the ack message's `ts` is stored as the request's `lifecycle_thread_ts`
- **WHEN** the next polling iteration runs
- **THEN** the handler runs the deterministic extractor, invokes the stylist via the executor, captures the diff
- **AND** validates that the diff touches only `CHANGELOG.md` AND/OR `openspec/changes/archive/<slug>/proposal.md` paths
- **AND** commits the diff to a `changelog-<short-hash>` branch
- **AND** opens a single PR
- **AND** posts a threaded reply in the lifecycle thread: `✓ Changelog draft ready at <PR-URL>. Review on GitHub; revise via @<bot> revise <text>.`
- **AND** the request's `status` advances to `Acted`

#### Scenario: Out-of-scope diff is refused
- **WHEN** the LLM's diff touches files outside `CHANGELOG.md` AND `openspec/changes/archive/<slug>/proposal.md`
- **THEN** the handler does NOT commit
- **AND** the handler posts `✗ changelog: LLM produced out-of-scope diff; refusing to commit. See <log-path>.` to the lifecycle thread
- **AND** the request's `status` advances to `Failed`
- **AND** the workspace is left clean (no partial branch, no orphan commit)

#### Scenario: Revision loop iterates the changelog
- **WHEN** an operator posts `@<bot> revise leave out the refactors from this changelog` on the changelog PR
- **THEN** the existing PR-comment revision dispatcher (from `a01-pr-comment-revision-loop`) parses the comment
- **AND** the next polling iteration re-invokes the stylist with the previous draft + the operator's instruction in context
- **AND** the handler validates the new diff's path scope AND force-pushes the updated commit to the `changelog-<short-hash>` branch
- **AND** the PR's diff updates in place; no PR close/re-open

#### Scenario: Revision proposes frontmatter edits when implied
- **WHEN** an operator's revision text implies a durable classification (e.g. `leave out the refactors` OR `internal changes shouldn't appear in the changelog`)
- **THEN** the stylist MAY include `changelog: skip` frontmatter edits to the relevant source proposals in the same diff
- **AND** the operator reviewing the PR sees both the CHANGELOG.md edit AND the proposal.md frontmatter edits in a single diff
- **AND** future invocations of the deterministic extractor honor the frontmatter, so the classification persists across releases

#### Scenario: Fresh-repo CHANGELOG.md creation
- **WHEN** the operator runs `@<bot> changelog <repo>` against a workspace with NO existing `CHANGELOG.md`
- **THEN** the stylist creates `CHANGELOG.md` in the Keep a Changelog v1.1.0 format
- **AND** the file starts with the project name as a top-level heading
- **AND** includes an `## [Unreleased]` placeholder
- **AND** the current release's section appears as `## [<version>] - <YYYY-MM-DD>`
- **AND** the operator reviewing the PR can validate the formatting choice before merging

#### Scenario: Polite refusal — missing repo substring
- **WHEN** an operator types `@<bot> changelog` with no first argument
- **THEN** the listener posts `✗ changelog: missing repo-substring.` as a threaded reply
- **AND** no state file is written
- **AND** the request is idempotent — re-issuing the verb with arguments works as if the first attempt never happened

#### Scenario: Polite refusal — repo substring matches nothing
- **WHEN** the operator's substring does not match any configured repository
- **THEN** the listener posts `✗ changelog: no repo matched '<sub>'; configured: <list>`
- **AND** the candidate list contains every configured `repositories[].url` so the operator can copy-paste a correction

#### Scenario: Polite refusal — chatops backend unconfigured
- **WHEN** the daemon's `OperatorCommandDispatcher` is constructed without a chatops backend
- **AND** an operator's `changelog` verb reaches the parser
- **THEN** the listener responds `✗ changelog: chatops backend not configured.`
- **AND** no state file is written

#### Scenario: Polite refusal — ack post failure
- **WHEN** the ack `post_notification` to the channel fails (HTTP error, scope revoked, channel renamed)
- **THEN** the listener posts the error inline (`✗ changelog: could not post ack to chat: <reason>`) where it can
- **AND** the state file is NOT written
- **AND** the operator can retry the verb after fixing the upstream chatops issue

#### Scenario: 7-day staleness pruning
- **WHEN** a `ChangelogRequest` state file's `submitted_at` is older than 7 days
- **THEN** the next polling iteration's pruning pass removes the file
- **AND** an INFO log line records the pruned `request_id`
- **AND** any PRs spawned from that request continue to work independently (their revision loop is keyed to the PR's branch, not the state file)
