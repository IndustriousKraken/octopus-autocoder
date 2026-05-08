## Context

The architecture spec defines `ExecutorOutcome::AskUser { question, resume_handle }` and `Executor::resume(handle, answer)`. Phase-1-foundation explicitly defers detection and routing of AskUser to this change. We need (a) a Slack integration, (b) a small state machine on disk to persist question/answer pairs across daemon restarts, (c) a modified iteration order in the polling loop so answered questions are resolved before new work starts, and (d) AskUser detection in the `ClaudeCliExecutor`.

## Goals / Non-Goals

**Goals:**
- A `chatops-manager` module that handles posting, polling, and the question/answer file lifecycle.
- Live escalation flow: when the executor returns `AskUser`, the orchestrator persists state and continues to the next change without blocking the queue.
- Resume-on-answer: when chatops-manager observes a human reply, the orchestrator picks up the change on the next iteration, reads the handle, deletes `.question.json`, calls `Executor::resume`, and handles the new outcome identically to a fresh `run` outcome.
- AskUser detection in `ClaudeCliExecutor` (the only executor backend so far).

**Non-Goals:**
- Interactive Slack elements (buttons, modals). Plain-text thread replies only.
- Multi-channel routing per change. One channel per repo via config; per-repo override of the global default is supported.
- Reply attribution beyond "first non-bot message in the thread."
- Pinging or escalation if no answer after N hours. Out of scope; the change waits indefinitely until a human replies or the user manually unblocks it.

## Decisions

- **State machine on disk:**
  - `.question.json`: `{ "thread_ts": "<slack ts>", "channel": "<id>", "resume_handle": <opaque executor payload>, "asked_at": "<RFC3339>" }`. Created when the orchestrator processes an `AskUser` outcome.
  - `.answer.json`: `{ "answer": "<text>", "answered_at": "<RFC3339>", "answerer_user_id": "<slack user id>" }`. Created when chatops-manager observes the first non-bot reply in the tracked thread.
  - Both files live alongside the change's `proposal.md` so they are visible to humans inspecting the workspace and survive daemon restart.
- **Iteration order and same-repo queue blocking:** in each polling iteration, the orchestrator first processes waiting changes (those with `.question.json`). After that step, if ANY change in the repository is still waiting, the orchestrator skips the pending-change loop entirely for this iteration. Pending changes are processed only when the waiting set is empty at the end of the waiting-processing step. Rationale: the architecture's serial-queue invariant exists because changes within a repository can depend on each other (Change B is authored assuming Change A's restructuring landed). Running B while A is still waiting produces a confidently-wrong implementation on a half-formed foundation. Strict-block is the conservative default; cross-repo polling tasks are unaffected (each repo is independent), and operators have three documented escape hatches when a wait drags on: reply in Slack to resume, manually delete `.question.json` to revert the change to pending, or `rewind` to fully reset the queue.
- **Consume-before-resume:** per the architecture spec's executor contract, the orchestrator deletes `.question.json` BEFORE invoking `Executor::resume`. After the resume returns (any outcome), `.answer.json` is deleted. If the daemon crashes between those steps, the change reverts to pending state on next startup and re-runs from scratch — acceptable corner case, rare.
- **Slack API surface (minimal):**
  - `POST https://slack.com/api/chat.postMessage`, body `{ channel, text, link_names: 1 }`, header `Authorization: Bearer <bot_token>`. Returns `{ ok, ts, ... }`.
  - `GET https://slack.com/api/conversations.replies?channel=<chan>&ts=<ts>`. Returns `{ ok, messages: [...] }`.
  - `POST https://slack.com/api/auth.test` once at startup to capture the bot's own `user_id` for reply attribution.
- **AskUser detection in `ClaudeCliExecutor`:** two-layer.
  - **Layer 1 (preferred):** an `ask_user` MCP tool is wired into the wrapped CLI's MCP config. When the agent calls it, the tool writes `<workspace>/openspec/changes/<change>/.askuser-pending.json` containing `{ "question": "<arg>" }` and signals the CLI to exit. The orchestrator detects the file after the child process returns and converts to `AskUser`.
  - **Layer 2 (heuristic backstop):** if `.askuser-pending.json` is absent AND the CLI exited 0 AND the workspace has no diff AND the captured stdout matches a clarification regex (case-insensitive: `\b(could you|please) (clarify|specify|tell me|provide)\b`), the orchestrator treats the matched text as a question and constructs an `AskUser`. Heuristic is documented in code; false negatives are caught downstream by the reviewer agent.
- **`ResumeHandle` payload for `ClaudeCliExecutor`:** JSON containing the change name plus any opaque conversation-id (claude session id if available, otherwise a unique correlation id). The handle is stored in `.question.json` as the `resume_handle` field.
- **Bot identity caching:** `chatops_manager.new(...)` calls `auth.test` once and caches the returned `user_id`. All subsequent `poll_thread_for_human_reply` calls use the cached id to filter out the bot's own messages.
- **Polling pacing:** the chatops-manager does NOT run its own polling loop. It is invoked once per polling-loop iteration per waiting change. The polling-loop's `poll_interval_sec` (default 300) is the natural pacing for Slack reply detection. Rationale: avoids Slack rate limits and keeps the chatops-manager free of timer state.

## Risks / Trade-offs

- **Risk:** Layer-1 AskUser detection requires the `ask_user` MCP tool to be properly wired into claude-cli's MCP config. If the user's claude-cli setup doesn't have the right config, the layer-1 path silently never fires.
  - **Mitigation:** The orchestrator emits a startup log line confirming the MCP config it wrote for the executor. The smoke test in `docs/chatops-smoke-test.md` verifies the layer-1 path explicitly.
- **Risk:** Layer-2 heuristic regex produces false positives (the agent says "could you clarify the next step yourself" as part of an unrelated thought).
  - **Mitigation:** The heuristic only fires when there is no diff AND no MCP marker. The reviewer agent provides a final-line backstop. If false positives become common, tighten the regex or remove the heuristic entirely.
- **Risk:** Daemon crash mid-resume loses the partial answer (`.question.json` deleted, `.answer.json` still present, but resume incomplete).
  - **Mitigation:** Documented corner case. The change reverts to pending and re-runs from scratch. If common, a future change can introduce a finer-grained `.resuming.json` marker.
- **Risk:** Slack bot token leaks into log output.
  - **Mitigation:** Token loaded from env var; never included in any log line. HTTP request bodies are not logged. Test reviews the log output for token substring before each change is approved.
- **Risk:** A change waits indefinitely if the human never replies.
  - **Mitigation:** Out of scope for this change. Other changes continue to process. Operators can manually delete `.question.json` to unblock; the change reverts to pending.
