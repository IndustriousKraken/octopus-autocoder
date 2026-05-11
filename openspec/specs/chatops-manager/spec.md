# chatops-manager Specification

## Purpose
TBD - created by archiving change chatops-escalation. Update Purpose after archive.
## Requirements
### Requirement: Post escalation question to Slack
The chatops-manager SHALL post a human-readable question to a configured Slack channel and return the resulting thread timestamp so future polling iterations can find the human's reply.

#### Scenario: Posting a fresh question
- **WHEN** autocoder passes a question string, a change name, and a target channel id to `post_question(...)`
- **THEN** the manager issues an HTTP POST to `https://slack.com/api/chat.postMessage` with header `Authorization: Bearer <token>` (token sourced from the configured environment variable) and a JSON body containing `channel`, `text` formatted to begin with `❓ \`<change>\`:` followed by the question, and `link_names: 1`
- **AND** on a 2xx response with `ok: true`, the manager returns the response's `ts` field as a string
- **AND** on a 2xx response with `ok: false`, the manager returns an error whose text contains the Slack `error` field verbatim
- **AND** on a non-2xx response, the manager returns an error whose text contains the response status code

### Requirement: Identify the bot's own Slack user id
The chatops-manager SHALL learn its own Slack user id at construction time so subsequent reply detection can distinguish bot messages from human replies.

#### Scenario: Successful authentication
- **WHEN** `ChatOps::new(bot_token)` is invoked
- **THEN** the manager issues an HTTP POST to `https://slack.com/api/auth.test` with the configured token
- **AND** on a 2xx response with `ok: true`, the manager caches the response's `user_id` field internally
- **AND** on any other response, the manager returns an error whose text contains the Slack `error` field (or HTTP status if non-2xx)

### Requirement: Poll Slack thread for first non-bot reply
The chatops-manager SHALL fetch replies in the tracked thread and return the earliest message authored by a human, or `None` if no such message is present.

#### Scenario: Thread contains only the bot's posting
- **WHEN** `poll_thread_for_human_reply(channel, thread_ts)` is called AND the only message in the thread is the bot's own posting
- **THEN** the manager returns `None`

#### Scenario: Thread contains a human reply
- **WHEN** the thread contains at least one message whose `bot_id` field is absent AND whose `user` field differs from the cached bot user id
- **THEN** the manager returns `Some(HumanReply { text, user_id, ts })` for the EARLIEST such message
- **AND** the original posting message is never returned even if it appears first in the array

### Requirement: Atomic and idempotent state-file management
The chatops-manager SHALL provide read, write, and delete helpers for the `.question.json` and `.answer.json` files inside change directories. Writes MUST be atomic; deletes MUST be idempotent.

#### Scenario: Writing a question file
- **WHEN** autocoder calls `write_question_file(workspace, change, payload)`
- **THEN** the manager writes a JSON document containing at least `thread_ts`, `channel`, `resume_handle`, and `asked_at` to `<workspace>/openspec/changes/<change>/.question.json`
- **AND** the write is performed via tempfile-then-rename in the same directory so a partially-written file is never observable

#### Scenario: Writing an answer file
- **WHEN** autocoder calls `write_answer_file(workspace, change, payload)`
- **THEN** the manager writes a JSON document containing at least `answer`, `answered_at`, and `answerer_user_id` to `<workspace>/openspec/changes/<change>/.answer.json`
- **AND** the write is atomic by the same mechanism

#### Scenario: Deleting state files is idempotent
- **WHEN** `delete_question_file(workspace, change)` or `delete_answer_file(workspace, change)` is called
- **THEN** the file is removed if it exists
- **AND** no error is returned if the file is already absent

