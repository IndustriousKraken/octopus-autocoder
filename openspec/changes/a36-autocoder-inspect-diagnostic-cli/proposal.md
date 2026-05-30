## Why

Operators today have three diagnostic data sources for understanding what their agents are doing AND what canonical context the RAG is feeding them:

1. **Per-change run logs** at `<logs_dir>/runs/<workspace-basename>/<change>.{log,stream.log}`. Plain text. Operators read them with `cat`, `grep`, `awk`.
2. **`journalctl -u autocoder`** for daemon-wide log lines.
3. **The control socket** for live queries against the running daemon's RAG store, queue state, etc. Reachable via `nc -U` + hand-constructed JSON-RPC payloads.

The data is all there. The friction is that operators end up writing shell pipelines like:

```bash
echo '{"action":"query_canonical_specs","workspace_basename":"...","query":"...","top_k":5}' \
  | sudo -u autocoder nc -U /tmp/1001-runtime/autocoder/control.sock \
  | jq -r '.hits[] | "\(.relevance_score|tostring|.[0:5])  \(.capability)/\(.requirement_title)"'
```

…to ask routine questions like "what does the RAG return for this query?" The pipeline is fragile to terminal line-wrapping, sudo prompts, JSON quoting, the operator finding the control socket path, AND knowing the workspace-basename sanitization rules. Per recent ops observation: pasting these one-liners through chat clients OR terminals with wrap-on-space mangles them past usability. The data is accessible; the UX is hostile.

This change adds a first-class diagnostic subcommand surface `autocoder inspect` that wraps the existing primitives in three operator-friendly subcommands. No new behavior in the daemon, no new control-socket actions, no new file formats. Pure surface change that makes the existing data legible from a single binary the operator already has installed.

## What Changes

**New CLI subcommand `autocoder inspect` with three subsubcommands:**

### `autocoder inspect rag --workspace <basename> --query "<text>" [--top-k N]`

Sends a `query_canonical_specs` action to the running daemon's control socket AND prints the result as a human-readable table:

```
Query: outcome tool sentinel parsing
Workspace: github_com_IndustriousKraken_openspec-autocoder
top_k: 5

  SCORE  CAPABILITY            REQUIREMENT                                       BYTES
  0.847  executor              Tool-recorded outcomes take precedence...         4123
  0.792  executor              Per-execution MCP child exposes outcome tools...  6201
  0.681  orchestrator-cli      Control socket exposes record_outcome AND...      2944
  0.587  chatops-manager       Operator-initiated re-review posts lifecycle...   3812
  0.412  code-reviewer         No reviewer re-run after a reviewer-initiated...  1873

Total response size: 18.9 KB (5 hits)
```

The `--workspace` argument accepts either a sanitized basename (`github_com_owner_repo`) OR a full repo URL (`git@github.com:owner/repo.git`). When a URL is provided, the subcommand sanitizes it AND uses the resulting basename. When omitted AND the daemon has exactly one configured workspace, that workspace is used. When omitted AND multiple workspaces exist, the subcommand exits non-zero AND lists the available basenames.

An optional `--show-bodies` flag adds a section per hit with the first 500 characters of `requirement_body`. An optional `--json` flag suppresses formatting AND prints the raw control-socket response (useful for piping into other tools).

### `autocoder inspect log --workspace <basename> <change>`

Pretty-prints a per-change stream log with tool calls grouped AND query/result pairs aligned. For each `[tool_use]` line, the matching `[tool_result]` (by `tool_use_id`) is rendered immediately after with indentation. Output shape:

```
== run log: a27a0-outcome-tools-replace-stdout-sentinels ==
workspace: github_com_IndustriousKraken_openspec-autocoder
summary log: /home/autocoder/.local/state/autocoder/logs/runs/.../a27a0...log
stream log:  /home/autocoder/.local/state/autocoder/logs/runs/.../a27a0...stream.log

[01:23:47.123]  tool_use   Read              path=autocoder/src/mcp_askuser_server.rs
[01:23:47.401]  tool_result                  (2841 bytes returned)

[01:23:48.012]  tool_use   query_canonical_specs  query="outcome sentinel parsing"  top_k=5
[01:23:48.456]  tool_result                  (18953 bytes; 5 hits; top score 0.847)

[01:23:49.301]  tool_use   Edit              path=autocoder/src/mcp_askuser_server.rs
[01:23:49.512]  tool_result                  (success)

... (truncated; 47 tool calls total. Pass --limit 0 for full output.)

=== FINAL ANSWER ===
<final_answer text from the result event, verbatim>
```

