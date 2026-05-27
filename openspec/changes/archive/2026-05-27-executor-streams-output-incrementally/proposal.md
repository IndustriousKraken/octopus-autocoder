## Why

Claude CLI's default output mode (`--output-format text`, the implicit default) is silent until the run completes: tool calls (Read, Edit, Write, Bash) happen internally between the CLI and the model and never reach the CLI's stdout; only the final assistant message — typically 1-4 paragraphs summarizing what was done — prints at the very end. When autocoder's timeout enforcement kills the child mid-run, there's nothing in the stdout buffer to capture because nothing had been written there yet. The 0-byte STDOUT in the per-change log on a timeout-kill isn't a buffering artifact; it's literally accurate.

This leaves operators with no signal about what Claude was doing during a long-running change. A real incident: `x01-chat-request-triage` hit the 45-minute timeout twice in a row. `git status` was clean, no commits landed, no tasks.md checkboxes flipped. The log captured the prompt but had nothing else. Triaging the perma-stuck required guessing among "exploring the codebase methodically", "stuck in a loop", "waiting on a stalled upstream API", or "crashed silently in MCP setup" — all plausible, none distinguishable from the log.

The fix is to switch Claude CLI's output mode to its JSON-event-stream form. In that mode, the CLI emits one JSON object per turn — `assistant` text blocks, `tool_use` calls, `tool_result` returns, the closing `result` event — flushed to stdout as each event occurs. Streaming reads on the pipe capture each event in near-real-time; on timeout-kill, every event up to the kill is preserved.

A natural consequence of structured event output: the captured content has two distinct categories. The action stream (tool calls and intermediate text) is diagnostic — useful for operators reading the log, not useful for PR reviewers. The final assistant text is the conversational summary that today fills the PR's "Agent implementation notes" section. Mixing them — dumping the raw event stream into the PR comment — would be both noisy AND a leak vector (the agent's intermediate reasoning shouldn't ride along with the final summary into the PR description). They belong in different places: the action stream in the log file, the final answer in the PR comment.

Today's PR-comment path reads `<workspace>/.../<change>.log` and uses the full stdout content as the comment body. With this change, the executor captures the final assistant text separately AND the PR-comment path reads from that separate field. The log file's content shape changes to include a structured ACTIONS section AND a FINAL ANSWER section; the PR comment shape stays the same (it's still just the agent's conversational summary).

A separate concern surfaced by this change: per-change log files have no retention policy today. With text-only summaries (~1-2 KB), accumulation is slow. With JSON event streams, each run produces ~100x more bytes. Over months of operation the per-change log directory grows unbounded. This spec adds a retention pass that prunes logs older than `executor.log_retention_days` (default 30) at daemon startup AND once per day thereafter.

## What Changes

**Switch Claude CLI invocation to JSON event streaming.** The executor's spawn command gains `--output-format stream-json` (or whatever the precise flag name is in the current Claude Code release — the implementer verifies; the spec's contract is on the JSON-event-per-line output shape, not the exact flag spelling). Each JSON event arrives on stdout as a single line, flushed by Claude CLI at emission time.

**Streaming reader parses events as they arrive.** A `tokio::spawn` task reads stdout line-by-line, parses each line as JSON, dispatches to the right destination:

