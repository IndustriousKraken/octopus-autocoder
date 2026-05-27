## 1. JSON event types + parser

- [x] 1.1 Create `autocoder/src/executor/json_event.rs`. Define the event types Claude CLI emits in `--output-format stream-json` mode:
  ```rust
  pub enum JsonEvent {
      System { content: serde_json::Value },
      Assistant { content_blocks: Vec<AssistantBlock> },
      User { content_blocks: Vec<UserBlock> },
      Result { stop_reason: String, final_text: String },
      Unknown { event_type: String, raw: serde_json::Value },
  }
  pub enum AssistantBlock {
      Text { text: String },
      ToolUse { tool_name: String, tool_input: serde_json::Value },
  }
  pub enum UserBlock {
      ToolResult { tool_use_id: String, content: String, is_error: bool },
  }
  pub fn parse_event_line(line: &str) -> Result<JsonEvent, ParseError>;
  ```
  The exact JSON field names follow Claude CLI's actual stream-json schema. The implementer reads the current Claude CLI documentation OR a sample stream-json output to confirm field names; the spec's contract is that the events get categorized into the variants above (tool calls, tool results, intermediate text, the final result).
- [x] 1.2 The parser is permissive: unknown `type` values return `Unknown { event_type, raw }`; malformed JSON returns `Err(ParseError::MalformedJson { underlying })`. Neither aborts the parsing loop.
- [x] 1.3 Tests with fixture JSON lines:
  - `{"type":"assistant","message":{"content":[{"type":"text","text":"hello"}]}}` parses as `Assistant { content_blocks: [Text { text: "hello" }] }`.
  - `{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"path":"src/foo.rs"}}]}}` parses with a `ToolUse` block.
  - `{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"abc","content":"file contents...","is_error":false}]}}` parses with a `ToolResult` block.
  - `{"type":"result","stop_reason":"end_turn","result":"I've fixed the bug..."}` parses as `Result { stop_reason: "end_turn", final_text: "I've fixed the bug..." }`.
  - `{"type":"future_event_kind","foo":"bar"}` parses as `Unknown { event_type: "future_event_kind", .. }`.
  - Malformed JSON line returns `Err`.

## 2. StructuredLogWriter

- [x] 2.1 Create `autocoder/src/executor/event_log.rs`. Public surface:
  ```rust
  pub struct StructuredLogWriter { /* internals */ }
  pub fn open(path: &Path) -> Result<StructuredLogWriter>;
  impl StructuredLogWriter {
      pub fn write_prompt(&self, prompt: &str) -> Result<()>;
      pub fn append_action(&self, kind: ActionKind, content: &str) -> Result<()>;
      pub fn set_final_answer(&self, text: String) -> Result<()>;
      pub fn append_stderr(&self, bytes: &[u8]) -> Result<()>;
      pub fn finalize(&self) -> Result<()>;
      pub fn final_answer(&self) -> Option<String>;
  }
  pub enum ActionKind { ToolUse, ToolResult, Assistant, Raw, Unknown(String) }
  ```
