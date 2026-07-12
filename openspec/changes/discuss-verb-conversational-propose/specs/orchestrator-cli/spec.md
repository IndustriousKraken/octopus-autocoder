## REMOVED Requirements

### Requirement: Triage prompt classifies the request as DIRECTIVE, QUESTION, or AMBIGUOUS before acting
### Requirement: `.chat-reply.md` marker drives the discussion-reply path
### Requirement: Directive triage uses the existing two-PR mechanic; PRs participate in the revision-loop
### Requirement: Proposal-request state files are pruned after 7 days

## MODIFIED Requirements

### Requirement: `propose` chatops verb is a permanent alias for `discuss`
The verb `propose` is a permanent alias for `discuss`. The inbound chatops listener SHALL accept `@<bot> propose <repo-substring> <free-form text>` and process it identically to `@<bot> discuss <repo-substring> <free-form text>` per the `discuss chatops verb starts an immediate conversational session` requirement. No deprecation warning, distinguishing log entry, or behavioral difference applies to the alias. `propose` remains a listed known verb in all help text and verb-recognition requirements.

#### Scenario: `propose` alias triggers discuss behavior
- **WHEN** an operator posts `@<bot> propose myrepo add a /healthz endpoint`
- **THEN** the behavior is identical to `@<bot> discuss myrepo add a /healthz endpoint`
- **AND** the ack message, DiscussionState file, and handler path are the same as for `discuss`
- **AND** no deprecation note appears in the ack

#### Scenario: Missing request text is rejected (alias path)
- **WHEN** an operator posts `@<bot> propose myrepo` (no free-form text)
- **THEN** the bot replies `✗ discuss: missing request text. Usage: @<bot> discuss <repo> <question or request>`
- **AND** no state file is written

## ADDED Requirements

### Requirement: `discuss` chatops verb starts an immediate conversational session
The chatops listener SHALL recognize `@<bot> discuss <repo-substring> <free-form text>` as the `DiscussAction` command. The repo-substring follows the established case-insensitive substring-matching rules. The free-form text is everything after the substring (trimmed, line breaks preserved, capped at 10,000 characters).

On a unique repo match, the dispatcher SHALL: generate a `request_id`, post a top-level channel message as the ack (the ack's `ts` becomes the session's `thread_ts`), write a `DiscussionState` file with `status: Active`, AND submit a `DiscussAction { repo_url, initial_text, channel, thread_ts, request_id, operator_user }` over the control socket.

The ack message SHALL contain the phrase "Follow along in this thread." AND the phrase "Note: only replies starting with @<bot> are seen here."

The daemon's dedicated discuss handler processes `DiscussAction` items as they arrive on the control socket, without waiting for the next repo polling iteration. The target elapsed time from control-socket receipt to first thread reply is under 60 seconds under normal load.

#### Scenario: Happy-path — message received and ack posted immediately
- **WHEN** an operator posts `@<bot> discuss myrepo how does the revision executor decide when to stop retrying?`
- **AND** `myrepo` uniquely resolves to a configured repo
- **THEN** the bot posts a top-level ack containing "Follow along in this thread." AND "Note: only replies starting with @<bot> are seen here."
- **AND** a `DiscussionState` file is written with `status: Active`
- **AND** the daemon's discuss handler begins processing within 60 seconds

#### Scenario: Missing request text is rejected
- **WHEN** an operator posts `@<bot> discuss myrepo` (no free-form text)
- **THEN** the bot replies `✗ discuss: missing request text. Usage: @<bot> discuss <repo> <question or request>`
- **AND** no state file is written

#### Scenario: Ambiguous repo substring surfaces the candidate list
- **WHEN** the repo-substring matches multiple configured repos
- **THEN** the bot replies with the existing `match_repo`-style candidate list
- **AND** no state file is written

### Requirement: Discuss handler processes requests without waiting for the polling loop
The daemon SHALL maintain a dedicated discuss handler as a persistent async task, separate from the per-repo polling loop. The handler listens for `DiscussAction` and `DiscussContinueAction` control-socket submissions and processes them immediately upon receipt. The handler MUST NOT wait for a polling iteration's sleep timer.

The discuss agent runs in a read-only sandbox (no file writes, no git commits) during the conversational phase. Session state — including a session ID usable for context resumption — is persisted to `DiscussionState` after each agent turn so continuation messages and the eventual `send it` can resume the conversation.

The discuss-mode agent prompt SHALL instruct the agent to proactively read:
- Canonical specs in `openspec/specs/*/spec.md` relevant to the topic.
- `CHANGELOG.md`, `OCTOPUS.md`, and any `docs/*.md` present in the workspace.
- Active and recently archived changes in `openspec/changes/`.
- Source files an implementer would need to modify to carry out the discussed change.

The agent SHALL reply conversationally in the thread — answering questions directly, describing proposed changes in plain terms, and waiting for operator input rather than acting unilaterally.

#### Scenario: Agent replies within 60 seconds
- **WHEN** a `DiscussAction` is submitted to the control socket
- **THEN** the discuss handler begins the agentic session without waiting for any polling sleep
- **AND** the first agent reply appears in the thread within 60 seconds under normal load

