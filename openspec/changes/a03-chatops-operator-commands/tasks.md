## 1. Command parser

- [ ] 1.1 Create `autocoder/src/chatops/operator_commands.rs`. Public surface: `pub fn parse_command(message: &str, bot_mention: &str) -> Option<OperatorCommand>`. Returns `None` for any message that doesn't start with the bot's mention OR doesn't match a known verb (silent ignore — operators shouldn't get error spam from typos in normal chat).
- [ ] 1.2 `OperatorCommand` enum:
  ```rust
  pub enum OperatorCommand {
      Status { repo_substring: String },
      ClearPermaStuck { repo_substring: String, change: String },
      ClearRevision { repo_substring: String, change: String },
      WipeWorkspace { repo_substring: String },
      WipeWorkspaceConfirm { repo_substring: String },
  }
  ```
  The `WipeWorkspaceConfirm` variant is for the second-step `confirm` reply within the 60-second window.
- [ ] 1.3 Parser tests: every verb with happy-path input; verbs with missing required args (returns `None`); messages that don't mention the bot; messages that mention the bot but use an unknown verb; case-insensitivity for the verb (`Status`, `STATUS`, `status` all work); whitespace tolerance.

## 2. Repo-substring matcher

- [ ] 2.1 `pub fn match_repo<'a>(substring: &str, configured: &'a [RepositoryConfig]) -> RepoMatch<'a>` returning one of:
  - `RepoMatch::Unique(&RepositoryConfig)` — exactly one match
  - `RepoMatch::Multiple(Vec<&RepositoryConfig>)` — multiple matches; caller formats a "be more specific" reply with the URLs
  - `RepoMatch::None` — no match; caller formats a "no repo matched; configured: ..." reply
- [ ] 2.2 Case-insensitive substring against `repository.url`. The match is liberal: `coterie` matches `git@github.com:IndustriousKraken/coterie.git`. If two repos with the same name exist under different owners, the operator must type more characters to disambiguate.
- [ ] 2.3 Tests: unique, multiple, none, case-insensitivity, empty substring (returns Multiple with all repos so the operator gets feedback rather than silent everything-match).

## 3. Control-socket action handlers

- [ ] 3.1 Extend `autocoder/src/control_socket.rs`'s action enum with new variants:
  - `RepoStatus { url: String }` — returns a `RepoStatusResponse` struct
  - `ClearPermaStuckMarker { url: String, change: String }` — returns `Ok` or `Err` with a message
  - `ClearRevisionMarker { url: String, change: String }` — same shape
  - `WipeWorkspace { url: String }` — same shape
- [ ] 3.2 Handler implementations:
  - `RepoStatus`: read failure-state.json, alert-state.json, scan change dirs for markers, query the in-memory polling-task map for last-iteration metadata. Assemble into the `RepoStatusResponse` struct.
  - `ClearPermaStuckMarker`: `queue::remove_perma_stuck_marker(workspace, change)`. Returns Err if the marker doesn't exist with a clear "no perma-stuck marker for change `<change>`" message.
  - `ClearRevisionMarker`: analogous via `queue::remove_revision_marker`.
  - `WipeWorkspace`: `std::fs::remove_dir_all(workspace_path)`. Returns the path that was removed in the success message. Idempotent — Ok if the directory was already absent (chatops won't surface that as an error; the user wanted it gone, it's gone).
- [ ] 3.3 The control socket's authn (Unix-socket-perms, daemon-user-only) covers these new actions identically. No new authn logic.
- [ ] 3.4 Tests: each action against a fixture workspace; happy path + error paths for marker-not-found / repo-not-found.

## 4. Wipe-workspace two-step confirmation

- [ ] 4.1 The chatops listener tracks pending confirmations in an in-memory map: `HashMap<ChannelId, PendingConfirmation>` where `PendingConfirmation { repo_url: String, expires_at: Instant }`.
- [ ] 4.2 First-step handling: parser returns `OperatorCommand::WipeWorkspace { repo_substring }`. The listener:
  - Resolves the repo via `match_repo`. On Multiple/None: post the error reply, return without storing a pending confirmation.
  - On Unique: store `PendingConfirmation { repo_url, expires_at: now + 60s }` keyed by the channel id where the message arrived.
  - Post the reply: `⚠️ This will delete /tmp/workspaces/<sanitized-url> (forces a re-clone on the next iteration). Reply 'confirm' within 60 seconds.`
- [ ] 4.3 Second-step handling: parser returns `OperatorCommand::WipeWorkspaceConfirm { ... }` OR a plain `confirm` message in a channel that has a pending confirmation. The listener:
  - Look up the pending confirmation by channel id. If absent OR expired (`now > expires_at`): post `✗ no pending wipe-workspace confirmation in this channel (or it expired — re-issue the original command)`.
  - If present + unexpired: submit `WipeWorkspace { url }` to the control socket. Remove the pending entry. Post the result.
- [ ] 4.4 Tests for the confirmation flow:
  - Happy path: wipe → confirm within 60s → wiped
  - Expired confirmation: wipe → wait > 60s → confirm → error reply, no wipe
  - Crossed wires: wipe in channel A, confirm in channel B → confirm has no pending in B, no wipe in A
  - Re-issue: wipe → wipe (replaces the prior pending) → confirm → wipes the second-named repo

## 5. Chatops listener integration

- [ ] 5.1 Each chatops backend (`slack.rs`, `discord.rs`, `teams.rs`, `mattermost.rs`, `matrix.rs`) has an incoming-message hook. Extend it to call `operator_commands::parse_command` BEFORE the existing AskUser-reply detection. If the parse returns Some, route to the operator-command handler instead of the AskUser handler.
- [ ] 5.2 The per-backend "bot mention" detection differs (Slack's `<@U123>`, Discord's `<@!123>`, etc.). Centralize this by passing the resolved bot-mention string into `parse_command` as a parameter; per-backend code is just "detect that the message is addressed to us and call the parser."
- [ ] 5.3 The operator-command handler does: resolve repo via `match_repo`, submit to control socket, format reply. The reply goes back to the same channel via the backend's existing `post_notification`.

## 6. Reply formatting

- [ ] 6.1 Status reply: format the `RepoStatusResponse` per the shape in the proposal — sectioned by markers / throttled alerts / last iteration / queue snapshot. Empty sections collapse (e.g. if no markers, omit the "active markers" section entirely; don't print "active markers: (none)").
- [ ] 6.2 Action confirmations: one line, `✓ <one-line summary>`. Action errors: one line, `✗ <one-line summary>`. Examples:
  - `✓ cleared .perma-stuck.json for a06-foo on coterie`
  - `✗ no perma-stuck marker for change a99-nonexistent on coterie`
  - `✗ no repo matched 'gibberish'; configured: coterie, sound-cabinet`

## 7. README documentation

- [ ] 7.1 Add a "ChatOps operator commands" subsection in the existing "ChatOps Escalation" section. Document the four verbs, the syntax, the repo-substring rule, and the confirmation flow for wipe-workspace.
- [ ] 7.2 Cross-reference: the section explicitly notes the existing perma-stuck and needs-spec-revision marker patterns and shows how the chatops verbs are the in-chat equivalent of the SSH-and-rm-the-file workflow.

## 8. Spec delta

- [ ] 8.1 Add the ADDED requirement "Chatops operator commands" to `orchestrator-cli` enumerating the four verbs, the syntax contract, the repo-substring matching rule, the wipe-workspace confirmation flow, and the silent-ignore for unknown verbs.

## 9. Verification

- [ ] 9.1 `cargo test` passes.
- [ ] 9.2 `openspec validate chatops-operator-commands --strict` passes.