- [x] 2.2 `write_prompt` writes the `=== PROMPT (<n> bytes) ===\n<content>\n\n=== ACTIONS ===\n` header. The ACTIONS section's content is appended over the run; the FINAL ANSWER section is written by `finalize`.
- [x] 2.3 `append_action` formats each event as a single line:
  - `ToolUse` → `[tool_use] <tool_name> <one-line-summary-of-input>` (truncate tool_input to ~200 chars for the log; full input goes to ACTIONS appended as a follow-up indented block when needed).
  - `ToolResult` → `[tool_result] (<N> bytes returned)` for normal results, `[tool_result:error] <message>` for errors.
  - `Assistant` → `[assistant] <text>` (text wrapped to ~80 cols for readability; multi-paragraph text becomes multiple `[assistant]` lines).
  - `Raw` → `[raw] <line content>` (when JSON parsing fails).
  - `Unknown(type)` → `[unknown:<type>] <raw JSON>` (when the event type isn't recognized).
- [x] 2.4 `set_final_answer` captures the `result` event's `final_text` field. Held in memory until `finalize` writes the FINAL ANSWER section at the end of the log file.
- [x] 2.5 `finalize` writes the trailing sections:
  ```
  
  === FINAL ANSWER (<n> bytes) ===
  <final_answer content, or empty if None>
  
  === STDERR (<n> bytes) ===
  <stderr content, or empty>
  ```
  Updates the PROMPT-section size annotation header if needed.
- [x] 2.6 Tests:
  - `write_prompt` then `append_action` × 3 then `set_final_answer` then `finalize` → log file has PROMPT, ACTIONS (with 3 lines), FINAL ANSWER (with text), STDERR (empty) sections in order.
  - `final_answer()` returns the captured text post-`finalize`.
  - When `set_final_answer` is never called (timeout case), `finalize` writes an empty FINAL ANSWER section and `final_answer()` returns None.

## 3. Refactor executor spawn + capture

- [x] 3.1 In `autocoder/src/executor/claude_cli.rs`, modify the spawn command:
  - When `executor.output_format == "json"`, append `--output-format stream-json` to the args.
  - When `"text"`, omit the flag (today's behavior).
- [x] 3.2 Replace the existing `wait_with_output` capture path with streaming:
  1. Open the structured log file: `let log = event_log::open(log_path)?;`
  2. `log.write_prompt(&prompt)`.
  3. Spawn the child with stdout + stderr piped (existing).
  4. Spawn a `tokio::spawn` task that reads stdout line-by-line:
     ```rust
     let mut reader = BufReader::new(child.stdout.take().unwrap()).lines();
     while let Some(line) = reader.next_line().await? {
         match json_event::parse_event_line(&line) {
             Ok(event) => dispatch_event_to_log(&log, event)?,
             Err(_) => log.append_action(ActionKind::Raw, &line)?,
         }
     }
     ```
  5. Spawn a `tokio::spawn` task that reads stderr and accumulates via `log.append_stderr`.
  6. Race `child.wait()` against the configured timeout. On timeout, kill via the existing SIGTERM/SIGKILL helper.
  7. Await both reader tasks.
  8. `log.finalize()`.
  9. Return the outcome shape, populated with `log.final_answer()` as a new field.
- [x] 3.3 `dispatch_event_to_log` handles event categorization:
  - `Assistant` with `Text` blocks → for each text block, `append_action(Assistant, text)`.
  - `Assistant` with `ToolUse` blocks → `append_action(ToolUse, "<tool_name> <input summary>")`.
  - `User` with `ToolResult` blocks → `append_action(ToolResult, "<size or error message>")`.
  - `Result` event → `log.set_final_answer(event.final_text)`.
  - `Unknown` → `append_action(Unknown(event_type), <raw json>)`.
- [x] 3.4 Tests:
  - Fixture child that emits 3 tool_use events then a result event → log has 3 actions in ACTIONS section AND the result text in FINAL ANSWER section.
  - Fixture child that gets killed mid-stream → log has the events received before kill in ACTIONS; FINAL ANSWER is empty.
  - Both stdout AND stderr non-empty → ACTIONS has stdout events; STDERR section has stderr bytes.

## 4. Executor outcome shape extension

- [x] 4.1 The `SubprocessOutcome` (or whatever the executor's internal outcome type is called) gains a `pub final_answer: Option<String>` field. Populated from `event_log.final_answer()` after capture completes. None when the run timed out before the result event arrived.
- [x] 4.2 The outer `ExecutorOutcome` enum's `Completed` variant exposes the final answer (via a new field OR a method on the outcome struct, depending on how today's surface is shaped). Other variants (Failed, AskUser) don't carry a final answer.
- [x] 4.3 Tests:
  - `Completed` outcome from a successful JSON run carries `final_answer: Some(text)`.
  - `Failed` outcome (timeout) carries no final answer.

## 5. PR-comment construction reads FINAL ANSWER

- [x] 5.1 In `autocoder/src/polling_loop.rs` (the path that constructs the "Agent implementation notes" PR comment), replace the existing "read full stdout from log file" logic with "read `event_log::final_answer(log_path)` from the log file's FINAL ANSWER section".
- [x] 5.2 Add `pub fn read_final_answer(log_path: &Path) -> Option<String>` to `event_log.rs`. Parses the log file looking for the `=== FINAL ANSWER (<n> bytes) ===` section header AND returns the content between that header AND the next section header (or EOF). Returns None when the section is missing OR empty.
- [x] 5.3 When `read_final_answer` returns None for a change whose PR is being constructed, the comment body uses a fallback string: `(executor timed out before final summary; see daemon log for action stream)`. The PR is still created with whatever commits landed; the comment just notes the gap.
- [x] 5.4 Tests:
  - Log file with populated FINAL ANSWER section → `read_final_answer` returns the text.
  - Log file with empty FINAL ANSWER section → returns None.
  - Log file with no FINAL ANSWER section at all (legacy format from text-mode opt-out) → returns None; PR-comment falls back to reading raw stdout (today's behavior preserved for the opt-out path).
  - PR-comment integration test: fixture iteration with a successful change → comment body matches FINAL ANSWER content; fixture iteration with a timeout → comment body is the documented fallback string.

## 6. Log retention policy

- [x] 6.1 In `autocoder/src/config.rs`, extend `ExecutorConfig` with `pub log_retention_days: u32` defaulting to `30` via `#[serde(default = "default_log_retention_days")]`. Clamp values above `365` to `365` with WARN at startup.
- [x] 6.2 Create `autocoder/src/log_retention.rs`. Public surface:
  ```rust
  pub struct RetentionConfig { pub days: u32 }
  pub fn prune_stale_logs(logs_root: &Path, workspaces_root: &Path, config: &RetentionConfig) -> Result<PruneReport>;
  pub struct PruneReport { pub files_deleted: u32, pub bytes_freed: u64, pub files_preserved: u32 }
  ```
- [x] 6.3 `prune_stale_logs` walks `<logs_root>/runs/<workspace>/<change>.log`, checking each file's mtime. A log is eligible for deletion when:
  - Its mtime is older than `now - days * 86400` seconds, AND
  - Its corresponding change directory at `<workspaces_root>/<workspace>/openspec/changes/<change>/` does NOT exist (the change has been archived AND that archive is now older than the retention window; OR the change was deleted).
  Files whose change directory still exists are PRESERVED regardless of log age — operators investigating long-running stuck changes want their logs even if old.
- [x] 6.4 The retention pass runs at daemon startup (after the existing startup checks) AND once every 24 hours via a periodic tokio task. Logs the report.
- [x] 6.5 Tests:
  - Fixture: log file 60 days old + change is archived → log is deleted.
  - Fixture: log file 60 days old + change directory still active → log is preserved.
  - Fixture: log file 10 days old + change archived → log preserved (within retention window).
  - Fixture: prune dry-run mode (optional; only for testing) reports what would be deleted without acting.

## 7. Opt-out path: text mode

- [x] 7.1 When `executor.output_format == "text"`, the executor:
  - Spawns Claude CLI WITHOUT the `--output-format stream-json` arg.
  - Uses the today-style at-exit capture path (no streaming, no event parser).
  - Writes the legacy log shape (`=== PROMPT ===`, `=== STDOUT ===`, `=== STDERR ===` sections).
  - The PR-comment construction path detects the legacy log shape AND falls back to reading raw stdout (today's behavior).
- [x] 7.2 Tests:
  - Opt-out config → spawn command lacks `--output-format`.
  - Opt-out config → log file uses legacy `=== STDOUT ===` section name.
  - Opt-out config → PR comment uses raw stdout as today.

## 8. README + docs updates

- [x] 8.1 In `docs/OPERATIONS.md`, replace the existing per-change log description with the new structured shape (PROMPT + ACTIONS + FINAL ANSWER + STDERR sections). Explain the ACTIONS section's content and how operators read it for timeout diagnostics.
- [x] 8.2 In `docs/CONFIG.md`, document `executor.output_format` and `executor.log_retention_days` fields.
- [x] 8.3 In `docs/TROUBLESHOOTING.md`'s "agent timeout" entry, replace today's "log is empty so you have no signal" with "log's ACTIONS section shows the agent's tool-call history up to the kill moment; the last action line names what the agent was doing when timeout fired".
- [x] 8.4 The `## Agent implementation notes` comment shape on PRs is unchanged from an operator-reading perspective — they see the agent's final conversational summary, same as today. Document this stability so operators know their existing PR-review workflows don't change.

## 9. Spec delta

- [x] 9.1 The ADDED requirement in `openspec/changes/executor-streams-output-incrementally/specs/executor/spec.md` codifies: the JSON event streaming contract, the log-file structured-section shape (PROMPT, ACTIONS, FINAL ANSWER, STDERR), the FINAL-ANSWER-vs-ACTIONS separation, the PR-comment routing, the timeout-fallback string, the log retention policy and active-change preservation rule, and the text-mode opt-out semantics.

## 10. Verification

- [x] 10.1 `cargo test` passes (new + existing).
- [x] 10.2 `openspec validate executor-streams-output-incrementally --strict` passes.
- [x] 10.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
