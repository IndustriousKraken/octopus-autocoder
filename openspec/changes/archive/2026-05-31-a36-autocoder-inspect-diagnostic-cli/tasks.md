# Tasks

## 1. CLI scaffolding

- [x] 1.1 In `autocoder/src/cli/mod.rs`, add `Inspect { command: InspectSubcommand }` to the `Command` enum.
- [x] 1.2 Define `InspectSubcommand` enum with three variants: `Rag { workspace: Option<String>, query: String, top_k: Option<u32>, show_bodies: bool, json: bool }`, `Log { workspace: Option<String>, change: String, limit: Option<u32>, json: bool }`, `ToolUsage { workspace: Option<String>, change: String, json: bool }`.
- [x] 1.3 Wire the dispatch arm in the main `match command` block: `Command::Inspect { command } => inspect::dispatch(command).await`.
- [x] 1.4 Add `inspect` module to `cli/mod.rs`: `mod inspect;`.

## 2. Workspace resolution helper

- [x] 2.1 In `autocoder/src/cli/inspect.rs` (OR a shared module), add `resolve_workspace_basename(arg: Option<String>) -> Result<String>`. Behavior:
  - When `Some(s)` AND `s` contains a `:` OR starts with `http`: treat as URL, sanitize via `crate::workspace::sanitize_url_to_basename`, return.
  - When `Some(s)` (otherwise): treat as basename, return verbatim.
  - When `None`: read the daemon's config, find configured workspaces. If exactly one, use it. If zero or more than one, return `Err` with a clear message listing the available basenames.
- [x] 2.2 Unit-test each branch (URL, basename, omit-single, omit-multi, omit-none).

## 3. `autocoder inspect rag`

- [x] 3.1 Implement `inspect::rag(args)` in `cli/inspect.rs`:
  - Resolve workspace basename per the helper.
  - Resolve control socket path via `DaemonPaths` (constructed via the daemon's env-driven resolution).
  - Connect via `UnixStream::connect`. On failure, print `error: control socket unreachable at <path>: <error>. Is the daemon running? (systemctl status autocoder)` AND exit `2`.
  - Send `{"action":"query_canonical_specs","workspace_basename":"<basename>","query":"<query>","top_k":<N>}` on a single line followed by `\n`.
  - Read the response (single-line JSON).
  - Parse `hits[]`.
  - If `--json`: print the raw response. Else render the table per the proposal's example.
  - On `--show-bodies`: after the table, print one section per hit with `## <capability>/<requirement_title>` followed by the first 500 chars of `requirement_body`.
- [x] 3.2 Render helper: format the table with aligned columns (score to 3 decimal places, capability and requirement truncated to fit terminal width OR a fixed reasonable width like 80 cols).
- [x] 3.3 Unit-test rendering against a canned hits array (assert the output contains expected substrings).
- [x] 3.4 Integration-test with a mock control socket that returns a canned response (assert exit 0 AND expected output).
- [x] 3.5 Integration-test the unreachable-socket path (assert exit 2 AND error message contains the socket path).

## 4. `autocoder inspect log`

- [x] 4.1 Implement `inspect::log(args)` in `cli/inspect.rs`:
  - Resolve workspace basename per the helper.
  - Resolve stream-log path: `<logs_dir>/runs/<basename>/<change>.stream.log`.
  - On file-not-found: print `error: no stream log at <path>. Available changes in this workspace: <list>` (enumerate `*.stream.log` siblings) AND exit `2`.
  - Read summary log path: `<logs_dir>/runs/<basename>/<change>.log`. Surface both paths in the header.
  - Parse the stream log line by line. The format is one `[tool_use] ...` OR `[tool_result] ...` OR `[assistant] ...` per line.
  - Group `tool_use` AND its matching `tool_result` (by `tool_use_id` if present in the format; else by position).
  - For `tool_use query_canonical_specs`: extract query AND top_k from the input. For its `tool_result`: extract hit count AND top score from the JSON content.
  - Render the formatted log per the proposal's example.
  - On `--json`: print the parsed event stream as a JSON array.
  - On `--limit N`: cap rendered tool calls at N (default 30). `--limit 0` means unlimited.
  - After the tool-call section, render the FINAL ANSWER section from the summary `.log` file (its existing FINAL ANSWER block).
- [x] 4.2 Parser helper: extract `tool_use_id`, tool name, input fields from a `[tool_use] ...` line. Extract corresponding fields from `[tool_result] ...`. Handle both string-content AND structured-content forms.
- [x] 4.3 Integration-test against a fixture stream log (assert expected output contains expected sections).
- [x] 4.4 Integration-test the missing-file path (assert exit 2 AND error message includes available-change list).

## 5. `autocoder inspect tool-usage`

- [x] 5.1 Implement `inspect::tool_usage(args)` in `cli/inspect.rs`:
  - Resolve workspace basename per the helper.
  - Resolve stream-log path (same as `inspect log`).
  - Parse the stream log; aggregate stats:
    - Duration: from the first `[tool_use]` timestamp to the last `[tool_result]` (or `[assistant]`) timestamp.
    - Tool call counts: per tool name.
    - For `query_canonical_specs` specifically: total bytes returned across all calls, hit count per call, score distribution buckets (`high >= 0.7`, `medium 0.5–0.7`, `low < 0.5`), avg hits per call.
  - Render per the proposal's example.
  - On `--json`: print the stats as a structured object.
- [x] 5.2 Integration-test against a fixture stream log with known counts (assert each stat matches expected).

## 6. Validation

- [x] 6.1 `cargo test` passes.
- [x] 6.2 `cargo clippy` produces no NEW warnings against the existing baseline.
- [x] 6.3 `openspec validate a36-autocoder-inspect-diagnostic-cli --strict` passes.
- [x] 6.4 Manual sanity check (during dev): `cargo run -- inspect rag --workspace github_com_test_repo --query "test query"` against a live daemon produces a readable table.