`--limit N` controls how many tool calls to render (default `30`; `0` = unlimited). `--json` prints the parsed event stream verbatim.

### `autocoder inspect tool-usage --workspace <basename> <change>`

Aggregates stats from a stream log:

```
== tool-usage summary: a27a0-outcome-tools-replace-stdout-sentinels ==
duration: 14m 32s (start 17:08:32, end 17:23:04)

tool calls:
  Read                    18
  Edit                    12
  Bash                     8
  Glob                     4
  query_canonical_specs    5
  Grep                     3

query_canonical_specs detail:
  5 calls
  total bytes returned: 67.4 KB
  total prompt-context loaded: 67.4 KB (no client-side caching)
  score distribution:
    high (>=0.7):   3 hits across 5 calls
    medium (.5-.7): 8 hits across 5 calls
    low (<.5):     11 hits across 5 calls
  avg hits/call: 4.4
```

`--json` prints the underlying stats as a structured object for scripting.

**Workspace resolution.** All three subcommands resolve `--workspace` via the same helper: accept basename OR URL; sanitize URLs to basenames via the existing `crate::workspace::sanitize_url_to_basename`; omit-AND-single-workspace selects the only one; omit-AND-multi-workspace prints the available list AND exits non-zero. Operators get the same UX shape across subcommands.

**Control-socket resolution.** `inspect rag` finds the control socket via the same `DaemonPaths.control_socket_path()` helper the daemon uses to BIND. On socket-not-found OR connection-refused, the subcommand prints a clear error naming the resolved path AND suggests `systemctl status autocoder` to verify the daemon is running.

**Log-file resolution.** `inspect log` AND `inspect tool-usage` find the per-change stream log via `<logs_dir>/runs/<basename>/<change>.stream.log`, also resolved through `DaemonPaths`. On file-not-found, the subcommand prints a clear error naming the resolved path AND lists the change names that DO have logs in the workspace's runs dir (so operators with a typo get an immediate suggestion).

**No new behavior in the daemon.** The subcommands read existing artifacts AND query existing endpoints. No new control-socket actions. No new log-file formats. No config knobs. Zero risk to the daemon's running state.

## Impact

- **Affected specs:**
  - `orchestrator-cli` — ADDED requirement for the `autocoder inspect` subcommand surface AND its three subsubcommands (`rag`, `log`, `tool-usage`).
- **Affected code:**
  - `autocoder/src/cli/mod.rs` — new `Inspect` variant on the `Command` enum with three nested subsubcommand variants.
  - New module `autocoder/src/cli/inspect.rs` (or `autocoder/src/cli/inspect/` if it grows enough to warrant per-subcommand files) hosting the three subcommands' implementations.
  - Reuse existing primitives: `crate::workspace::sanitize_url_to_basename`, `DaemonPaths`, the control-socket relay helper from `mcp_askuser_server.rs`, AND the stream-log line-format parser (small helper, may need extraction).
- **Operator-visible behavior:**
  - `autocoder inspect --help` shows the three subsubcommands AND their flags.
  - The shell-pipeline-with-nc-and-jq workflow becomes a single readable command operators can type without mangling.
  - No changes to existing CLI subcommands (`run`, `check-config`, `install`, `reload`, `sync-specs`, `changelog`, `audit`, `rewind`).
- **Backward compatibility:** purely additive. Existing CLI behavior is byte-identical. Existing scripts that grep stream logs OR `nc` the control socket continue to work unchanged.
- **Dependencies:** none. Independent of every other queued change. Can land in any order.
- **Acceptance:** `cargo test` passes; `openspec validate a36-autocoder-inspect-diagnostic-cli --strict` passes. Tests:
  - `autocoder inspect --help` exits 0 AND lists the three subsubcommands.
  - `autocoder inspect rag --workspace <basename> --query "x"` against a mocked control socket renders the canonical table format AND exits 0.
  - `autocoder inspect rag --workspace <basename> --query "x" --json` prints the raw control-socket response.
  - `autocoder inspect rag` (no `--workspace`) with multiple configured workspaces exits non-zero AND lists the basenames.
  - `autocoder inspect log --workspace <basename> <change>` against a fixture stream log renders the tool-call-grouped format.
  - `autocoder inspect log` against a missing log file exits non-zero AND lists the available change names in the workspace.
  - `autocoder inspect tool-usage --workspace <basename> <change>` against a fixture stream log produces the documented stats output.
  - URL form of `--workspace` (e.g. `--workspace git@github.com:owner/repo.git`) sanitizes to the basename internally.