- `tool_use` and `tool_result` events → format as human-readable lines, append to the log file's ACTIONS section
- `assistant` text blocks (intermediate model messages, e.g. "let me look at X first") → append to the ACTIONS section with an `[assistant]` prefix
- The `result` event (Claude CLI's final-completion marker, which carries the run's stop_reason and the model's final text) → captured separately as the "final answer"; the final text is what PR comments use

If a line fails JSON parsing (corrupt event, partial line at kill, future Claude CLI version with an unknown event type), log WARN naming the line content, write the raw line to the ACTIONS section as `[raw] <line>`, continue processing. Unknown event types are logged similarly with `[unknown:<type>]` and their JSON payload — preserves forward compatibility without crashing the executor.

**Log file shape becomes structured.** The per-change log gains a new section layout:

```
=== PROMPT (<n> bytes) ===
<prompt content>

=== ACTIONS ===
[tool_use] Read autocoder/src/foo.rs
[tool_result] (<n> bytes returned)
[tool_use] Edit autocoder/src/foo.rs
[tool_result] applied: <summary>
[assistant] I've identified the issue in line 42...
[tool_use] Bash cargo test --lib
[tool_result] tests pass (15s)
...

=== FINAL ANSWER (<n> bytes) ===
<final assistant text>
```

The legacy `=== STDOUT ===` / `=== STDERR ===` section names go away (replaced by ACTIONS for diagnostic content). When Claude CLI emits stderr (rare; usually just framework errors), it lands in a separate `=== STDERR ===` section at the bottom for completeness.

**PR-comment reads from FINAL ANSWER, not raw stdout.** The polling-loop code that today reads the full stdout from the log and posts it as the PR's "Agent implementation notes" comment now reads the FINAL ANSWER section specifically. The ACTIONS section is operator-only — it lives in the daemon log and never ships to GitHub.

When the final answer is empty (timeout-kill before the run completed; the `result` event never arrived), the PR comment falls back to the existing "(reviewer failed)"-style placeholder: `(executor timed out before final summary; see daemon log for action stream)`. The PR is still opened with whatever commits actually landed; the comment just notes the missing summary.

**Per-change log retention policy.** A new field `executor.log_retention_days: u32` defaults to `30`. At daemon startup AND once per day during normal operation, a retention pass walks the log directory and deletes per-change `.log` files whose mtime is older than the retention window. Log files for changes that are STILL active (i.e., the change directory still exists under `openspec/changes/<slug>/`, not yet archived) are preserved regardless of age — operators investigating a long-running stuck change want its log even if it's older than 30 days.

**Opt-out for the JSON mode.** A new `executor.output_format: "text" | "json"` config field defaults to `"json"`. Operators who hit edge cases (custom Claude CLI version that lacks the streaming JSON format, log-file size pressure even at the action-stream level, debugging the executor itself) can set `"text"` to fall back to today's at-exit behavior. The opt-out preserves the existing log shape with `=== STDOUT ===` and `=== STDERR ===` sections and feeds raw stdout into the PR comment.

## Impact

- **Affected specs:** `executor` — one ADDED requirement covering JSON event streaming, the log-file section structure, the FINAL ANSWER separation from ACTIONS, the PR-comment routing, the log retention policy, and the opt-out config.
- **Affected code:**
  - `autocoder/src/executor/claude_cli.rs` — append `--output-format stream-json` to the spawn args when `executor.output_format` is `"json"`. Refactor the spawn-and-capture path to stream stdout line-by-line and dispatch parsed events to the structured log writer.
  - New module `autocoder/src/executor/event_log.rs` housing the event-parser + log-writer:
    ```rust
    pub struct StructuredLogWriter {
        file: Arc<Mutex<std::fs::File>>,
        actions_bytes: AtomicU64,
        final_answer: Arc<Mutex<Option<String>>>,
        stderr_bytes: AtomicU64,
    }
    pub fn open(path: &Path) -> Result<StructuredLogWriter>;
    impl StructuredLogWriter {
        pub fn write_prompt(&self, prompt: &str) -> Result<()>;
        pub fn append_action_line(&self, kind: ActionKind, content: &str) -> Result<()>;
        pub fn set_final_answer(&self, text: String) -> Result<()>;
        pub fn append_stderr(&self, bytes: &[u8]) -> Result<()>;
        pub fn finalize(&self) -> Result<()>;
        pub fn final_answer(&self) -> Option<String>;
    }
    pub enum ActionKind { ToolUse, ToolResult, Assistant, Raw, Unknown(String) }
    ```
  - `autocoder/src/executor/json_event.rs` (or similar) housing the JSON event types + parser. The parser is permissive on unknown event types (logs `[unknown:<type>]` instead of erroring).
  - `autocoder/src/polling_loop.rs` — the PR-comment construction path reads `event_log.final_answer()` instead of reading the full stdout. Falls back to the timeout-placeholder when None.
  - `autocoder/src/config.rs` — add `executor.output_format` (default `"json"`) and `executor.log_retention_days` (default `30`, max `365` with WARN-and-clamp).
  - New `autocoder/src/log_retention.rs` (or extend an existing housekeeping module) — runs at startup AND once per day, walks the log directory, deletes stale per-change `.log` files older than `log_retention_days` AND whose change directory is no longer in active path.
  - Tests:
    - JSON parser handles `assistant`, `tool_use`, `tool_result`, `result` event types; unknown types route to `Unknown` without erroring.
    - Streaming dispatch: simulate a child writing 10 JSON-events-per-line then exiting; assert the log file's ACTIONS section has all 10 events formatted; assert the FINAL ANSWER section has the `result`-event content.
    - Timeout-kill mid-stream: simulate a child writing 5 events then getting killed; assert ACTIONS has the 5 events; assert FINAL ANSWER is empty; assert PR-comment uses the timeout-placeholder.
    - PR-comment routing: an arrived `result` event with text X → PR comment body contains X (only X, not the action stream).
    - Log retention: fixture log directory with files of various ages; retention pass deletes files older than `retention_days` whose change is no longer active; preserves files whose change is still active.
    - Opt-out: `executor.output_format: "text"` reverts to today's at-exit-capture behavior; log shape is the legacy format; PR comment uses raw stdout as today.
    - Malformed JSON line: lands in ACTIONS as `[raw] <line>`, WARN logged, processing continues.
    - Unknown event type: lands in ACTIONS as `[unknown:<type>] <json>`, processing continues.

- **Operator-visible behavior:** per-change logs become structured with separate ACTIONS and FINAL ANSWER sections. Operators triaging a timeout see the agent's full action history up to the kill. PR comments contain ONLY the agent's final summary, not intermediate reasoning. Per-change logs are pruned to 30 days by default.
- **Breaking:** the per-change log file's shape changes (new section names). Tooling that reads these files via regex looking for `=== STDOUT ===` would break; opt-out via `executor.output_format: "text"` preserves the legacy shape. The PR-comment body shape on successful runs is observationally identical to today (today's final-summary IS the agent's final assistant text; the new code just extracts it more precisely).
- **Acceptance:** `cargo test` passes (new + existing). A successful Claude run produces a log file with PROMPT + ACTIONS + FINAL ANSWER sections; the PR's "Agent implementation notes" comment matches the FINAL ANSWER section's content exactly. A timeout-killed run produces a log file with PROMPT + ACTIONS sections containing the agent's tool calls up to the kill; the FINAL ANSWER section is empty AND the PR isn't created (timeout → Failed → no PR, today's behavior preserved).
