## ADDED Requirements

### Requirement: MCP server exposes `query_canonical_specs` tool to the implementer
The daemon's MCP server SHALL register a `query_canonical_specs` tool alongside the existing `ask_user` tool. The tool is available to the wrapped agent (Claude CLI by default) AND lets the implementer retrieve the most-relevant canonical-spec chunks for any query string. The tool's surface:

- Name: `query_canonical_specs`.
- Input schema: `{ query: string, top_k?: number }`. `query` is required. `top_k` defaults to `canonical_rag.top_k` from config (default 10), clamped per the orchestrator spec.
- Output: a JSON array of objects shaped `{ capability: string, requirement_title: string, requirement_body: string, scenario_titles: string[], relevance_score: number }`, sorted by descending `relevance_score`.
- Cost: free to the agent; the daemon owns the embedding pipeline AND amortizes its cost across iterations.

When RAG is disabled OR the workspace's store is absent (init failed; provider unreachable), the tool SHALL return an empty array PLUS a structured `error_hint` field naming the cause (`"rag disabled in config"` OR `"rag init failed; see daemon log"`). The agent's prompt is updated (in the implementer prompt template) to mention the tool's existence AND to encourage its use when working on a capability with a canonical spec.

The tool routes via the existing MCP transport; no new transport surface is introduced. The MCP server's per-workspace context resolution (the same logic that routes `ask_user` calls to the correct workspace's marker file) routes `query_canonical_specs` calls to the correct workspace's `CanonicalRagStore`.

#### Scenario: Tool registered alongside `ask_user`
- **WHEN** the daemon's MCP server starts up
- **THEN** the server's tool registry contains BOTH `ask_user` (existing) AND `query_canonical_specs` (new)
- **AND** the per-workspace `.mcp.json` file the daemon writes for the wrapped agent lists both tools
- **AND** the agent's discovery of available tools returns the documented input/output schemas

#### Scenario: Tool returns ranked chunks for a workspace with embeds
- **WHEN** an agent invokes `query_canonical_specs({ query: "audit framework cadence", top_k: 5 })`
- **AND** the workspace's `CanonicalRagStore` exists AND contains audit-framework-related embeds
- **THEN** the tool returns up to 5 results sorted by descending `relevance_score`
- **AND** each result has the documented fields (capability, requirement_title, requirement_body, scenario_titles, relevance_score)
- **AND** the requirement_body contains enough text for the agent to act on (not just the title)

#### Scenario: Tool returns empty array when RAG is disabled
- **WHEN** the workspace's config has no `canonical_rag:` block (RAG disabled)
- **AND** an agent invokes `query_canonical_specs({ query: "..." })`
- **THEN** the tool returns an empty array AND a structured `error_hint: "rag disabled in config"` field at the response root
- **AND** the agent's MCP client surfaces the hint so the implementer's prompt can fall back to its non-RAG behavior

#### Scenario: Tool returns empty array when init failed
- **WHEN** RAG is configured but workspace-init's embed call failed earlier (provider unreachable)
- **AND** an agent invokes `query_canonical_specs({ query: "..." })`
- **THEN** the tool returns an empty array AND `error_hint: "rag init failed; see daemon log"`
- **AND** the daemon log contains the original failure's WARN line for operator diagnosis

#### Scenario: Tool respects per-workspace store isolation
- **WHEN** two workspaces are managed by the same daemon AND both have `CanonicalRagStore` registered
- **AND** agent A (for workspace 1) invokes `query_canonical_specs(...)`
- **THEN** the tool's results come ONLY from workspace 1's store
- **AND** workspace 2's store entries do NOT appear in the response
- **AND** the routing uses the same per-workspace context resolution the existing `ask_user` tool uses

#### Scenario: Default `top_k` from config when omitted
- **WHEN** an agent invokes `query_canonical_specs({ query: "..." })` with NO `top_k` argument
- **AND** `canonical_rag.top_k` is set to `15`
- **THEN** the tool returns up to 15 results
- **AND** the agent's explicit `top_k` (when present) overrides the config default

#### Scenario: Implementer prompt mentions the tool
- **WHEN** the daemon assembles the implementer prompt for an executor invocation
- **AND** the embedded `prompts/implementer.md` (OR an operator override) is loaded
- **THEN** the prompt contains a paragraph naming `query_canonical_specs` AND its purpose (retrieve canonical-spec chunks for the change's capability context)
- **AND** the operator MAY override the prompt template to remove the mention if they prefer the agent not call the tool — the tool stays registered regardless, just unused
