## Why

Phase-1-foundation handles `ExecutorOutcome::AskUser` by logging an error and exiting. To run unattended, the daemon must instead route the agent's question to a human via Slack, persist conversation state to disk so it survives a restart, resume the implementation when an answer arrives, and continue processing other changes in the meantime.

## What Changes

- Add `chatops-manager` capability: posts questions to a configured Slack channel via `chat.postMessage`, polls thread replies via `conversations.replies`, owns the `.question.json` and `.answer.json` file lifecycle for each change.
- Modify `orchestrator-cli`: in every polling iteration, process waiting (escalated) changes before pending (fresh) changes; on `AskUser`, persist state and continue rather than fail; on detected answer, call `Executor::resume`.
- Modify `openspec-queue-engine`: changes containing a `.question.json` file (waiting on a human) are filtered from the pending enumeration; a new `list_waiting` enumeration returns these for resume processing.
- Implementation: detect `AskUser` in `ClaudeCliExecutor` via a wired-in `ask_user` MCP tool plus a heuristic stdout backstop; implement `Executor::resume` so the answered conversation can continue.

## Capabilities

### New Capabilities
- `chatops-manager`: Slack-side communication and the file state machine (`.question.json` + `.answer.json`) for ChatOps escalation.

### Modified Capabilities
- `orchestrator-cli`: live escalation flow — waiting-first iteration order, AskUser persistence, resume-on-answer.
- `openspec-queue-engine`: filter changes with `.question.json` from pending; expose `list_waiting`.

## Impact

The daemon graduates from "exit on ambiguity" to "ask a human and continue." A single confused agent does not block the queue; other changes process while the human is consulted. All escalation state is on disk so the daemon can be restarted without losing context.
