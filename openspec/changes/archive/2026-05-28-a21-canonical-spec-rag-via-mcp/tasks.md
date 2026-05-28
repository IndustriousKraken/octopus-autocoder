## 1. Config schema

- [x] 1.1 In `autocoder/src/config.rs`, add top-level `Config.canonical_rag: Option<CanonicalRagConfig>`. `None` means the feature is disabled; `Some` with `enabled: false` is also disabled (explicit opt-out preserves the config block for documentation purposes).
- [x] 1.2 Define `CanonicalRagConfig`:
  ```rust
  #[derive(Deserialize, Serialize, Debug, Clone)]
  pub struct CanonicalRagConfig {
      #[serde(default)]
      pub enabled: bool,
      pub provider: RagProvider,                 // ollama | openai_compatible
      pub model: String,
      pub api_base_url: String,
      pub api_key_env: Option<String>,
      pub api_key: Option<SecretSource>,         // inline alternative
      #[serde(default = "default_top_k")]
      pub top_k: usize,                          // 10
      #[serde(default)]
      pub chunk_strategy: ChunkStrategy,         // per_requirement (default), per_scenario, per_capability
      #[serde(default = "default_reembed_on_archive")]
      pub reembed_on_archive: bool,              // true
  }
  #[derive(Deserialize, Serialize, Debug, Copy, Clone, PartialEq, Eq)]
  #[serde(rename_all = "snake_case")]
  pub enum RagProvider { Ollama, OpenaiCompatible }
  #[derive(Deserialize, Serialize, Debug, Copy, Clone, Default, PartialEq, Eq)]
  #[serde(rename_all = "snake_case")]
  pub enum ChunkStrategy {
      #[default]
      PerRequirement,
      PerScenario,
      PerCapability,
  }
  ```
- [x] 1.3 Clamps + defaults: `top_k` clamps at `[1, 100]` with WARN. `api_key` AND `api_key_env` mutually exclusive (same pattern as `reviewer:` — inline wins with WARN if both set).
- [x] 1.4 Update `config.example.yaml` AND the project-documentation config-coverage test list.
- [x] 1.5 Tests: default `None` parses; explicit block with all fields parses; missing required field (e.g. `provider`) produces a clear error.

## 2. Embedding provider adapters

- [x] 2.1 New module `autocoder/src/rag/embedding.rs`. Define:
  ```rust
  pub trait EmbedClient: Send + Sync {
      async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
      async fn embed_one(&self, text: &str) -> Result<Vec<f32>>;
  }
  pub fn build_client(config: &CanonicalRagConfig) -> Result<Arc<dyn EmbedClient>>;
  ```
- [x] 2.2 Ollama adapter at `autocoder/src/rag/embedding/ollama.rs`:
  - POST to `<base_url>/api/embed` with `{"model": "<model>", "input": ["text1", "text2"]}`.
  - Parse the response per Ollama's embedding API.
  - Batch up to 32 inputs per request (configurable later via `extra.batch_size`).
- [x] 2.3 OpenAI-compatible adapter at `autocoder/src/rag/embedding/openai_compatible.rs`:
  - POST to `<base_url>/embeddings` with `{"model": "<model>", "input": ["text1", "text2"]}`.
  - Set `Authorization: Bearer <api_key>` header from the resolved secret.
  - Parse the OpenAI embeddings response format.
- [x] 2.4 Add `rig-core` dependency. Check current version at implementation time per `check-current-versions-not-training`. Rig's embedding builders MAY supersede our handwritten adapters; if so, prefer Rig's surface AND wire our trait through it.
- [x] 2.5 Tests: both adapters against mocked HTTP responses; batch handling; error propagation (4xx, 5xx, network).

## 3. Markdown-aware chunking

- [x] 3.1 New module `autocoder/src/rag/chunking.rs`. Define:
  ```rust
  pub struct ChunkInput {
      pub source_path: PathBuf,
      pub capability: String,
      pub requirement_title: String,
      pub scenario_titles: Vec<String>,
      pub text: String,                 // The chunk's text body for embedding.
  }
  pub fn chunk_canonical_spec(spec_path: &Path, strategy: ChunkStrategy) -> Result<Vec<ChunkInput>>;
  ```
