# Tasks

## 1. Chatops inbound listener

- [x] 1.1 Rename the `propose` handler to `discuss`; keep `propose` as a synonym that dispatches identically (no deprecation warning).
- [x] 1.2 Post the discuss ack including both the "Follow along in this thread." phrase and the "Note: only replies starting with @<bot> are seen here." note.
- [x] 1.3 Write a `DiscussionState` file with `status: Active` on a new discuss request.
- [x] 1.4 Detect in-thread `@<bot>` replies that are NOT `send it`: look up `thread_ts` across active `DiscussionState` files; submit `DiscussContinueAction`.
- [x] 1.5 Detect in-thread `@<bot> send it [trailing text]`: route to `DiscussSendItAction` carrying the trailing text as `final_context`.
- [x] 1.6 Add `DiscussionState`-thread detection as the fifth context in the `send it` dispatch chain (after audit, brownfield-survey, issue-candidate, revision).
- [x] 1.7 Update the untracked-thread refusal string and `help` text to name the five contexts (discuss added).

## 2. Dedicated discuss handler

- [x] 2.1 Add a `discuss_handler` task the daemon starts at launch alongside the per-repo polling tasks. It listens for `DiscussAction` and `DiscussContinueAction` on a dedicated channel fed by the control-socket dispatcher — no polling-loop delay.
- [x] 2.2 On `DiscussAction`: spin up an agentic session in a read-only sandbox with the discuss-mode prompt; post the reply to the thread. Persist the session id/context to `DiscussionState` so continuation can resume it.
- [x] 2.3 On `DiscussContinueAction`: resume the session (append new message to context); post the reply to the thread.
- [x] 2.4 On `DiscussSendItAction`: queue an artifact-creation job on the per-repo sequential executor queue (wait for a running executor to finish; start immediately if none). Resume the session in write mode with `final_context` appended; the agent commits the artifact; the daemon opens a PR on `agent-q`; post the PR URL to the thread.
- [x] 2.5 Auto-defer: when the agent signals it is discussing an existing spec, write and commit the defer marker, then post the undefer reminder naming the `@<bot> undefer` command. On `send it` completion (PR opened), clear and commit the marker removal.
- [x] 2.6 7-day idle reminder: track `last_activity_at` in `DiscussionState`; a background check (existing periodic-task pattern) posts the reminder once per stale discussion holding a defer marker.
- [x] 2.7 Prune `DiscussionState` files whose `last_activity_at` is older than 14 days (existing housekeeping pattern).

## 3. Remove obsolete propose machinery

- [x] 3.1 Remove the `.chat-reply.md` check and post logic from the polling iteration.
- [x] 3.2 Remove the old `propose` triage handler in `proposals.rs` (the classification/two-PR path), retaining only what the `discuss` path needs; wire the `propose` verb to the discuss handler.
- [x] 3.3 Drain any in-flight `ProposalRequestState` files left at deploy time before removing their state type (migration note in RELEASING.md).

## 4. Discuss-mode prompt

- [x] 4.1 Write the new prompt file `prompts/discuss-mode.md` (this task creates a prompt only — it does not touch any specification files). The prompt text instructs the discuss agent to: proactively READ project context for background (the repo's existing specification documents, `CHANGELOG.md`, `OCTOPUS.md`, `docs/*`, active and archived change folders, relevant implementer source files); respond conversationally and read-only during discussion; emit the existing-spec-modification deferral signal; route the output artifact between a roadmap item and a change (per `a01-roadmap-items`); and switch to write mode on `send it`.
- [ ] 4.2 Retire `prompts/chat-request-triage.md` (superseded by the discuss-mode prompt).
  - NOT done: the prompt is still loaded by `Executor::run_chat_triage`, which the
    out-of-scope scout→`spec-it` flow (`polling/spec_it.rs` → `process_proposal_requests`)
    depends on. Deleting it would silently break `spec-it` at runtime. Retiring it
    requires migrating `spec-it` off the shared triage machinery — a separate change,
    since `spec-it` has its own canonical spec. The `discuss` flow itself no longer
    uses this prompt (it uses `prompts/discuss-mode.md`).

## 5. Tests

- [x] 5.1 `discuss` verb and `propose` alias both submit `DiscussAction` with identical fields.
- [x] 5.2 The discuss ack contains the @bot-only-seen note.
- [x] 5.3 An in-thread `@<bot>` reply routes to `DiscussContinueAction`, not a new top-level discuss.
- [x] 5.4 An in-thread `@<bot> send it` routes to `DiscussSendItAction`, not `DiscussContinueAction`.
- [x] 5.5 `send it <trailing text>` carries the trailing text in `DiscussSendItAction.final_context`.
- [x] 5.6 A `DiscussSendItAction` artifact job waits for a running executor before starting.
- [x] 5.7 A `DiscussSendItAction` artifact job starts immediately when no executor is running.
