## Why

When something goes wrong — perma-stuck marker fires, spec-needs-revision marker lands, weird workspace state — the current operator workflow is "SSH to the host running autocoder, sudo around in /tmp/workspaces, edit or delete files, restart the service." That works but takes the operator out of the channel where the chatops alert just landed. The operator already has the diagnostic context in chat; the action they want to take is small. Making them switch contexts to a terminal is friction.

Concretely: every operator who has hit perma-stuck on a change has had to (a) read the chatops alert, (b) SSH, (c) `sudo rm /tmp/workspaces/<repo>/openspec/changes/<change>/.perma-stuck.json`, (d) wait for the next iteration. Step (c) is the only meaningful action; (b) and (d) are workflow overhead. If the operator could reply in the channel — `@autocoder clear-perma-stuck your-repo a06-foo` — and the bot performs the action and reports back, the round-trip drops from minutes to seconds.

There's a second motivating use case: operators want to see at a glance what state the daemon thinks a repo is in. Today this requires inspecting markers + `failure-state.json` + `alert-state.json` + journalctl + git status across multiple files. A `@autocoder status <repo>` command that aggregates the same information into one chat reply is a frequent-use diagnostic shortcut.

The threat model is unchanged from existing chatops behavior: whoever has write access to the channel is trusted as an operator. Sites that need stricter control already configure separate channels per concern (e.g. per-repo `chatops_channel_id` overrides), and the same mechanism applies here.

## What Changes

**Initial verb set (this spec).** Four verbs covering the most common pain points; subsequent specs can add more as patterns emerge:

1. **`status <repo-substring>`** — high-value diagnostic. Returns the daemon's current view of the repo: active markers (perma-stuck, needs-spec-revision), alert-state entries (which 24h throttles are currently engaged), last iteration's outcome + timestamp, queue snapshot (pending, excluded-by-marker, waiting/escalated).

2. **`clear-perma-stuck <repo-substring> <change-slug>`** — removes the named change's `.perma-stuck.json` marker. Next iteration retries the change. Replies with a one-line confirmation OR an actionable error if the marker doesn't exist / the change name doesn't match / the repo substring doesn't resolve.

3. **`clear-revision <repo-substring> <change-slug>`** — removes the named change's `.needs-spec-revision.json` marker (introduced by the companion `a01-spec-needs-revision-outcome` change). Next iteration retries.

4. **`wipe-workspace <repo-substring>`** — equivalent to `sudo rm -rf /tmp/workspaces/<sanitized-url>`. Forces a re-clone on the next iteration. Destructive; requires two-step confirmation in the channel: first message asks "reply 'confirm' within 60 seconds", second message must literally be `confirm`. Confirmation expires; no implicit-yes.

**Message syntax**: `@<bot-mention> <verb> <args>`. Detection happens in the existing chatops backend's incoming-message path (the same path that handles AskUser replies). Messages that don't match a recognized verb are ignored silently (no negative-feedback for typos — operators shouldn't worry about noise).

**Repo identification by substring**: operators type `your-repo`, not `git@github.com:your-org/your-repo.git`. The matcher does case-insensitive substring search against configured `repositories[].url`. Single match → proceed. Multiple matches → reply with the list and ask for more specificity (`matches multiple: foo/bar, baz/bar — be more specific`). Zero matches → reply with the configured repo list.

**Implementation via control-socket extension**: rather than putting logic in the chatops backend, the listener parses the command and submits a JSON action over the existing Unix-domain control socket (today used only for `reload`). The control socket gains new action handlers for each verb. This means: (a) the chatops listener is pure parsing + IPC, no business logic; (b) the same actions are available to a future CLI (`autocoder clear-perma-stuck ...`) for operators who prefer that; (c) authn-via-Unix-socket-perms is the existing trust boundary.

**Pause / resume / clear-alert-throttle**: explicitly **NOT** in this initial spec. They're useful but not as urgent as the four above. Add via follow-up spec(s) if usage patterns warrant. Keeping the initial verb set small reduces review surface and lets the operator-feedback loop tell us which verbs to add next.

**Status command output shape** (one representative example):

```
📊 git@github.com:your-org/your-repo.git

active markers (excluded from list_pending):
  • a06-refactor-portal-handlers (.perma-stuck.json — consecutive_failures: 2, marked 4h ago)
  • a07-stripe-test-mode (.needs-spec-revision.json — marked 22m ago)

24h-throttled alerts currently engaged:
  • workspace_dirty_mid_iteration — last fired 3h ago (15h remaining)
  • branch_push_failure — last fired 8h ago (16h remaining)

last iteration:
  finished: 12m ago
  outcome: 1 change archived (a05-stripe-setup-docs), PR #34 opened
  next iteration: in ~3m (poll_interval 300s, jitter ±10%)

queue snapshot:
  pending: a08-deploy-runbook, a09-restore-from-backup
  waiting (escalated): a10-secrets-rotation (question in chat: <thread-link>)
  excluded: a06, a07 (see markers above)
```

**Reply formatting**: every command's reply uses a tight, scannable shape. Success replies are one line with `✓`. Errors are one line with `✗`. The status command is the only multi-line reply.

## Impact

- Affected specs: `orchestrator-cli` — one ADDED requirement establishing the chatops-operator-commands contract.
- Affected code:
  - `autocoder/src/control_socket.rs` — new action variants + handlers for the four verbs. Existing `reload` action stays unchanged.
  - `autocoder/src/chatops/mod.rs` and per-backend modules (`slack.rs`, `discord.rs`, etc.) — extend the incoming-message handler to parse `@bot <verb> <args>` patterns. The parser is shared across backends; only the per-backend "did the message mention us" detection differs.
  - `autocoder/src/queue.rs` — small helpers: `remove_perma_stuck_marker(workspace, change)`, `remove_revision_marker(workspace, change)`. The existing `is_perma_stuck` / `is_needs_spec_revision_marked` helpers stay.
  - A new module `autocoder/src/chatops/operator_commands.rs` (or similar) housing the verb parser, the repo-substring matcher, and the reply formatter. Pure-data and easy to unit-test.
  - Tests: parser correctness (every verb shape, error cases), repo-substring matcher (single match, multiple matches, no matches, case-insensitivity), control-socket action handlers (clear-marker happy path + non-existent-marker error path), wipe-workspace confirmation flow (pending-confirmation tracking, 60s expiry).
- Operator-visible behavior: new chatops command surface. Operators can do common maintenance from chat. Existing notification + AskUser-reply behaviors are unchanged.
- Breaking: no. Pure addition.
- Acceptance: `cargo test` passes (new tests + existing). A fake chatops backend in tests can drive the full message-in → action → message-out flow without a real Slack/Discord/etc. server.
