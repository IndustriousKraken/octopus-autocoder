# `discuss` verb: conversational propose with immediate response

## Why

The existing `propose` verb was designed as a one-shot "queue a request, wait
for the next polling cycle, get an artifact." In practice, that design has two
problems:

**Latency**: repo polling loops are typically 5–10 minutes. An operator asking
a question (`does feature X run concurrently or single-thread?`) waits a full
polling cycle before seeing any reply — the same latency as a full code
implementation. Chat should feel like chat.

**Conversational gap**: the agent may not have enough context from a single
prompt to make the right artifact. The current spec acknowledges this via the
AMBIGUOUS classification (which asks a clarifying question and resumes) but does
not support a natural multi-round discussion before the operator decides whether
to create anything at all. In practice, the agent misinterprets the request or
creates an artifact the operator didn't want because there was no conversational
correction step.

The solution is to replace the one-shot propose flow with a first-class
**conversation loop** anchored to a chatops thread, where `send it` is the
explicit operator signal to create an artifact — not an automatic consequence
of the agent classifying the request as a DIRECTIVE.

## What Changes

### Verb rename and alias

The verb is renamed from `propose` to `discuss`. `propose` remains accepted as
a backward-compatible alias. Both verbs trigger identical behavior. All
operator-facing documentation, help text, and ack messages use `discuss`; the
alias is silent (no deprecation warning).

### Immediate response (decoupled from the polling loop)

When `@<bot> discuss <repo> <text>` is received, the chatops listener:

1. Posts a top-level ack in the channel: `💬 Starting discussion for <repo>. Replies follow in this thread. Only messages starting with @<bot> in this thread are seen.`
2. Captures the ack's `ts` as the discussion's `thread_ts`.
3. Writes a `DiscussionState` file (`status: Active`, `repo_url`, `thread_ts`, `request_id`, `operator_user`).
4. Submits a `DiscussAction` to the daemon via control socket.

The daemon has a **dedicated discuss handler** that is triggered immediately on
control-socket receipt — NOT scheduled at the next polling-loop iteration. The
handler is a lightweight loop independent of the per-repo polling task; it
processes `DiscussAction` items as they arrive with no polling delay. The
target response time from message receipt to first reply in the thread is under
60 seconds under normal load.

The first reply is the agent's response to the initial message. Subsequent
replies may follow as the agent reads files and builds context, per the existing
partial-result streaming pattern.

### Thread continuation

An operator who wants to continue the discussion posts `@<bot> <follow-up
text>` as a reply in the discussion thread. The listener detects the reply as
in-thread (via `thread_ts` match against active `DiscussionState` records) AND
as a continuation (NOT a `send it`) and submits a `DiscussContinueAction`.

The daemon resumes the discussion session with the new message appended to
context. Replies continue in the same thread.

**Only `@<bot>`-prefixed replies in the thread reach the bot.** The ack message
(and any subsequent automated reply) SHALL include the phrase: _"Note: only
replies that start with @<bot> are seen here."_ This is the universal
convention for all autocoder lifecycle threads (revision, audit, brownfield,
discuss); this change makes it explicit in the ack for all of them.

### `send it [optional final context]` in a discuss thread

When the operator posts `@<bot> send it` (with or without trailing text) as a
reply in an active discuss thread, the listener routes it as `DiscussSendItAction`
(separate from the revision-thread and audit-thread `send it` dispatch).

Any text following `send it` is treated as **final context** and appended to
the session before the artifact-creation step. This allows the operator to fold
last-minute clarifications into a single message: `@<bot> send it and let's go
with Option B, keep the existing error format`.

The daemon queues the artifact-creation job to run **sequentially after the
current executor for that repo finishes** (if one is running). If no executor
is running, the job starts immediately. This prevents file conflicts: the
discuss artifact creation uses the same `agent-q` branch and workspace as the
implementation executor.

The artifact-creation step resumes the existing discuss session (reusing cached
context) rather than starting a new agent session. The agent is informed it now
has write capability (can create/modify files) and that its task is to produce
the artifact discussed in the conversation (spec change, issue, roadmap item, or
documentation update). It commits the artifact and the daemon opens a PR on
`agent-q`.

