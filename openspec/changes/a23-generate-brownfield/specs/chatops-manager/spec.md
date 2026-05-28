## ADDED Requirements

### Requirement: Inbound listener recognizes the `brownfield` verb AND submits a `BrownfieldAction`
The inbound chatops listener SHALL recognize `@<bot> brownfield <repo-substring> <capability-name> [optional guidance]` as a known verb alongside the existing chat-driven workflow verbs (`propose`, `send it`, `audit`) AND the operator recovery verbs. The listener SHALL parse the verb's arguments per the following grammar:

- `<repo-substring>` — case-insensitive substring-match against configured repos, following the established `match_repo` rule.
- `<capability-name>` — the next whitespace-delimited token; SHALL match the regex `^[a-z][a-z0-9-]*$`.
- Optional guidance — everything after the capability-name token (preserving internal whitespace AND line breaks, trimmed of leading/trailing whitespace, capped at 10,000 characters).

On a unique repo match AND valid slug, the dispatcher SHALL: generate a `request_id`, post a top-level ack message containing `✓ Queued brownfield draft for <repo_url>: capability=<capability-name>. The next polling iteration will run it (~Nm). Follow along in this thread.`, capture the ack message's `ts` as the request's lifecycle `thread_ts`, write a `BrownfieldRequestState` file with `status: Pending`, AND submit a `BrownfieldAction { repo_url, capability_name, guidance: Option<String>, channel, thread_ts, request_id }` over the daemon's control socket.

#### Scenario: Happy-path queueing with thread creation
- **WHEN** an operator posts `@<bot> brownfield myrepo scheduler` AND `myrepo` uniquely resolves to a configured repo
- **THEN** the bot posts a top-level ack containing `✓ Queued brownfield draft for <repo_url>: capability=scheduler. The next polling iteration will run it (~Nm). Follow along in this thread.`
- **AND** the ack's `ts` becomes the request's `thread_ts`
- **AND** a `BrownfieldRequestState` file is written with `status: Pending` AND `guidance: None`
- **AND** the per-repo `pending_brownfield_requests` queue gains an entry

#### Scenario: Happy-path with operator guidance
- **WHEN** an operator posts `@<bot> brownfield myrepo scheduler focus on the cron-trigger lifecycle; skip telemetry hooks`
- **THEN** the ack message names `capability=scheduler` (the guidance is NOT echoed in the ack to keep the ack short)
- **AND** the `BrownfieldRequestState.guidance` field stores `focus on the cron-trigger lifecycle; skip telemetry hooks`
- **AND** the polling iteration passes the guidance verbatim to the brownfield-draft prompt

#### Scenario: Missing capability name is rejected
- **WHEN** an operator posts `@<bot> brownfield myrepo`
- **THEN** the bot replies `✗ brownfield: missing capability name. Usage: @<bot> brownfield <repo> <capability-name> [optional guidance]`
- **AND** no state file is written
- **AND** no control-socket action is submitted

#### Scenario: Invalid capability slug is rejected
- **WHEN** an operator posts `@<bot> brownfield myrepo BadName_Slug`
- **THEN** the bot replies `✗ brownfield: capability name must match ^[a-z][a-z0-9-]*$ (got: BadName_Slug)`
- **AND** no state file is written

#### Scenario: Repo substring ambiguity surfaces the candidate list
- **WHEN** the repo-substring matches multiple configured repos
- **THEN** the bot replies with the existing `match_repo`-style "be more specific" list
- **AND** no state file is written

#### Scenario: Pre-existing canonical spec is rejected at dispatch time
- **WHEN** an operator posts `@<bot> brownfield myrepo scheduler` AND `openspec/specs/scheduler/spec.md` already exists in `myrepo`'s workspace HEAD
- **THEN** the bot replies `✗ brownfield: openspec/specs/scheduler/spec.md already exists. Use @<bot> propose ... for changes to an existing capability.`
- **AND** no state file is written

#### Scenario: Verb disabled per workspace
- **WHEN** the resolved repo's config has `features.brownfield.enabled: false`
- **THEN** the bot replies `✗ brownfield: disabled in this workspace's config (features.brownfield.enabled=false).`
- **AND** no state file is written

### Requirement: `brownfield` ack message creates the lifecycle thread for subsequent updates
The bot's ack for a brownfield request SHALL be a top-level channel message (NOT a thread reply) so that the ack's `ts` can serve as the lifecycle thread for: subsequent status updates posted by the polling iteration, the eventual `✅ Brownfield draft PR opened: <pr_url>` notification, AND any `@<bot> revise ...` discussion the operator initiates on the resulting PR.

#### Scenario: Lifecycle thread carries status updates
- **WHEN** the polling iteration begins processing a brownfield request
- **THEN** the iteration's status updates (`▶️ Starting brownfield draft`, `✅ Brownfield draft PR opened`, etc.) post as threaded replies under the ack's `ts`

#### Scenario: Lifecycle thread persists across iterations
- **WHEN** a brownfield request remains pending across multiple polling iterations
- **THEN** all related notifications continue to thread under the original ack
- **AND** the operator sees a single conversation per brownfield request
