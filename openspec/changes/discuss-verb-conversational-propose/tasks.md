# Tasks

## 1. Spec delta — orchestrator-cli

- [ ] 1.1 SUPERSEDE the `propose chatops verb queues a chat-driven triage request` requirement with a `discuss chatops verb starts an immediate conversational session` requirement covering: verb rename to `discuss` (with `propose` alias), ack message format (including the @bot-only-seen note), `DiscussionState` schema, `DiscussAction` control-socket action, and the immediate-response SLA (under 60 seconds).
- [ ] 1.2 SUPERSEDE the `Triage prompt classifies the request as DIRECTIVE, QUESTION, or AMBIGUOUS before acting` requirement and the `.chat-reply.md` marker requirement with the new conversational model (no automatic classification, agent responds in thread, `send it` triggers artifact creation).
- [ ] 1.3 Add requirement: `discuss thread continuation` — `@<bot>` replies in a discuss thread submit `DiscussContinueAction`; daemon resumes session with new message; agent replies in thread.
- [ ] 1.4 Add requirement: `send it in discuss thread triggers sequential artifact creation` — `DiscussSendItAction` queues artifact job after current executor; agent session resumes in write mode; PR opened on `agent-q`.
- [ ] 1.5 Add requirement: `send it in discuss thread accepts trailing final context` — text after `send it` is appended to session context before artifact step.
- [ ] 1.6 Add requirement: `auto-defer for existing-spec modification` — agent writes defer marker on first reply when modification of existing spec is identified; posts undefer command in thread; clears defer on PR open; 7-day idle reminder.
- [ ] 1.7 Add requirement: `discuss-mode agent context` — agent proactively reads specs, CHANGELOG, docs, relevant source files before first reply.
- [ ] 1.8 Add requirement: `@bot-only-seen note in all lifecycle thread acks` — all lifecycle thread ack messages (discuss, revision, audit, brownfield, scout, changelog) SHALL include the phrase "Note: only replies that start with @<bot> are seen here." Update each relevant requirement's ack-message scenario.

## 2. Spec delta — chatops-manager

- [ ] 2.1 In the verb-recognition requirement, rename `propose` to `discuss` (with `propose` accepted as alias). Update help text listing.
- [ ] 2.2 In the `send it` dispatch requirement (`Inbound listener dispatches send it by thread context`), add `DiscussionState` as a recognized thread-context type and route to `DiscussSendItAction`.
- [ ] 2.3 Add `DiscussContinueAction` routing: `@<bot>` reply in discuss thread (not `send it`) → `DiscussContinueAction`.

## 3. Code — chatops inbound listener

- [ ] 3.1 Rename `propose` handler to `discuss`; add `propose` as a synonym that dispatches identically.
- [ ] 3.2 Post ack including the @bot-only-seen note.
- [ ] 3.3 Write `DiscussionState` file with `status: Active`.
- [ ] 3.4 Detect in-thread `@<bot>` replies (excluding `send it`): look up `thread_ts` across active `DiscussionState` files; submit `DiscussContinueAction`.
- [ ] 3.5 Detect in-thread `send it [trailing text]`: route to `DiscussSendItAction` carrying the trailing text.
- [ ] 3.6 Add `DiscussionState`-thread detection to the `send it` dispatch chain (alongside revision, audit, brownfield-survey, issue-candidate threads).
- [ ] 3.7 Update all existing lifecycle-thread ack messages (revision, audit, brownfield, scout, changelog) to append the @bot-only-seen note.

## 4. Code — dedicated discuss handler

- [ ] 4.1 Add a `discuss_handler` task (or channel) that the daemon starts at launch alongside the per-repo polling tasks. The handler listens for `DiscussAction` and `DiscussContinueAction` items on a dedicated channel fed by the control-socket dispatcher.
- [ ] 4.2 On `DiscussAction`: spin up an agentic session in read-only sandbox with the discuss-mode prompt; post each reply chunk to the thread as it arrives (or as a single reply on session completion).
- [ ] 4.3 Session state (session ID, cached prefix, thread context) is stored in `DiscussionState` so `DiscussContinueAction` can resume it.
- [ ] 4.4 On `DiscussContinueAction`: resume session (append new message to context); post reply to thread.
- [ ] 4.5 On `DiscussSendItAction`: queue an artifact-creation job on the per-repo sequential executor queue (wait for current executor to finish). Resume session in write mode with trailing context appended. Agent commits artifact; daemon opens PR on `agent-q`. Post "PR opened" reply to thread with URL.
- [ ] 4.6 Implement auto-defer: if the agent's read-only reply writes a marker file (the agent signals "I'm deferring X" via a special MCP tool call or a structured reply field), the handler writes the defer marker, commits/pushes it, and posts the undefer reminder. On `send it` completion (PR opened), the handler clears the defer marker and commits/pushes the removal.
- [ ] 4.7 Implement 7-day idle reminder: track `last_activity_at` in `DiscussionState`; a background check (per the existing periodic-task pattern) posts the reminder and fires once per stale discussion.

## 5. Code — remove obsolete propose machinery

- [ ] 5.1 Remove `pending_proposal_requests` queue drain from `loop_drive.rs` (or mark it as the alias-drain for in-flight state at deploy).
- [ ] 5.2 Remove `proposals.rs` polling handler (the file or its non-alias contents).
- [ ] 5.3 Remove `.chat-reply.md` check and post logic from the polling iteration.
- [ ] 5.4 Archive or supersede `prompts/chat-request-triage.md`; add `prompts/discuss-mode.md` with the new agent context instructions.

## 6. New discuss-mode prompt

- [ ] 6.1 Write `prompts/discuss-mode.md` covering: proactive context reads (specs, CHANGELOG, docs, source files), conversational response style, read-only constraint during discussion, existing-spec-modification deferral signal, and write-mode transition instructions for `send it`.

## 7. Tests

- [ ] 7.1 `discuss` verb and `propose` alias both submit `DiscussAction` with identical fields.
- [ ] 7.2 Ack message contains the @bot-only-seen note.
- [ ] 7.3 In-thread `@<bot>` reply routes to `DiscussContinueAction`, not a new top-level discuss.
- [ ] 7.4 In-thread `send it` routes to `DiscussSendItAction`.
- [ ] 7.5 In-thread `send it plus context` carries the trailing text in `DiscussSendItAction.final_context`.
- [ ] 7.6 `DiscussSendItAction` artifact job waits for a running executor before starting (does not start concurrently).
- [ ] 7.7 `DiscussSendItAction` artifact job starts immediately when no executor is running.
