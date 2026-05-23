## 1. Command parser

- [x] 1.1 Create `autocoder/src/chatops/operator_commands.rs`. Public surface: `pub fn parse_command(message: &str, bot_mention: &str) -> Option<OperatorCommand>`. Returns `None` for any message that doesn't start with the bot's mention OR doesn't match a known verb (silent ignore — operators shouldn't get error spam from typos in normal chat).
- [x] 1.2 `OperatorCommand` enum:
  ```rust
  pub enum OperatorCommand {
      Status { repo_substring: String },
      ClearPermaStuck { repo_substring: String, change: String },
      ClearRevision { repo_substring: String, change: String },
      WipeWorkspace { repo_substring: String },
      WipeWorkspaceConfirm { repo_substring: String },
  }
  ```
  The `WipeWorkspaceConfirm` variant is for the second-step `confirm` reply within the 60-second window. (Implementation uses `Option<String>` for `repo_substring` so the bare friendly `confirm` form has nothing to fabricate; the channel's pending entry is authoritative.)
- [x] 1.3 Parser tests: every verb with happy-path input; verbs with missing required args (returns `None`); messages that don't mention the bot; messages that mention the bot but use an unknown verb; case-insensitivity for the verb (`Status`, `STATUS`, `status` all work); whitespace tolerance.

## 2. Repo-substring matcher

- [x] 2.1 `pub fn match_repo<'a>(substring: &str, configured: &'a [RepositoryConfig]) -> RepoMatch<'a>` returning one of:
  - `RepoMatch::Unique(&RepositoryConfig)` — exactly one match
  - `RepoMatch::Multiple(Vec<&RepositoryConfig>)` — multiple matches; caller formats a "be more specific" reply with the URLs
  - `RepoMatch::None` — no match; caller formats a "no repo matched; configured: ..." reply
- [x] 2.2 Case-insensitive substring against `repository.url`. The match is liberal: `myrepo` matches `git@github.com:acme/myrepo.git`. If two repos with the same name exist under different owners, the operator must type more characters to disambiguate.
- [x] 2.3 Tests: unique, multiple, none, case-insensitivity, empty substring (returns Multiple with all repos so the operator gets feedback rather than silent everything-match).

## 3. Control-socket action handlers

- [x] 3.1 Extend `autocoder/src/control_socket.rs`'s action enum with new variants:
  - `repo_status` (action string) — returns a `RepoStatusResponse` struct in the `status` field
  - `clear_perma_stuck_marker` — returns `{"ok": true}` on success, `{"ok": false, "error": "..."}` on missing-marker / unknown-repo
  - `clear_revision_marker` — same shape
  - `wipe_workspace` — same shape, with the removed `path` echoed back in the success body
- [x] 3.2 Handler implementations:
  - `repo_status`: reads failure-state.json (for the last-iteration timestamp + reason excerpt), alert-state.json (for both category-level and per-change perma-stuck/spec-revision throttles), scans change dirs for markers via `queue::list_marker_excluded`, computes `next_iteration_estimate` from the failure timestamp + the repo's poll interval. Assembles into the `RepoStatusResponse` struct.
  - `clear_perma_stuck_marker`: `queue::remove_perma_stuck_marker(workspace, change)`. Returns Err with `"no perma-stuck marker for change \`<change>\`"` when absent.
  - `clear_revision_marker`: analogous via `queue::remove_revision_marker`.
  - `wipe_workspace`: `std::fs::remove_dir_all(workspace_path)`. Returns the removed `path` in the success body. Idempotent — Ok with `already_absent: true` if the directory was already absent.
- [x] 3.3 The control socket's authn (Unix-socket-perms, daemon-user-only) covers these new actions identically. No new authn logic.
- [x] 3.4 Tests: each action against a fixture workspace; happy path + error paths for marker-not-found / repo-not-found.

## 4. Wipe-workspace two-step confirmation

- [x] 4.1 The chatops listener tracks pending confirmations in an in-memory map: `HashMap<ChannelId, PendingConfirmation>` where `PendingConfirmation { repo_url: String, expires_at: Instant }`. Wrapped in a `ConfirmationStore` for thread-safe access.
- [x] 4.2 First-step handling: parser returns `OperatorCommand::WipeWorkspace { repo_substring }`. The dispatcher:
  - Resolves the repo via `match_repo`. On Multiple/None: returns the error reply, no pending confirmation stored.
  - On Unique: stores `PendingConfirmation { repo_url, expires_at: now + 60s }` keyed by the channel id where the message arrived.
  - Returns the reply: `⚠️ This will delete <sanitized-workspace-path> (forces a re-clone on the next iteration). Reply 'confirm' within 60 seconds.`
- [x] 4.3 Second-step handling: parser returns `OperatorCommand::WipeWorkspaceConfirm { ... }` (bare `confirm` OR explicit `wipe-workspace-confirm`). The dispatcher:
  - Looks up the pending confirmation by channel id; expired entries are removed at lookup time. If absent OR expired: returns `✗ no pending wipe-workspace confirmation in this channel (or it expired — re-issue the original command)`.
  - If present + unexpired: submits `wipe_workspace { url }` to the control socket. Removes the pending entry. Returns the result.
- [x] 4.4 Tests for the confirmation flow:
  - Happy path: wipe → confirm within 60s → wiped
  - Expired confirmation: wipe → wait > 60s → confirm → error reply, no wipe (test uses a 1ms TTL to avoid sleeping 60s)
  - Crossed wires: wipe in channel A, confirm in channel B → confirm has no pending in B, no wipe in A
  - Re-issue: wipe → wipe (replaces the prior pending) → confirm → wipes the second-named repo

## 5. Chatops listener integration

- [x] 5.1 The central `OperatorCommandDispatcher::handle_message` function is the per-backend integration point: it calls `operator_commands::parse_command` before any reply-as-answer routing and returns `None` for unrecognised messages so existing AskUser-reply detection can fall through. Per-backend wiring to subscribe to channel events (Slack Events API, Discord gateway, Teams webhooks, Mattermost websockets, Matrix sync) is deferred — the current chatops backends only poll question threads and do not subscribe to channel-wide messages. The dispatcher + `ControlSocketSubmitter` + the end-to-end integration test demonstrate the full flow; when channel-event subscription is added per-backend, those backends call `dispatcher.handle_message(text, channel_id, bot_mention, ...)` before their existing reply handler.
- [x] 5.2 The per-backend "bot mention" detection differs (Slack's `<@U123>`, Discord's `<@!123>`, etc.). The dispatcher accepts `bot_mention: &str` as a parameter; per-backend code is just "detect that the message is addressed to us and call the dispatcher."
- [x] 5.3 The dispatcher does: resolve repo via `match_repo`, submit to control socket via an `ActionSubmitter` (production: `ControlSocketSubmitter`; tests: `FakeSubmitter`), format reply. The reply text is the dispatcher's return value; the caller posts it via the backend's existing `post_notification`.

## 6. Reply formatting

- [x] 6.1 Status reply: format the `RepoStatusResponse` per the shape in the proposal — sectioned by markers / throttled alerts / last iteration / queue snapshot. Empty sections collapse (e.g. if no markers, the "active markers" section is omitted entirely; no `(none)` placeholder).
- [x] 6.2 Action confirmations: one line, `✓ <one-line summary>`. Action errors: one line, `✗ <one-line summary>`. Verified examples:
  - `✓ cleared .perma-stuck.json for a06-foo on myrepo`
  - `✗ no perma-stuck marker for change \`a99-nonexistent\``
  - `✗ no repo matched \`gibberish\`; configured: ...`

## 7. README documentation

- [x] 7.1 Added a "ChatOps operator commands" subsection in the existing "ChatOps Escalation" section. Documents the four verbs, the syntax, the repo-substring rule, and the confirmation flow for `wipe-workspace`.
- [x] 7.2 Cross-references the existing perma-stuck and needs-spec-revision marker patterns and notes that the chatops verbs are the in-chat equivalent of the SSH-and-rm-the-file workflow.

## 8. Spec delta

- [x] 8.1 The ADDED requirement "Chatops operator commands" is in place at `openspec/changes/a03-chatops-operator-commands/specs/orchestrator-cli/spec.md`. Enumerates the four verbs, the syntax contract, the repo-substring matching rule, the wipe-workspace confirmation flow, and the silent-ignore for unknown verbs.

## 9. Verification

- [x] 9.1 `cargo test` passes (623 tests, 0 failed, 1 ignored).
- [x] 9.2 `openspec validate a03-chatops-operator-commands --strict` passes.