- [x] 3.2 `PerRequirement` strategy: split on `### Requirement:` headers. Each chunk = the requirement's SHALL paragraph + every `#### Scenario:` block under it. The chunk's `text` includes the requirement title (so embeddings capture title semantics) + body. `requirement_title` is set; `scenario_titles` enumerates scenario headings.
- [x] 3.3 `PerScenario` strategy (future; spec but don't implement in this change): finer chunking; one chunk per scenario. Tests can validate the contract; full implementation can ship in a follow-up.
- [x] 3.4 `PerCapability` strategy (future; spec but don't implement): coarsest; one chunk per spec file. Tests validate the contract.
- [x] 3.5 Tests:
  - PerRequirement chunker against a fixture canonical spec → returns the expected number of chunks with the right titles AND scenario lists.
  - Spec with no `### Requirement:` headers (rare; malformed) → returns empty Vec with a WARN.
  - Spec with a `### Requirement:` heading but no body → returns the chunk with empty body; not skipped (the empty case is still valid).

## 4. RAG store

- [x] 4.1 New module `autocoder/src/rag/mod.rs`. Define:
  ```rust
  pub struct CanonicalRagStore {
      workspace_basename: String,
      provider: Arc<dyn EmbedClient>,
      config: CanonicalRagConfig,
      entries: RwLock<Vec<StoreEntry>>,
  }
  struct StoreEntry {
      input: ChunkInput,
      embedding: Vec<f32>,
  }
  pub struct RagHit {
      pub capability: String,
      pub requirement_title: String,
      pub requirement_body: String,
      pub scenario_titles: Vec<String>,
      pub relevance_score: f32,
  }
  impl CanonicalRagStore {
      pub async fn rebuild_for_workspace(workspace: &Path, config: CanonicalRagConfig) -> Result<Self>;
      pub async fn rebuild_capabilities(&self, workspace: &Path, capabilities: &[String]) -> Result<()>;
      pub async fn query(&self, query: &str, top_k: Option<usize>) -> Result<Vec<RagHit>>;
  }
  ```
- [x] 4.2 `rebuild_for_workspace`: glob `<workspace>/openspec/specs/<cap>/spec.md`; for each, chunk + embed; populate entries.
- [x] 4.3 `rebuild_capabilities`: for each named capability, remove existing entries with that capability slug, re-chunk + re-embed, append.
- [x] 4.4 `query`: embed the query string; compute cosine similarity against every entry; return top-k sorted descending. Cosine similarity is float math over Vec<f32>; no external dep needed.
- [x] 4.5 Per-workspace store registry. The daemon maintains `HashMap<workspace_basename, Arc<CanonicalRagStore>>` so multiple managed repos each have their own corpus.
- [x] 4.6 Tests:
  - Build a store from a fixture workspace → query returns expected hits.
  - Rebuild a single capability → other capabilities' entries are unaffected.
  - Empty workspace (no `openspec/specs/`) → store has zero entries; queries return empty Vec.

## 5. Workspace-init AND post-archive cadence integration

- [x] 5.1 In the polling loop's workspace-init step (after `recreate_branch` succeeds for the first iteration of a workspace), check `config.canonical_rag` AND if `Some(c)` with `c.enabled == true`:
  - Build the embed client.
  - Call `CanonicalRagStore::rebuild_for_workspace`.
  - Register the store in the per-workspace registry.
  - On error: log WARN, omit the store from the registry (subsequent queries return empty Vec).
- [x] 5.2 In the polling loop's post-archive step (after an iteration's commits land):
  - If `c.reembed_on_archive == true` AND the archive touched any `openspec/specs/<cap>/spec.md` files:
    - Determine the affected capabilities via `git diff --name-only HEAD~N HEAD -- openspec/specs/` where N is the number of newly-archived commits.
    - Call `store.rebuild_capabilities(workspace, &affected_caps)`.
    - On error: log WARN, leave existing embeds in place (stale-but-usable).
- [x] 5.3 Tests:
  - Iteration A: workspace init → store rebuilt; query works.
  - Iteration B: archive includes a canonical spec change → re-embed of affected capability.
  - Iteration C: archive with no canonical changes → no re-embed.
  - Iteration D: re-embed fails → existing embeds retained; WARN logged.

## 6. Control-socket action + per-execution MCP child relay

- [x] 6.1 In `autocoder/src/control_socket.rs`, add a `query_canonical_specs` action handler. Request shape:
  ```json
  {"action":"query_canonical_specs","workspace_basename":"<sanitized-basename>","query":"<text>","top_k":10}
  ```
  Handler behavior:
  - Look up `workspace_basename` in the daemon's `HashMap<String, Arc<CanonicalRagStore>>` registry.
  - If absent (RAG disabled, init failed, OR no workspace by that basename): return `{"ok":true,"hits":[],"error_hint":"rag disabled in config" | "rag init failed; see daemon log" | "no workspace registered for that basename"}`.
  - If present: call `store.query(query, top_k)`; return `{"ok":true,"hits":[<RagHit JSON>...]}`.
  - On query error (provider unreachable mid-query, etc.): return `{"ok":true,"hits":[],"error_hint":"query failed: <reason>"}` (fail-open, matching the canonical RAG failure posture).
- [x] 6.2 In `autocoder/src/executor/claude_cli.rs::write_mcp_config`, add two env vars to the MCP child's spawn environment:
  - `ORCH_DAEMON_CONTROL_SOCKET` — value from `DaemonPaths.control_socket_path()`. Required when `canonical_rag` is enabled in the per-repo config; absent (env var not set at all) otherwise.
  - `ORCH_MCP_WORKSPACE_BASENAME` — the sanitized basename the daemon uses for `CanonicalRagStore` registry keys.
- [x] 6.3 In the existing stdio MCP child (`autocoder/src/mcp_askuser_server.rs`, OR a renamed sibling `autocoder/src/mcp_server.rs` if the rename is preferable for clarity):
  - Extend the `tools/list` response to advertise `query_canonical_specs` alongside `ask_user`, with the documented input schema (`{ query: string, top_k?: number }`).
  - When `tools/call` receives `query_canonical_specs`:
    - If `ORCH_DAEMON_CONTROL_SOCKET` env var is not set, return a tool result `{"hits":[],"error_hint":"rag not configured for this execution"}`.
    - Otherwise, connect to the control socket; send a single line of JSON per the request shape above; read the single-line JSON response; return the response's `hits` array (plus `error_hint` if present) to the agent as the tool-call result.
  - Connection timeout: 10 seconds. On timeout OR socket error: return `{"hits":[],"error_hint":"control socket unreachable: <error>"}`.
- [x] 6.4 Tests:
  - Control-socket action: a daemon with a fixture `CanonicalRagStore` returns the expected JSON for a known query; missing-store case returns the expected `error_hint`.
  - MCP child relay: a stdio session against a mocked control socket returns the expected tool result.
  - Env-var absent case: tool returns `error_hint: "rag not configured for this execution"`.
  - Round-trip integration: a daemon + a real stdio MCP child + a mocked embed-provider — invoking `query_canonical_specs` returns ranked chunks.

## 7. Implementer prompt update

- [x] 7.1 Edit `prompts/implementer.md` to add the canonical-RAG paragraph from the proposal. Place it in the "tools you have access to" section if one exists, or near the top of the file.
- [x] 7.2 Embedded prompt re-loads at runtime via `include_str!` (existing pattern); no spec-side schema change.
- [x] 7.3 Operators with custom implementer prompt overrides receive no automatic update — `docs/CONFIG.md`'s `executor.implementer_prompt_path` documentation gains a note about the new tool's availability so override-users can update their templates.

## 8. Install-wizard graduated-path flow

- [x] 8.1 Extend `autocoder/src/cli/install.rs` with the graduated-path RAG prompt sequence per the proposal.
- [x] 8.2 Detection logic:
  - Ollama on localhost: HTTP GET `http://localhost:11434/api/tags` with a 2-second timeout. 200 OK = Ollama present.
  - Remote Ollama: HTTP GET `<operator-provided base_url>/api/tags`.
  - OpenAI-compatible: send a tiny embed request to verify the endpoint AND credentials work.
- [x] 8.3 Branch logic:
  - Localhost detected → suggest using it; operator confirms model name (default `nomic-embed-text` or `qwen3-embedding:4b`).
  - Localhost not detected → present the four-option menu.
  - Option 1 (docker): copy `install/ollama-docker-compose.yml` to `<config_dir>/ollama-docker-compose.yml`; print the `docker compose -f <path> up -d` command; the wizard does NOT auto-run docker.
  - Option 2 (remote ollama): prompt for `base_url` + model; probe; write config block.
  - Option 3 (openai-compatible): prompt for `base_url` + model + api_key source; probe; write config block.
  - Option 4 (disable): write no `canonical_rag:` block; print a one-liner about re-enabling later via config edit.
- [x] 8.4 Non-interactive mode: respect new flags `--rag-provider <ollama|openai_compatible|none>`, `--rag-base-url <url>`, `--rag-model <model>`, `--rag-api-key-env <name>`. Validation matches the interactive probe behavior.
- [x] 8.5 Tests: wizard branches exercised via `ScriptedIo` + mocked HTTP responses.

## 9. Bundled docker-compose

- [x] 9.1 Create `install/ollama-docker-compose.yml` per the proposal.
- [x] 9.2 The file pulls `nomic-embed-text` as the default-startup model (cheaper than qwen3-embedding:4b; works on CPU; reasonable quality for the docker-quick-start use case). Operators upgrading hardware can edit the entrypoint to pull a bigger model.
- [x] 9.3 The compose file is in-tree but NOT embedded into the binary — `autocoder install` copies it to the operator's config dir at install time. Stays editable post-install.
- [ ] 9.4 Smoke test (manual): on a host with docker installed, `docker compose -f install/ollama-docker-compose.yml up -d`; wait 30 seconds; `curl http://localhost:11434/api/tags`; confirm `nomic-embed-text` is listed.

## 10. Docs

- [x] 10.1 In `docs/CONFIG.md`, add a `## \`canonical_rag:\` (optional)` section documenting every field AND its default. Include a note that `canonical_rag.api_key_env` AND `canonical_rag.api_key` are mutually exclusive (same pattern as `reviewer:`).
- [x] 10.2 In `docs/OPERATIONS.md`, add a "Canonical-spec RAG" section describing:
  - When re-embeds fire (workspace init + post-archive touching canonical).
  - Per-workspace in-memory store (no on-disk persistence; daemon restart re-embeds).
  - Failure modes (provider error → WARN + RAG disabled for the workspace's lifetime; per-query error → empty Vec).
  - Cost expectations (sub-second on GPU; ~30s on CPU for typical corpus; once-per-archive thereafter).
- [x] 10.3 In `docs/CHATOPS.md`, add a one-line note in the implementer-flow section that the agent may call `query_canonical_specs` as part of its work; results show in the per-change run log.
- [x] 10.4 In `docs/DEPLOYMENT.md`, add a "Self-hosted Ollama for RAG" subsection covering:
  - The docker-compose quick-start.
  - Pointing at a remote Ollama on a GPU machine (`api_base_url: http://gpu-host:11434`).
  - Hardware suggestions (CPU works; GPU is faster but not required for the corpus size).
- [x] 10.5 In `docs/CONFIG.md`'s `executor.implementer_prompt_path` row, add a note that operators with override templates can mention `query_canonical_specs` in their prompt OR ignore the new tool entirely.

## 11. Spec deltas

- [x] 11.1 `openspec/changes/a21-canonical-spec-rag-via-mcp/specs/orchestrator-cli/spec.md` ADDs: `Canonical-spec RAG configuration AND pipeline`; `RAG re-embed cadence (workspace init AND post-archive)`; `Install-wizard graduated RAG-configuration flow`.
- [x] 11.2 `openspec/changes/a21-canonical-spec-rag-via-mcp/specs/executor/spec.md` ADDs `MCP server exposes \`query_canonical_specs\` tool to the implementer`.
- [x] 11.3 `openspec/changes/a21-canonical-spec-rag-via-mcp/specs/project-documentation/spec.md` ADDs `CONFIG.md, OPERATIONS.md, CHATOPS.md, AND DEPLOYMENT.md document the RAG configuration AND operator workflow`.

## 12. Verification

- [x] 12.1 `cargo test` passes (new + existing).
- [x] 12.2 `openspec validate a21-canonical-spec-rag-via-mcp --strict` passes.
- [x] 12.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
- [ ] 12.4 Manual end-to-end verification:
  - Configure `canonical_rag.provider: ollama` + a reachable Ollama with `nomic-embed-text` AND a fixture managed repo.
  - Start the daemon; observe workspace-init log line `canonical RAG embedded N chunks across M capabilities`.
  - Wait for an iteration; inspect the per-change run log for any `query_canonical_specs` calls the implementer made.
  - Archive a change that touches a canonical spec; observe the post-archive re-embed log line for the affected capability.
