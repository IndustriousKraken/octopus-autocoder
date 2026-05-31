## ADDED Requirements

### Requirement: `autocoder inspect` subcommand surface for operator diagnostics

The `autocoder` CLI SHALL expose an `inspect` subcommand with three subsubcommands that wrap existing diagnostic data sources in operator-friendly forms. Each subsubcommand exits `0` on success, `2` on operator error (missing arg, unresolvable workspace, unreachable socket, missing log file), AND `1` on internal error (parse failure, IO error, etc.).

All three subsubcommands accept a `--workspace <basename-or-url>` argument. The argument SHALL be resolved as follows:

1. When the argument contains `:` OR starts with `http`/`https`: parsed as a git URL, sanitized to a basename via the existing workspace-basename-sanitization helper.
2. Otherwise: used as a basename verbatim.
3. When omitted AND the daemon's config has exactly one repository: that repository's basename is used.
4. When omitted AND the config has zero OR multiple repositories: the subcommand prints the list of available basenames AND exits `2`.

The workspace resolution rule is uniform across all three subsubcommands.

#### Subsubcommand: `autocoder inspect rag`

`autocoder inspect rag --workspace <basename-or-url> --query "<text>" [--top-k N] [--show-bodies] [--json]` SHALL:

1. Resolve the workspace basename per the rule above.
2. Resolve the control-socket path via `DaemonPaths.control_socket_path()`.
3. Connect via `UnixStream::connect`. On failure: print `error: control socket unreachable at <path>: <error>. Is the daemon running? (systemctl status autocoder)` to stderr AND exit `2`.
4. Send `{"action":"query_canonical_specs","workspace_basename":"<basename>","query":"<query>","top_k":<N>}` (defaulting `top_k` to the daemon's configured `canonical_rag.top_k` when the flag is omitted) on a single line followed by `\n`.
5. Read the single-line JSON response.
6. When `--json` is set: print the raw response to stdout AND exit `0`.
7. When `--json` is NOT set: render a table with columns SCORE, CAPABILITY, REQUIREMENT, BYTES (one row per hit, sorted by descending score), preceded by header lines naming the query, workspace, AND top_k. Below the table, print a one-line summary: `Total response size: <KB> KB (N hits)`.
8. When `--show-bodies` is set: after the table, render one section per hit with `## <capability>/<requirement_title>` followed by the first 500 characters of `requirement_body`.

#### Subsubcommand: `autocoder inspect log`

`autocoder inspect log --workspace <basename-or-url> <change> [--limit N] [--json]` SHALL:

1. Resolve the workspace basename per the rule above.
2. Resolve the stream-log path: `<logs_dir>/runs/<basename>/<change>.stream.log` via `DaemonPaths`.
3. On file-not-found: enumerate `*.stream.log` files in `<logs_dir>/runs/<basename>/`, print `error: no stream log at <path>. Available changes in this workspace: <comma-separated list>` to stderr, AND exit `2`.
4. Parse the stream log: each line is a `[tool_use] ...`, `[tool_result] ...`, OR `[assistant] ...` event with a timestamp prefix.
5. Group `tool_use` events with their matching `tool_result` events (by `tool_use_id` field if present, else by source-order positional pairing).
6. Render a header naming the change, workspace, summary-log path, AND stream-log path.
7. Render at most `--limit N` (default `30`) tool-call event groups, each formatted as `[timestamp] tool_use <name> <input-summary>` followed by `[timestamp] tool_result (<bytes-or-summary>)`. `--limit 0` means unlimited.
8. For `tool_use query_canonical_specs`: the input summary SHALL include the query text AND top_k. The matching `tool_result` summary SHALL include the hit count AND top relevance score.
9. After the tool-call section, render the FINAL ANSWER content from the summary `.log` file (the FINAL ANSWER section that a20a2 split out of the stream log).
10. When `--json` is set: print the parsed event stream as a JSON array AND skip the formatted-rendering AND FINAL ANSWER sections.

#### Subsubcommand: `autocoder inspect tool-usage`

`autocoder inspect tool-usage --workspace <basename-or-url> <change> [--json]` SHALL:

1. Resolve workspace basename AND stream-log path per the same rules as `inspect log`.
2. On file-not-found: same behavior as `inspect log` (error message + exit `2`).
3. Parse the stream log AND aggregate:
   - Duration: from the first event's timestamp to the last event's timestamp.
   - Tool-call counts grouped by tool name.
   - For `query_canonical_specs` calls specifically: total bytes returned (sum of tool_result content sizes), total hits returned (sum across calls), score distribution buckets (`high >= 0.7`, `medium 0.5–0.7`, `low < 0.5`), AND avg hits per call.
4. Render the aggregated stats per the canonical format (named sections for duration, tool calls, AND query_canonical_specs detail when present).
5. When `--json` is set: print the aggregated stats as a structured object AND skip the formatted rendering.

#### Scenario: `autocoder inspect rag` queries the live RAG store
- **WHEN** the operator runs `autocoder inspect rag --workspace github_com_foo_bar --query "audit framework cadence" --top-k 5` against a running daemon
- **THEN** the command connects to the control socket, sends the `query_canonical_specs` action, AND prints a table with one row per hit
- **AND** the table includes the SCORE, CAPABILITY, REQUIREMENT, AND BYTES columns
- **AND** the exit code is 0

#### Scenario: `autocoder inspect rag --json` prints raw response
- **WHEN** the operator passes `--json`
- **THEN** the command prints the raw control-socket response JSON to stdout
- **AND** no formatted table is rendered

#### Scenario: Unreachable control socket produces clear error
- **WHEN** `autocoder inspect rag` runs AND the control socket path does NOT exist (daemon not running OR socket path mismatch)
- **THEN** stderr contains `error: control socket unreachable at <resolved-path>: <error>. Is the daemon running? (systemctl status autocoder)`
- **AND** the exit code is 2

#### Scenario: Workspace omitted with single configured repo auto-selects
- **WHEN** `autocoder inspect rag --query "x"` runs (no `--workspace` flag) AND the daemon's config has exactly one repository
- **THEN** that repository's basename is used automatically
- **AND** the command proceeds without operator prompt

#### Scenario: Workspace omitted with multiple repos exits with list
- **WHEN** `autocoder inspect rag --query "x"` runs AND the daemon's config has more than one repository
- **THEN** stderr contains `error: --workspace required. Available basenames: <comma-separated list>`
- **AND** the exit code is 2

#### Scenario: `autocoder inspect log` renders tool-call-grouped output
- **WHEN** the operator runs `autocoder inspect log --workspace github_com_foo_bar a30-baz`
- **THEN** the command reads `<logs_dir>/runs/github_com_foo_bar/a30-baz.stream.log` AND renders a header followed by tool-call event groups
- **AND** each `query_canonical_specs` tool_use's matching tool_result shows hit count AND top score
- **AND** after the tool-call section, the FINAL ANSWER from the summary log is appended

#### Scenario: `autocoder inspect log` with missing file lists available changes
- **WHEN** the operator runs `autocoder inspect log --workspace github_com_foo_bar nonexistent-change`
- **THEN** stderr contains `error: no stream log at <path>. Available changes in this workspace: <list>`
- **AND** the listed names are the basenames (without `.stream.log` suffix) of files in `<logs_dir>/runs/github_com_foo_bar/`
- **AND** the exit code is 2

#### Scenario: `autocoder inspect tool-usage` produces aggregated stats
- **WHEN** the operator runs `autocoder inspect tool-usage --workspace github_com_foo_bar a30-baz`
- **THEN** the output includes a `duration:` line, a `tool calls:` section with per-tool counts, AND when `query_canonical_specs` calls are present, a `query_canonical_specs detail:` section with bytes returned, score distribution, AND avg hits per call

#### Scenario: URL-form workspace argument sanitizes correctly
- **WHEN** any `inspect` subsubcommand runs with `--workspace git@github.com:foo/bar.git`
- **THEN** the URL is sanitized to `github_com_foo_bar` via the existing workspace-basename-sanitization helper
- **AND** the resulting basename is used for control-socket queries OR log-path resolution

#### Scenario: `inspect rag` reads RAG-store data the daemon owns
- **WHEN** the operator runs `inspect rag` AND the daemon has a `CanonicalRagStore` registered for the workspace
- **THEN** the response's `hits` array reflects the store's actual content at that instant
- **AND** the subcommand does NOT load OR query embeddings independently (the daemon is the single source of truth)
