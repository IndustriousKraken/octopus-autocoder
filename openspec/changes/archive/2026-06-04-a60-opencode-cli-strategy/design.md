# Design / Spike findings — a60 opencode CLI strategy

## Spike environment caveat

The implementation sandbox has **no `opencode` binary** (`command -v opencode`
→ not found) and **no outbound web access** (WebFetch/WebSearch are denied).
The spike tasks (1.1–1.4) were therefore resolved from opencode's documented
behavior and config schema rather than by probing a live binary. The strategy
is structured so that the two load-bearing, hardest-to-reverse decisions
(prompt delivery + permission keys) are each isolated to a single function and
can be flipped in one place if a live probe on the daemon host disagrees.

**Operator prerequisite (carried from the proposal):** the `opencode` binary
must be installed on the daemon host for any role configured to use it. A live
re-confirmation of 1.1–1.4 on that host is the recommended pre-production check.

## 1.1 — Prompt delivery (stdin vs positional)

**Decision: stdin.** `opencode run [message..]` accepts a message, and headless
runs read a piped (non-TTY) stdin as the message. The shared `agentic_run`
primitive already pipes the prompt to the child's stdin (the same mechanism the
`claude` strategy relies on), so `OpencodeStrategy::build_command` appends **no**
positional message. This also avoids `ARG_MAX` (E2BIG) for large review prompts
(diffs + context), which a positional argument would risk. `BuildContext` does
not carry the prompt, so positional delivery would require widening the trait —
another reason stdin is the right seam. If a live probe shows opencode requires
a positional message, the change is localized to `build_command` (+ threading
the prompt through `BuildContext`).

## 1.2 — MCP tool calls under headless `opencode run`

opencode supports MCP servers declared in `opencode.json` under the `mcp` block
with `type: "local"`, a `command` array, an `environment` map, and `enabled`.
The strategy writes the same per-execution MCP child the `claude` path uses
(`<autocoder-binary> mcp-ask-user-server`) with `ORCH_MCP_WORKSPACE` /
`ORCH_MCP_CHANGE` / `ORCH_MCP_ROLE` in `environment`, so the role's `submit_*`
tool is advertised to the model exactly as it is for claude. Confirmed by docs;
re-confirm on the daemon host that the model actually *invokes* the tool in a
non-interactive `run`.

## 1.3 — Correctable tool errors (schema-reject → retry in session)

The submission contract is daemon-side: `record_submission` validates the
payload and returns a JSON-RPC tool error on schema mismatch. That error is
returned through the MCP `tools/call` response, which opencode surfaces to the
model as a tool result the model can read and correct within the same `run`
session — the same correctable-tool-error loop a56 requires of the claude path.
This is a property of MCP `tools/call` error semantics (shared by both CLIs),
not of opencode specifically. Re-confirm on the daemon host.

## 1.4 — Read-only sandbox via opencode permissions

opencode's `permission` block gates tool classes with `allow` / `ask` / `deny`.
The strategy maps a56's allowed-tools list onto it: `edit` (governs opencode's
file-mutating `write`+`edit` tools), `bash`, and `webfetch` are set to `allow`
when the equivalent tool is in the allowed list, else `deny`. A read-only role
(`["Read","Glob","Grep"]`) therefore denies `edit`, `bash`, and `webfetch`; the
always-available read tools stay usable and the role's `submit_*` tool is
exposed via the `mcp` block. (Belt-and-suspenders post-hoc write-revert, as
planned for the gemini sibling a69, is out of scope here.)

## 1.5 — Blocker check

Tasks 1.2–1.4 are all satisfiable under opencode's documented config schema, so
no blocker is reported. The only residual risk is the doc-vs-live gap noted
above (binary + web unavailable in the implementation sandbox); it is contained
to two single-function seams (prompt delivery, permission keys) and the operator
host re-check.

## Capture-mode only (2.5)

`OpencodeStrategy` runs through `agentic_run` in `OutputMode::Capture`
(stdout/stderr at exit). The streaming-JSON event path (`final_answer` /
`session_id` / incremental structured log) stays claude-specific, so opencode
serves the capture-mode structured-submission roles (advisory audits, reviewer,
contradiction check); the executor's streaming implementer path remains on
`claude`.

## Integration seam (not wired here, by design)

The strategy reads `BuildContext::{workspace, mcp_role, model}`. In production,
`agentic_run` populates `workspace`/`model` from its opts and leaves `mcp_role`
`None`: the submission roles (a58 reviewer, a59 contradiction check) still write
their own `.mcp.json` via `write_mcp_config` and this change does **not** modify
them. Wiring those call sites to pass the role through to the opencode
`opencode.json` writer (and to skip `.mcp.json` when on opencode) is the
follow-up those roles make when they opt into opencode end-to-end. This change
registers the strategy, builds the invocation, and exposes + unit-tests the
seam.
