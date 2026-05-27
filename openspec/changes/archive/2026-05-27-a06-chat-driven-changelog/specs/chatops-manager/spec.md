## ADDED Requirements

### Requirement: Inbound listener recognizes the `changelog` verb and submits a `ChangelogAction`
The Slack Socket Mode inbound listener SHALL recognize `@<bot> changelog <repo-substring> [<args>]` as a known verb alongside the existing chat-driven workflow verbs (`propose`, `send it`, `audit`) AND the operator recovery verbs (`status`, `clear-perma-stuck`, `clear-revision`, `wipe-workspace`, `rebuild-specs`, `help`). The listener SHALL parse the verb, resolve the repo substring via the existing case-insensitive substring-match rule, AND submit a `ChangelogAction { repo_url, raw_args, channel, thread_ts }` over the daemon's Unix-domain control socket. The listener SHALL post the bot's ack as a top-level channel message (NOT a thread reply) so that the ack's `ts` can serve as the lifecycle thread for subsequent status updates AND `@<bot> revise ...` discussion.

#### Scenario: Valid verb dispatches a ChangelogAction with the resolved repo URL
- **WHEN** the listener receives `@<bot> changelog coterie --since v0.1.0`
- **AND** the substring `coterie` unambiguously resolves to a configured repository
- **THEN** the listener submits a `ChangelogAction` over the control socket with `repo_url = <resolved URL>`, `raw_args = "--since v0.1.0"`, `channel = <originating channel>`, AND `thread_ts = <bot ack message ts>`
- **AND** the listener posts `✓ Queued changelog request for <repo-url>. The next polling iteration will run it. Follow along in this thread.` as a top-level channel message
- **AND** the resulting message's `ts` is the value passed in `thread_ts`

#### Scenario: Ambiguous repo substring lists candidates
- **WHEN** the listener receives `@<bot> changelog my-repo` AND `my-repo` matches multiple configured URLs
- **THEN** the listener does NOT submit a `ChangelogAction`
- **AND** posts the standard "be more specific" reply with each candidate URL listed
- **AND** no state file is written

#### Scenario: Verb without a repo substring is refused
- **WHEN** the listener receives `@<bot> changelog` (no arguments)
- **THEN** the listener posts `✗ changelog: missing repo-substring.` as a threaded reply
- **AND** no `ChangelogAction` is submitted

#### Scenario: Help verb lists the changelog verb
- **WHEN** an operator runs `@<bot> help`
- **THEN** the help text lists `changelog` alongside the other chat-driven workflow verbs
- **AND** the one-line description names the verb's purpose (`generate an LLM-styled CHANGELOG.md update via PR`)

#### Scenario: Verb participates in dedup
- **WHEN** Slack redelivers the same `@<bot> changelog ...` event (per the Socket Mode at-least-once contract)
- **THEN** the existing event-dedup cache (from `chatops-slack-event-dedup`) suppresses the second delivery
- **AND** exactly one `ChangelogAction` is submitted regardless of redelivery count
- **AND** exactly one ack message is posted to the channel
