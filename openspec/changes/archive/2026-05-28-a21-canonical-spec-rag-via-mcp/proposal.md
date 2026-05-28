## Why

The implementer agent does its work armed with the CHANGE's spec deltas + the codebase it can `Read`. It does NOT see the canonical `openspec/specs/<capability>/spec.md` files unless it explicitly reads them — AND it usually doesn't, because the prompt doesn't direct it AND the files don't surface naturally as part of the implementer's context. The result: changes get implemented in semantic isolation from the existing canonical contract they're supposed to fit within. Common failure modes:

- The implementer introduces a new abstraction that duplicates one already specced for a sibling capability AND nothing surfaces the duplication until a reviewer or auditor catches it later.
- A change references a symbol (function, type, requirement title) whose canonical contract has constraints the change's deltas didn't restate; the implementer satisfies only the deltas AND silently breaks the canonical contract.
- The change-v-canon contradiction case (`a22`'s eventual scope): a change's ADDED requirement contradicts an existing canonical requirement it doesn't `MODIFY` — undetectable without semantic comparison.

Retrieval-augmented context fixes both classes. The daemon embeds the canonical specs at workspace init + on every successful archive that touched a `openspec/specs/<capability>/spec.md` file. The implementer's MCP surface exposes a `query_canonical_specs(query, top_k?)` tool. The implementer is encouraged (but not required) to call it when working on a capability whose canonical contract matters. The result is bounded, in-memory, fast, AND can also feed `a22`'s change-v-canon contradiction check without re-running the embed pipeline.

The embedding-provider surface mirrors the existing `reviewer:` config block — provider, base URL, model, API key (env or inline). Operators with Ollama on localhost / a remote GPU machine / an OpenAI-compatible endpoint (OpenRouter, Voyage, Anthropic, etc.) configure once AND get RAG. The daemon's install wizard offers a graduated path: detect Ollama on localhost → offer to install Ollama via a bundled docker-compose → fall through to remote-provider configuration → offer to disable RAG entirely.

## What Changes

**New `canonical_rag:` config block.** Top-level, optional. Absent block disables RAG entirely (the daemon behaves as today). Present block enables the embedding pipeline AND the MCP tool surface.

```yaml
canonical_rag:
  enabled: true                          # explicit toggle; absent block = disabled regardless of other fields
  provider: ollama                       # | openai_compatible
  model: qwen3-embedding:4b              # provider-specific model identifier
  api_base_url: http://gpu-host:11434    # for ollama: include /api implicit; for openai_compatible: include the /v1 prefix
  api_key_env: null                      # required for openai_compatible; ignored for ollama unless the endpoint requires auth
  # api_key: { value: "..." }            # inline alternative, parallel to reviewer.api_key
  top_k: 10                              # default for tool queries when caller doesn't specify
  chunk_strategy: per_requirement        # default; future variants per_scenario | per_capability
  reembed_on_archive: true               # default true: re-embed any capability whose spec changed after archive
```

**Embedding pipeline.** New module `autocoder/src/rag/mod.rs` integrates the `rig` crate. At workspace init (after `recreate_branch`), the daemon:

1. Reads every `<workspace>/openspec/specs/<capability>/spec.md` file.
2. Parses each into chunks per `chunk_strategy` (default `per_requirement`: heading + body + scenarios as one chunk; future variants follow the same surface).
3. Embeds each chunk via the configured provider.
4. Stores `(chunk, embedding, source_path, capability, requirement_title)` tuples in an in-memory store keyed by `<workspace_basename>`.
5. The store persists across iterations within the daemon's lifetime; daemon restart re-embeds from scratch (fast, ≤1 second on GPU, ~30 seconds on CPU for typical corpora).

**Re-embed cadence.** Two triggers:

- **Workspace init** (first iteration after daemon start, OR after a workspace wipe). Always re-embeds from scratch.
- **Post-archive** (any iteration's archive step that modifies at least one `<workspace>/openspec/specs/<cap>/spec.md` file). Re-embeds ONLY the affected capabilities, not the entire corpus. Detection: `git diff --name-only HEAD~1 HEAD -- openspec/specs/`.

The cadence is deliberately conservative — re-embedding on every iteration would waste cycles when canonical is unchanged. Re-embedding only on archive matches the only operation that updates canonical.

**MCP tool: `query_canonical_specs` — control-socket relay.** autocoder's MCP architecture is per-execution stdio child processes, NOT a long-running daemon MCP server. Each polling iteration launches a fresh `autocoder mcp-server` child whose lifetime is bounded by the wrapped agent's execution; the child has no access to the daemon's in-memory state. To make `query_canonical_specs` work in this architecture, the stdio MCP child SHALL relay queries to the daemon via the existing Unix-domain control socket (`<system-temp>/autocoder/control/control.sock`, per canonical `orchestrator-cli` "Control socket for runtime daemon interaction"). The daemon's control-socket handler answers via the in-memory `CanonicalRagStore`. Tool surface (as seen by the agent):

```
query_canonical_specs(query: string, top_k?: number) -> Array<{
    capability: string,
    requirement_title: string,
    requirement_body: string,
    scenario_titles: string[],
    relevance_score: number,
}>
```

Default `top_k` from config (default 10). The tool returns chunks ranked by cosine similarity. The MCP child blocks on the control-socket round trip for one tool call's duration (typically tens of milliseconds — embedding the query + cosine similarity across ≤O(1000) chunks).

**Per-execution MCP child plumbing.** The existing `ClaudeCliExecutor::write_mcp_config` adds two more env vars to the child's spawn environment alongside `ORCH_MCP_WORKSPACE` AND `ORCH_MCP_CHANGE`:

- `ORCH_DAEMON_CONTROL_SOCKET` — absolute path to the daemon's control socket (resolved from `DaemonPaths.control_socket_path()`).
- `ORCH_MCP_WORKSPACE_BASENAME` — the sanitized basename the daemon uses as the `CanonicalRagStore` registry key.

The MCP child reads these on startup; when the agent invokes `query_canonical_specs`, the child opens a connection to the socket, sends `{"action":"query_canonical_specs","workspace_basename":"...","query":"...","top_k":...}`, reads the single-line JSON response, AND returns the `hits` array to the agent as the tool-call result. The child does NOT cache results across calls; each invocation roundtrips through the socket.

The same MCP child still hosts the existing `ask_user` tool via the marker-file mechanism (unchanged). `query_canonical_specs` adds a parallel tool surface that relays via control socket instead of via marker file because `query_canonical_specs` needs a SYNCHRONOUS response within the same execution (the agent acts on the hits immediately), whereas `ask_user` is fire-and-forget from the child's perspective (the daemon picks up the marker after the agent has paused).

**Implementer prompt update.** The embedded `prompts/implementer.md` gets a new paragraph:

> When you're working on a capability whose canonical contract matters (any capability with a `openspec/specs/<capability>/spec.md`), prefer `query_canonical_specs` over guessing OR over `Read`-ing the entire canonical spec yourself. The tool returns the most-relevant existing requirements for your query, ranked by semantic similarity. Free to call as often as you find useful; the results are bounded AND don't consume your prompt budget the way reading the whole file would.

The prompt addition is content-only (no spec change to the prompt-template-loading requirement). Operators with custom implementer prompts get a doc note about the new tool's availability.

**Install-wizard graduated path.** When the operator runs `autocoder install` (interactive mode), the wizard prompts:

```
Configure canonical-specs RAG? (Y/n)
  > Y
  Detecting Ollama on http://localhost:11434...
  > Not found.
  Options:
    1) Install local Ollama via docker (autocoder ships docker-compose.yml; you run `docker compose up -d`)
    2) Point at a remote Ollama instance (you provide base_url + model)
    3) Point at an OpenAI-compatible endpoint (Voyage, OpenAI, OpenRouter, local llama.cpp server, etc.)
    4) Disable RAG (skip this feature)
  > 2
  Ollama base_url: http://gpu-host:11434
  Embedding model (default qwen3-embedding:4b):
  > [enter]
  Writing canonical_rag block to config.yaml.
```

The detection step probes `<base>/api/tags` on Ollama OR sends a small embed call on OpenAI-compatible endpoints. Failure of the probe in non-interactive mode fails the install with a descriptive error.

**Bundled docker-compose for the quick-start path.** A new `install/ollama-docker-compose.yml` ships in the repo:

```yaml
services:
  ollama:
    image: ollama/ollama:latest
    ports:
      - "11434:11434"
    volumes:
      - ollama_data:/root/.ollama
    # On startup, pull nomic-embed-text (small enough for CPU; works as a default).
    entrypoint: ["/bin/sh", "-c", "ollama serve & sleep 5 && ollama pull nomic-embed-text && wait"]
volumes:
  ollama_data:
```

The wizard's option 1 copies this file to `<config_dir>/ollama-docker-compose.yml` AND prints `docker compose -f <path> up -d` as the operator's next command. The wizard does NOT auto-run `docker compose up` — the operator opts in explicitly.

**Cold start.** On the first iteration of a fresh workspace (no embeds yet), the daemon embeds synchronously before invoking the executor. Subsequent iterations skip if embeds exist AND no canonical files have changed since the last embed. Embeds are in-memory, NOT persisted to disk — daemon restart re-embeds (fast).

**Failure modes.** Embedding-provider errors (network, auth, rate-limit) are non-fatal:
- At workspace init: log WARN naming the error, proceed without RAG. The MCP tool returns an empty array on every call; the implementer falls back to its non-RAG behavior. The daemon continues to retry the embed on subsequent iterations.
- During a runtime query: log WARN, return empty array to the caller.
- The check is fail-open, same posture as `a14`'s transient-failure handling.

**The change-v-canon contradiction check is NOT in this spec.** `a22` (drafted separately) uses this spec's RAG infrastructure for that purpose. `a21` ships the foundation; `a22` ships the application.

## Impact

- **Affected specs:**
  - `orchestrator-cli` — three ADDED requirements: `Canonical-spec RAG configuration AND pipeline`, `RAG re-embed cadence (workspace init AND post-archive)`, `Install-wizard graduated RAG-configuration flow`.
  - `executor` — one ADDED requirement: `MCP server exposes \`query_canonical_specs\` tool to the implementer`.
  - `project-documentation` — one ADDED requirement: `CONFIG.md, OPERATIONS.md, CHATOPS.md, AND DEPLOYMENT.md document the RAG configuration AND operator workflow`.
- **Affected code:**
  - `autocoder/Cargo.toml` — add `rig-core` AND its provider crate(s) as dependencies. Check the current version at draft-implementation time per `check-current-versions-not-training`.
  - `autocoder/src/rag/mod.rs` (new) — embedding pipeline:
    ```rust
    pub struct CanonicalRagStore { ... }
    impl CanonicalRagStore {
        pub async fn rebuild_for_workspace(workspace: &Path, config: &CanonicalRagConfig) -> Result<Self>;
        pub async fn rebuild_capabilities(&mut self, capabilities: &[String]) -> Result<()>;
        pub async fn query(&self, query: &str, top_k: usize) -> Result<Vec<RagHit>>;
    }
    pub struct RagHit { pub capability: String, pub requirement_title: String, pub requirement_body: String, pub scenario_titles: Vec<String>, pub relevance_score: f32 }
    ```
  - `autocoder/src/rag/embedding/{ollama,openai_compatible}.rs` (new) — provider-specific embed call adapters. Both implement a common `EmbedClient` trait.
  - `autocoder/src/rag/chunking.rs` (new) — markdown-aware chunker that splits canonical specs into per-requirement chunks (heading + body + scenarios).
  - `autocoder/src/config.rs` — add `CanonicalRagConfig` struct AND the top-level `canonical_rag:` block.
  - `autocoder/src/polling_loop.rs` — workspace-init hook calls `CanonicalRagStore::rebuild_for_workspace`; post-archive hook checks for canonical changes AND calls `rebuild_capabilities` for affected caps.
  - `autocoder/src/mcp_askuser_server.rs` (rename to `autocoder/src/mcp_server.rs` OR add a sibling) — extend the per-execution stdio MCP child's tool list with `query_canonical_specs`; on tool invocation, open a Unix-domain connection to the daemon's control socket (path from `ORCH_DAEMON_CONTROL_SOCKET` env var), send the `query_canonical_specs` action, return the `hits` array to the agent.
  - `autocoder/src/control_socket.rs` — add a new `query_canonical_specs` action handler. On request, look up `workspace_basename` in the per-workspace `CanonicalRagStore` registry; call `store.query(query, top_k)`; return `{"ok": true, "hits": [...]}` OR `{"ok": false, "error": "..."}`.
  - `autocoder/src/executor/claude_cli.rs::write_mcp_config` — add `ORCH_DAEMON_CONTROL_SOCKET` AND `ORCH_MCP_WORKSPACE_BASENAME` to the spawn env.
  - `autocoder/src/cli/install.rs` — the wizard's graduated-path flow.
  - `prompts/implementer.md` — new paragraph mentioning the tool.
  - `install/ollama-docker-compose.yml` (new) — the bundled quick-start compose file.
  - `docs/CONFIG.md` — document the `canonical_rag:` block.
  - `docs/OPERATIONS.md` — operational discussion (when re-embeds fire, what fails-open looks like, cold-start cost).
  - `docs/CHATOPS.md` — note (in passing) that the implementer may now call `query_canonical_specs` as part of its work.
  - `docs/DEPLOYMENT.md` — the docker-compose option AND how to run a separate Ollama host.
- **Operator-visible behavior:**
  - With RAG enabled: the implementer's PR-body "Agent implementation notes" comment MAY mention canonical-spec queries it made (the implementer's narrative). Per-iteration log files at `<logs>/runs/<workspace>/<change>.log` include the queries AND the chunks returned, alongside the existing prompt + actions + final-answer sections.
  - With RAG disabled (or absent block): no behavior change.
  - Operators inspecting daemon resource use see one additional in-memory data structure per workspace (small; bounded by the canonical spec size).
- **Breaking:** no. Default-off when the block is absent.
- **Acceptance:** `cargo test` passes; `openspec validate a21-canonical-spec-rag-via-mcp --strict` passes. Tests cover:
  - Pipeline: a fixture corpus embeds correctly via a mocked provider; query returns ranked chunks.
  - Cadence: workspace init triggers rebuild; archive touching canonical triggers rebuild-of-affected-caps; archive NOT touching canonical does NOT trigger rebuild.
  - MCP tool: the daemon's MCP server lists the tool; a tool call returns the expected JSON shape.
  - Wizard: the graduated-path flow's branch logic is exercised against a `ScriptedIo` + mocked Ollama-detect.
  - Failure modes: embed-provider error at init → WARN logged, RAG disabled for this workspace's lifetime, tool returns empty array, daemon continues.
- **Dependencies:** Independent of a07-a20 in implementation order (none of them touch the RAG surface). `a22` (the change-v-canon contradiction check) stacks on top of `a21`.