#### Scenario: Agent reads relevant specs before responding
- **WHEN** the operator's question references a daemon feature
- **THEN** the agent reads the relevant canonical spec AND any active change touching that feature before replying
- **AND** the reply reflects current canon, not a generic answer

### Requirement: `@<bot>` replies in a discuss thread continue the conversation
When an operator posts `@<bot> <text>` as a reply in an active discuss thread (any thread whose `thread_ts` matches an active `DiscussionState`), AND the text is NOT the `send it` verb (case-insensitive match on the leading `send it` token after stripping the `@<bot>` prefix), the listener SHALL submit a `DiscussContinueAction { repo_url, text, channel, thread_ts, request_id }`. The discuss handler resumes the session for that thread with the new text appended to context and posts a reply in the thread. `@<bot> send it` replies are handled exclusively by the `send it in a discuss thread creates an artifact sequentially` requirement and SHALL NOT also produce a `DiscussContinueAction`.

Replies NOT prefixed with `@<bot>` in a discuss thread are silently ignored. A `DiscussContinueAction` submitted to a `thread_ts` that no longer has an active `DiscussionState` SHALL be refused with a thread reply: `✗ This discussion is no longer active. Start a new one with @<bot> discuss <repo> <text>.`

#### Scenario: Operator follow-up continues in thread
- **WHEN** an operator posts `@<bot> what about the revision-cap edge case?` in an active discuss thread
- **THEN** a `DiscussContinueAction` is submitted
- **AND** the discuss handler resumes the session and replies in the thread

#### Scenario: Reply without @<bot> prefix is silently ignored
- **WHEN** an operator posts `thanks, that makes sense` (no @<bot> prefix) in a discuss thread
- **THEN** no action is submitted and no reply is posted

### Requirement: `send it` in a discuss thread creates an artifact sequentially
When an operator posts `@<bot> send it [optional trailing text]` in an active discuss thread, the listener SHALL submit a `DiscussSendItAction { repo_url, final_context: Option<String>, channel, thread_ts, request_id }`. Any text following `send it` (trimmed) becomes `final_context` and is appended to the session context before the artifact-creation step. The `DiscussionState` status transitions to `Executing`.

The discuss handler queues the artifact-creation job so that it runs AFTER the current executor for that repo completes (if one is in flight). If no executor is in progress the job starts immediately. In the artifact-creation step the session resumes in write mode: the agent may create and modify files, commits the result, and the daemon opens a PR on the configured `agent-q` branch. The bot posts the PR URL as a thread reply.

#### Scenario: `send it` with final context
- **WHEN** an operator posts `@<bot> send it and let's go with Option B, keep the existing error format`
- **THEN** the `DiscussSendItAction.final_context` is `and let's go with Option B, keep the existing error format`
- **AND** the agent appends this context before writing the artifact

#### Scenario: Artifact creation waits for current executor
- **WHEN** a `DiscussSendItAction` arrives while the repo's implementation executor is in flight
- **THEN** artifact creation does not start until the running executor finishes
- **AND** no merge conflict is introduced on the `agent-q` branch

#### Scenario: PR opened and URL posted to thread
- **WHEN** the artifact-creation step completes and the daemon opens a PR on `agent-q`
- **THEN** the bot posts the PR URL as a reply in the discuss thread
- **AND** `DiscussionState.status` transitions to `Completed`

### Requirement: Auto-defer protects an existing spec under active discuss
When the discuss agent determines during the conversational phase that the operator is discussing a modification to an existing canonical spec or an active change's spec delta, the handler SHALL write the defer marker for the relevant change or spec entry, commit it to the workspace, and post a thread reply naming the deferred unit AND stating the exact command to clear it: `"I've deferred <slug> while we discuss. If you decide not to follow through, clear it with @<bot> undefer <repo> <slug>. I'll clear it automatically when a PR lands."`

On `send it` completion (PR opened), the handler SHALL clear the defer marker and commit/push the removal. If no `send it` arrives within 7 days of the last thread activity, the handler SHALL post a single idle reminder in the thread naming the deferred unit and restating the undefer command. The reminder fires once per stale discussion.

#### Scenario: Auto-defer on existing-spec modification discussion
- **WHEN** the discuss agent determines the operator is discussing a change to an existing spec
- **THEN** the defer marker is written and committed
- **AND** the thread reply names the spec and the `@<bot> undefer` command

#### Scenario: Defer cleared on PR open
- **WHEN** `send it` completes and a PR is opened
- **THEN** the defer marker is removed and the removal is committed/pushed

#### Scenario: 7-day idle reminder fires once
- **WHEN** no thread activity in 7 days AND a defer marker is active
- **THEN** one reminder is posted in the thread
- **AND** no further reminders fire until activity resumes

### Requirement: DiscussionState files are pruned after 14 days
The daemon SHALL prune `DiscussionState` files whose `last_activity_at` is older than 14 days. The prune runs at iteration start or once per day per the existing housekeeping pattern. Stale entries are removed regardless of `status`.

#### Scenario: Stale DiscussionState is removed
- **WHEN** the prune runs AND a `DiscussionState` has `last_activity_at` more than 14 days in the past
- **THEN** the state file is removed

#### Scenario: Active discussion is preserved
- **WHEN** the prune runs AND a `DiscussionState` has `last_activity_at` within the last 14 days
- **THEN** the state file is NOT removed
