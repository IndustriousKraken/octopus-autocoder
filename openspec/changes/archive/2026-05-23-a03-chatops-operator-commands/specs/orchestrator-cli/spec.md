## ADDED Requirements

### Requirement: Chatops operator commands
The chatops listener SHALL recognize a small set of operator-issued commands as in-channel equivalents of the most common SSH-and-edit operator workflows: querying daemon state, clearing exclusion markers, and wiping the local workspace. Commands SHALL be addressed to the bot via the per-backend mention syntax (Slack `<@bot>`, Discord `<@!bot>`, etc.) followed by a verb and arguments. Unrecognized verbs SHALL be silently ignored (no negative feedback for typos in normal channel chat). Recognized commands SHALL be parsed by a backend-independent parser, dispatched as actions through the existing Unix-domain control socket, and replied to in the same channel where the command arrived.

The initial verb set is:

- `status <repo-substring>` — returns a multi-line summary of the daemon's view of the named repo
- `clear-perma-stuck <repo-substring> <change-slug>` — removes the change's `.perma-stuck.json` marker
- `clear-revision <repo-substring> <change-slug>` — removes the change's `.needs-spec-revision.json` marker
- `wipe-workspace <repo-substring>` — destructive; requires two-step confirmation

The threat model is unchanged from existing chatops behavior: write access to the channel is the trust boundary. Sites needing finer-grained control configure per-repo channels via the existing `chatops_channel_id` override.

#### Scenario: status returns aggregated daemon state for the named repo
- **WHEN** an operator posts `@<bot> status your-repo` in a
  channel where the chatops listener is active AND `your-repo`
  resolves to exactly one configured repository
- **THEN** the bot posts a single multi-line reply containing
  (any subset of these sections may be empty and omitted):
  active markers (`.perma-stuck.json` and
  `.needs-spec-revision.json` entries with their metadata),
  currently-engaged 24h alert throttles, the last iteration's
  outcome + timestamp + next-iteration estimate, AND a queue
  snapshot (pending changes, waiting/escalated changes,
  marker-excluded changes)
- **AND** if `your-repo` matches multiple configured repos, the
  reply lists the matches AND asks for a more specific
  substring
- **AND** if no repo matches, the reply lists every
  configured repo's URL so the operator sees their options

#### Scenario: clear-perma-stuck removes the marker
- **WHEN** an operator posts
  `@<bot> clear-perma-stuck your-repo a06-foo`
- **THEN** the bot resolves the repo, submits a
  `ClearPermaStuckMarker` action to the control socket
- **AND** on success: the marker file is deleted from disk
  AND the bot posts a one-line confirmation
  `✓ cleared .perma-stuck.json for a06-foo on your-repo`
- **AND** the next polling iteration's `list_pending`
  returns the change (assuming no other markers exclude it)
- **AND** on marker-not-found: the bot posts
  `✗ no perma-stuck marker for change a06-foo on your-repo`
  (informational; not retried)

#### Scenario: clear-revision removes the spec-revision marker
- **WHEN** an operator posts
  `@<bot> clear-revision your-repo a07-bar`
- **THEN** the bot resolves the repo, submits a
  `ClearRevisionMarker` action, and on success deletes
  `openspec/changes/a07-bar/.needs-spec-revision.json` AND
  posts the success confirmation
- **AND** failure modes mirror `clear-perma-stuck`:
  no-such-marker / no-such-repo errors with the same shape

#### Scenario: wipe-workspace two-step confirmation
- **WHEN** an operator posts `@<bot> wipe-workspace your-repo`
  in channel `C` AND `your-repo` resolves to a unique repo
- **THEN** the bot posts a warning
  `⚠️ This will delete /tmp/workspaces/<sanitized-url>
  (forces a re-clone on the next iteration). Reply 'confirm'
  within 60 seconds.`
- **AND** the bot stores an in-memory pending-confirmation
  entry keyed by `C` with a 60-second expiry
- **WHEN** the operator (any channel member) replies
  `confirm` in `C` within 60 seconds
- **THEN** the bot submits the `WipeWorkspace` action,
  removes the pending entry, AND posts
  `✓ wiped /tmp/workspaces/<sanitized-url>; next iteration
  will re-clone`
- **AND** if no `confirm` reply arrives within 60 seconds,
  the pending entry expires AND a subsequent `confirm` reply
  is treated as if there were no pending confirmation
  (`✗ no pending wipe-workspace confirmation in this
  channel (or it expired)`)

#### Scenario: Cross-channel confirmations do not match
- **WHEN** the wipe-workspace command is issued in channel A
  AND the `confirm` reply is posted in channel B
- **THEN** channel B's `confirm` does NOT trigger the wipe
  (no pending confirmation exists in channel B)
- **AND** channel A's pending confirmation expires after 60s
  without firing

#### Scenario: Unknown verbs are silently ignored
- **WHEN** a message starts with the bot mention but the
  next token is not in the recognized verb set (e.g.
  `@<bot> hello`, `@<bot> please archive everything`, an
  AskUser reply that doesn't match an open question)
- **THEN** the operator-command parser returns `None`
- **AND** the chatops listener continues to the existing
  AskUser-reply detection path (so chatops-escalation
  replies still work as today)
- **AND** if neither path matches, the message is ignored
  silently (no error reply, no log spam beyond the existing
  message-received DEBUG log)

#### Scenario: Repo-substring matching is case-insensitive
- **WHEN** an operator posts `@<bot> status MYREPO`,
  `@<bot> status YOUR-REPO`, or `@<bot> status your-repo`
- **THEN** all three forms resolve to the same configured
  repository (assuming the substring is unique under
  case-insensitive matching)

#### Scenario: Chatops commands use the same control socket as autocoder CLI
- **WHEN** any operator command's action is performed
- **THEN** the chatops listener submits the action via the
  existing Unix-domain control socket (the same socket used
  by `autocoder reload`)
- **AND** the new action handlers (RepoStatus,
  ClearPermaStuckMarker, ClearRevisionMarker, WipeWorkspace)
  are reachable in principle to any future CLI subcommand
  (e.g. `autocoder clear-perma-stuck <repo> <change>`)
  without duplicating logic
- **AND** the control socket's existing authn
  (Unix-socket-perms, daemon-user-only) applies identically

#### Scenario: Pause / resume / clear-alert-throttle are deliberately absent
- **WHEN** an operator posts `@<bot> pause your-repo` (or
  `resume`, `clear-alert-throttle`)
- **THEN** the message is parsed as an unknown verb AND
  silently ignored (per the unknown-verbs scenario above)
- **AND** the spec explicitly leaves these verbs to
  follow-up changes when usage patterns indicate they're
  worth adding
