## ADDED Requirements

### Requirement: Per-execution MCP child exposes `query_canonical_specs` tool via control-socket relay
The per-execution stdio MCP server (the child process autocoder launches per polling iteration via `.mcp.json`, currently `autocoder/src/mcp_askuser_server.rs`) SHALL advertise a `query_canonical_specs` tool alongside the existing `ask_user` tool. The tool's surface as seen by the wrapped agent:

- Name: `query_canonical_specs`.
- Input schema: `{ query: string, top_k?: number }`. `query` is required. `top_k` defaults to `canonical_rag.top_k` from the daemon's config (default 10), clamped per the orchestrator spec.
- Output: a JSON object `{ hits: Array<RagHit>, error_hint?: string }` where each `RagHit` is shaped `{ capability: string, requirement_title: string, requirement_body: string, scenario_titles: string[], relevance_score: number }`, sorted by descending `relevance_score`.

The tool's handler SHALL NOT compute results locally. Instead it SHALL relay the request to the daemon via the existing control socket (per the canonical `orchestrator-cli` "Control socket for runtime daemon interaction" requirement) using a new `query_canonical_specs` action defined in the orchestrator-cli spec deltas. The daemon owns the `CanonicalRagStore` AND answers via its in-memory state; the MCP child is a thin synchronous relay.

The relay is configured via two env vars set by `ClaudeCliExecutor::write_mcp_config` when launching the MCP child:

- `ORCH_DAEMON_CONTROL_SOCKET` — absolute path to the daemon's Unix-domain control socket. When absent (i.e., RAG is not configured for this workspace), the tool returns `{ hits: [], error_hint: "rag not configured for this execution" }` AND does NOT attempt a socket connection.
- `ORCH_MCP_WORKSPACE_BASENAME` — the sanitized basename the daemon uses as the `CanonicalRagStore` registry key. Routed verbatim into the control-socket request.

Connection timeout: 10 seconds. On timeout OR socket error, the tool returns `{ hits: [], error_hint: "control socket unreachable: <error>" }` AND surfaces the error so the agent can fall back to non-RAG behavior. The control-socket relay is fail-open in every error path; the agent never blocks indefinitely AND never sees a tool-call failure.

The implementer prompt template (`prompts/implementer.md`) SHALL contain a paragraph naming the tool AND describing when to use it (working on a capability with a canonical spec). Operators with custom implementer prompt overrides MAY remove the mention to suppress agent use; the tool stays registered regardless, just unused.

#### Scenario: Tool advertised in the MCP child's `tools/list`
- **WHEN** an agent connects to the MCP child AND sends a `tools/list` request
- **THEN** the response lists BOTH `ask_user` (existing) AND `query_canonical_specs` (new)
- **AND** `query_canonical_specs`'s `inputSchema` matches the documented `{ query: string, top_k?: number }` shape

#### Scenario: Tool returns ranked hits via control-socket relay
- **WHEN** an agent invokes `query_canonical_specs({ query: "audit framework cadence", top_k: 5 })`
- **AND** `ORCH_DAEMON_CONTROL_SOCKET` AND `ORCH_MCP_WORKSPACE_BASENAME` are set in the child's env
- **AND** the daemon has a `CanonicalRagStore` registered for that workspace_basename
- **THEN** the MCP child opens a connection to the socket AND sends `{"action":"query_canonical_specs","workspace_basename":"<basename>","query":"audit framework cadence","top_k":5}`
- **AND** the daemon's handler returns `{"ok":true,"hits":[...]}` with up to 5 results
- **AND** the MCP child returns the `hits` array to the agent as the tool-call result

#### Scenario: RAG not configured — tool returns empty with hint
- **WHEN** the workspace's config has no `canonical_rag:` block (RAG disabled)
- **AND** `ClaudeCliExecutor::write_mcp_config` omits `ORCH_DAEMON_CONTROL_SOCKET` from the spawn env
- **AND** an agent invokes `query_canonical_specs({ query: "..." })`
- **THEN** the tool returns `{ hits: [], error_hint: "rag not configured for this execution" }`
- **AND** no socket connection is attempted

#### Scenario: Control socket unreachable — tool returns empty with hint
- **WHEN** `ORCH_DAEMON_CONTROL_SOCKET` is set BUT the socket is unreachable (file missing, permission denied, daemon down)
- **AND** an agent invokes `query_canonical_specs({ query: "..." })`
- **THEN** the tool returns `{ hits: [], error_hint: "control socket unreachable: <error>" }`
- **AND** the connect attempt times out after 10 seconds at most

#### Scenario: Store missing for workspace — daemon surfaces hint
- **WHEN** RAG is configured BUT workspace-init's embed call failed earlier (provider unreachable)
- **AND** the daemon's `CanonicalRagStore` registry has no entry for the workspace_basename
- **AND** an agent invokes `query_canonical_specs({ query: "..." })`
- **THEN** the daemon's control-socket handler returns `{"ok":true,"hits":[],"error_hint":"rag init failed; see daemon log"}`
- **AND** the MCP child surfaces the hint to the agent
- **AND** the daemon log contains the original failure's WARN line for operator diagnosis

#### Scenario: Per-workspace isolation enforced by daemon
- **WHEN** two workspaces are managed by the same daemon AND both have `CanonicalRagStore` registered
- **AND** an MCP child spawned for workspace 1 (with its `ORCH_MCP_WORKSPACE_BASENAME` env var set to workspace 1's basename) invokes `query_canonical_specs(...)`
- **THEN** the control-socket request carries workspace 1's basename
- **AND** the daemon's handler queries ONLY workspace 1's store
- **AND** workspace 2's entries do NOT appear in the response
- **AND** the routing is enforced by the daemon, not the child (the child cannot accidentally query another workspace's store even if its env var is spoofed — the daemon's handler is the source of truth)

#### Scenario: Default `top_k` from config when omitted
- **WHEN** an agent invokes `query_canonical_specs({ query: "..." })` with NO `top_k` argument
- **AND** `canonical_rag.top_k` is set to `15`
- **THEN** the control-socket request omits `top_k`; the daemon's handler applies the config default
- **AND** the tool returns up to 15 results
- **AND** the agent's explicit `top_k` (when present) overrides the config default

#### Scenario: Implementer prompt mentions the tool
- **WHEN** the daemon assembles the implementer prompt for an executor invocation
- **AND** the embedded `prompts/implementer.md` (OR an operator override) is loaded
- **THEN** the prompt contains a paragraph naming `query_canonical_specs` AND its purpose (retrieve canonical-spec chunks for the change's capability context)
- **AND** the operator MAY override the prompt template to remove the mention if they prefer the agent not call the tool — the tool stays registered in the MCP child regardless, just unused
