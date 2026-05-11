## 1. Config additions

- [ ] 1.1 Extend the top-level `Config` with `slack: SlackConfig { bot_token_env: String, default_channel_id: String }`. Apply `#[serde(deny_unknown_fields)]`.
- [ ] 1.2 Extend `RepositoryConfig` with optional `slack_channel_id: Option<String>` to override the default channel per repo.
- [ ] 1.3 Update `config.example.yaml` with the new Slack fields and a per-repo channel override example.
- [ ] 1.4 **Verify:** `cargo test config::tests::loads_with_slack` parses an example with the new keys; `cargo test config::tests::repo_overrides_channel` confirms per-repo override resolution.

## 2. ChatOps manager module

- [ ] 2.1 Create `src/chatops.rs` with `pub struct ChatOps { client: reqwest::Client, bot_token: String, bot_user_id: String }`.
- [ ] 2.2 Implement `pub async fn ChatOps::new(bot_token: String) -> Result<Self>`: POSTs to `auth.test`, caches `user_id` on success, returns an error naming the Slack `error` field on failure.
- [ ] 2.3 Implement `pub async fn post_question(&self, channel: &str, change: &str, question: &str) -> Result<String>` returning `thread_ts`. POSTs to `chat.postMessage` with body `{ channel, text: format!("❓ `{change}`: {question}"), link_names: 1 }`. On a 2xx response with `ok: false`, return `Err(anyhow!("slack post failed: {error}"))`. On non-2xx, return `Err(anyhow!("slack post http {status}"))`.
- [ ] 2.4 Implement `pub async fn poll_thread_for_human_reply(&self, channel: &str, thread_ts: &str) -> Result<Option<HumanReply>>`. GETs `conversations.replies`. Returns the earliest message whose `bot_id` field is absent AND whose `user` field differs from `self.bot_user_id`. Returns `None` if no qualifying message exists.
- [ ] 2.5 Implement file lifecycle helpers: `write_question_file`, `read_question_file`, `write_answer_file`, `read_answer_file`, `delete_question_file`, `delete_answer_file`. Writes are atomic (`tempfile::NamedTempFile::new_in(...)` + `persist`) so a torn write cannot be observed. Deletes are idempotent (no error if already absent).
- [ ] 2.6 **Verify:** `cargo test chatops::tests::post_question_*`, `poll_picks_first_non_bot_reply`, `poll_returns_none_when_only_bot_messages`, and `file_helpers_atomic_write` against `mockito` HTTP server fixtures and `tempfile::TempDir`.

## 3. Queue engine: waiting state

- [ ] 3.1 Update `queue::list_pending` to also exclude any directory containing a `.question.json` file.
- [ ] 3.2 Implement `pub fn list_waiting(workspace: &Path) -> Result<Vec<String>>` returning sorted change names that contain `.question.json`.
- [ ] 3.3 **Verify:** `cargo test queue::tests::pending_excludes_waiting`, `queue::tests::list_waiting_returns_questioned`, `queue::tests::list_waiting_excludes_archive`.

## 4. ClaudeCliExecutor: AskUser detection + resume

- [ ] 4.1 Add a small MCP tool `ask_user(question: string)` exposed to the wrapped CLI: when called, writes `<workspace>/openspec/changes/<change>/.askuser-pending.json` containing `{ "question": "<arg>" }` and returns a "halt and exit" signal to the CLI. Wire this tool into whatever MCP config `ClaudeCliExecutor` uses to launch the wrapped CLI.
- [ ] 4.2 In `ClaudeCliExecutor::run`, after the child process exits AND before mapping the outcome, check for `.askuser-pending.json`. If present: read the question, build a `ResumeHandle` from `{ "change": "<name>", "session_id": "<claude session id if available>" }`, delete `.askuser-pending.json`, return `Ok(ExecutorOutcome::AskUser { question, resume_handle })`.
- [ ] 4.3 As a heuristic backstop (Layer 2): if `.askuser-pending.json` is absent AND the CLI exited 0 AND `git status --porcelain` is empty AND captured stdout matches `(?i)\b(could you|please) (clarify|specify|tell me|provide)\b`, construct an `AskUser` whose `question` is the first sentence containing the match. Document the heuristic in code comments.
- [ ] 4.4 Implement `Executor::resume` for `ClaudeCliExecutor`: re-invoke the configured CLI with the original change context PLUS a synthetic prepended message `"(Earlier you asked a question and the human answered: <answer>) Continue the implementation."` Use `resume_handle.session_id` to resume claude's session if the CLI supports it; otherwise re-prompt from scratch with the answer appended.
- [ ] 4.5 **Verify:** `tests/executor_askuser_smoke.rs` (gated behind `claude-cli-smoke` feature) creates a fixture change whose `tasks.md` is intentionally ambiguous ("create a file with the project's name as content"), runs `executor.run`, asserts `AskUser { question, .. }` with non-empty `question`, then runs `executor.resume(handle, "use the name SAMPLE")`, asserts `Completed`, asserts a file containing `SAMPLE` exists in the workspace.

## 5. Orchestrator-cli: escalation flow

- [ ] 5.1 In `execute_one_pass`, BEFORE the existing pending-change loop, add a "process waiting changes" loop:
  - For each `change` in `queue::list_waiting(workspace)?`:
    - Read `.question.json` to get `thread_ts`, `channel`, `resume_handle`.
    - Call `chatops.poll_thread_for_human_reply(channel, thread_ts)`.
    - If `None`: continue to next waiting change.
    - If `Some(reply)`: write `.answer.json` (containing reply text, user id, timestamp); delete `.question.json`; call `executor.resume(resume_handle, &reply.text)`; on `Completed` → if diff exists then commit, then archive (cleans up the `.answer.json` automatically as part of directory move); on `AskUser` again → delete `.answer.json`, call `chatops.post_question(...)`, write a fresh `.question.json`; on `Failed` → log the reason, delete `.answer.json` so the change reverts cleanly to pending state.
- [ ] 5.1a After the waiting-processing loop completes, re-check `queue::list_waiting(workspace)?`. If the result is non-empty, log `"queue blocked for {url}: {N} change(s) still waiting on human reply: {names}"` and SKIP the pending-change loop for this iteration. Proceed directly to the iteration's sleep step. This enforces the same-repo serial-queue invariant: pending changes are not processed while any earlier change is awaiting human input.
- [ ] 5.2 In the existing pending-change loop, change the `AskUser` handling from "log + exit" to: call `chatops.post_question(channel, change, &question)` to obtain `thread_ts`; write `.question.json` containing the `thread_ts`, channel, `resume_handle`, and `asked_at` timestamp; `queue::unlock(workspace, change)` (architecture already requires unlock-on-any-outcome); proceed to next change.
- [ ] 5.3 Resolve the per-change channel: `repo.slack_channel_id.as_deref().unwrap_or(&config.slack.default_channel_id)`.

## 6. Documentation

- [ ] 6.1 Update `README.md` to describe the ChatOps escalation flow, the Slack bot setup (required scopes: `chat:write`, `channels:history`, `channels:read`), the `slack` and per-repo `slack_channel_id` config keys, AND the same-repo queue-blocking policy. The README MUST explain the three operator escape hatches for a stuck waiting change: reply in Slack to resume; manually delete `.question.json` to revert the change to pending state and retry from scratch; run `orchestrator rewind <change>` to fully reset the queue and the agent branch.
- [ ] 6.2 Document `.question.json` and `.answer.json` as workspace artifacts: safe to inspect, unsafe to modify by hand, and removed automatically when a change is archived.