### Session continuation and write capability

The discuss handler maintains session continuity across rounds. The
implementation MAY use token-cache-friendly session resumption (same session ID,
cached prefix) rather than replaying the full conversation into a new context,
to keep response latency low on follow-up messages.

During the discussion phase (before `send it`), the agent has **read-only**
access to the workspace (no file writes, no git commits). On `send it`, the
session transitions to **write mode**: the agent is told it may now create and
modify files, and must commit and push its artifact.

### Auto-defer for existing-spec modification discussions

When the agent, during the discussion phase, determines that the operator is
discussing a modification to an **existing spec** (an already-archived change
being amended, or a requirement in `openspec/specs/*/spec.md`), the agent SHALL:

1. Write the defer marker for the relevant change or spec.
2. Post in the thread: _"I've deferred `<spec-or-change>` while we discuss.
   If you decide not to follow through, clear it with
   `@<bot> undefer <repo> <spec-or-change>`. I'll clear it automatically when
   a PR lands."_
3. On `send it`: after the PR is opened, clear the defer marker.

If the operator does not follow up with `send it` and the discussion goes stale
(no activity for 7 days), the daemon posts a reminder in the thread:
_"This discussion has been idle for 7 days. `<spec-or-change>` is still
deferred. Run `@<bot> send it` to proceed or `@<bot> undefer <repo>
<spec-or-change>` to release it."_ The reminder fires once; the deferred state
persists until explicitly cleared.

### Agent context for discuss sessions

The discuss-mode prompt SHALL instruct the agent to proactively read:

- The repo's `openspec/specs/*/spec.md` files relevant to the topic.
- `CHANGELOG.md` and any `docs/*.md` or `ROADMAP.md` present in the workspace.
- The `openspec/changes/` directory (active and recently archived changes).
- Source files that an implementer would need to modify to carry out the
  discussed change.

This front-loads context so the agent can answer precisely without requiring
extra question rounds.

The agent is also instructed: "You are having a conversation with the operator.
Do NOT create files or open PRs yet. Respond concisely. If the operator's
request is a question, answer it directly. If it is a proposed change, outline
your understanding, name the affected files, and wait for `send it`."

### Removal of the automatic DIRECTIVE / QUESTION / AMBIGUOUS classification

The existing `Triage prompt classifies the request as DIRECTIVE, QUESTION, or
AMBIGUOUS before acting` requirement is SUPERSEDED by this change. The new flow
does NOT classify and act in one step:

- What was `QUESTION`: still answered in the thread; the agent responds
  conversationally. No special classification needed.
- What was `DIRECTIVE`: the agent describes what it would do and waits for
  `send it`. The operator controls whether and when the artifact is created.
- What was `AMBIGUOUS`: the agent asks for clarification in the thread, same as
  before, but now as a natural conversational exchange rather than a formal
  escalation.

The `.chat-reply.md` file mechanism and the `Discussed` status are REMOVED; the
agent replies directly via chatops rather than writing a file for the daemon to
post.

The existing `pending_proposal_requests` queue, `ProposalRequestState` schema,
and `process_proposal_requests` polling handler are REPLACED by
`pending_discuss_requests`, `DiscussionState`, and the dedicated discuss
handler. The control-socket action type changes from `queue_proposal_request` to
`queue_discuss_action`.

## Impact

- **Affected specs**: `orchestrator-cli` (propose verb requirements, chat-request
  triage requirements, `.chat-reply.md` path — all superseded or removed);
  `chatops-manager` (propose verb renamed to discuss, send-it dispatch extended
  for discuss threads).
- **Affected code**: `proposals.rs` (replaced), `polling_loop/loop_drive.rs`
  (discuss handler added), `chatops-inbound` listener (verb rename, thread
  detection, discuss-send-it routing), `prompts/chat-request-triage.md`
  (superseded by new discuss-mode prompt).
- **Backward compatibility**: `propose` continues to work as an alias. Existing
  `ProposalRequestState` files in flight at deploy time are drained by the old
  handler before it is removed; a migration note in RELEASING.md covers this.
