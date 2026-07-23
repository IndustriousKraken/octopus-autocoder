## MODIFIED Requirements

### Requirement: Canonical-spec RAG configuration and pipeline

autocoder SHALL support a per-workspace retrieval-augmented-context pipeline that embeds the workspace's canonical OpenSpec specs (`openspec/specs/<capability>/spec.md`) into an in-memory vector store AND exposes a retrieval surface for the implementer (via `a21`'s executor MCP requirement) AND for downstream pre-flight checks (`a22`'s change-vs-canon contradiction check). The pipeline is configured via a top-level `canonical_rag:` block in `config.yaml`; an absent block disables the feature entirely. A present block with `enabled: false` also disables; both forms preserve "no behavior change" for operators who don't opt in.

The `canonical_rag:` config block contains: `enabled: bool`, `provider: LlmProvider` (subsystem-valid subset: `ollama | openai_compatible`; `anthropic` is rejected at config-load per the per-subsystem provider-validity requirement), `model: string`, `api_base_url: string` (required for both valid providers), `api_key_env: string?` AND `api_key: SecretSource?` (mutually exclusive — inline wins with WARN if both set; same pattern as `reviewer:`; FORBIDDEN entirely when `provider: ollama` per the per-provider auth-semantics requirement), `top_k: usize` (default `10`, clamped `[1, 100]` with WARN), `chunk_strategy: per_requirement | per_scenario | per_capability` (default `per_requirement`), AND `reembed_on_archive: bool` (default `true`).

The embedding pipeline SHALL:
- Build an `EmbedClient` from the provider config — an Ollama adapter calling `<base_url>/api/embed` for Ollama, OR an OpenAI-compatible adapter calling `<base_url>/embeddings` with `Authorization: Bearer <api_key>` for the openai_compatible path.
- Glob `<workspace>/openspec/specs/<cap>/spec.md` files, chunk each per `chunk_strategy`, embed each chunk via the client, AND store `(chunk, embedding, source_path, capability, requirement_title)` tuples in an in-memory `CanonicalRagStore`.
- Maintain a per-workspace store registry keyed by sanitized workspace basename. Multiple managed repos each have their own store; the stores are independent.
- Persist no STORE state to disk: the vector store AND its registry are in-memory only and rebuilt on workspace-init. The sole on-disk artifact is the embedding-vector cache defined by the "RAG re-embed cadence" requirement (`<cache_dir>/rag-embeddings/`), which caches provider responses so a rebuild avoids re-calling the provider for unchanged chunks — CACHE-category data, safe to delete at any time; its absence merely restores full-provider-cost rebuilds.

Failure modes are fail-open: embedding-provider errors (network, auth, rate-limit) at init log WARN AND omit the workspace's store from the registry. Subsequent queries against the absent store return empty Vec with a structured error hint. The daemon does NOT gate iteration progress on RAG availability; the implementer's non-RAG fallback behavior remains correct.

The `Anthropic` arm of the embedding dispatch SHALL exist as a defensive backstop returning `Err(anyhow!("anthropic does not support embeddings; configure canonical_rag.provider as ollama or openai_compatible"))`. In normal operation this is unreachable (config-load rejects `anthropic` for RAG); the backstop exists in case the validation is bypassed by a future code change.

#### Scenario: Absent `canonical_rag:` block disables the feature
- **WHEN** `config.yaml` does NOT contain a `canonical_rag:` top-level block
- **THEN** the daemon's workspace-init step skips the RAG pipeline entirely
- **AND** no `CanonicalRagStore` is registered for any workspace
- **AND** the implementer's MCP tool `query_canonical_specs` returns empty Vec (per the executor spec) with the error hint `rag disabled in config`
- **AND** no embedding-provider HTTP calls are issued at any point

#### Scenario: Present block with `enabled: false` is also disabled
- **WHEN** `config.yaml` contains `canonical_rag: { enabled: false, provider: ollama, model: nomic-embed-text, api_base_url: http://localhost:11434 }`
- **THEN** behavior is identical to absent block (no embed calls, empty tool results)
- **AND** the config is preserved so operators can flip `enabled: true` without re-entering field values

#### Scenario: Ollama provider embeds via the `/api/embed` endpoint
- **WHEN** `canonical_rag.provider: ollama` AND the daemon's workspace-init step runs
- **THEN** the daemon POSTs to `<api_base_url>/api/embed` with `{"model": "<model>", "input": [<chunk1>, <chunk2>, ...]}` for batches of up to 32 chunks
- **AND** parses the Ollama embedding response format into `Vec<Vec<f32>>`
- **AND** stores the resulting embeddings paired with their chunk metadata

#### Scenario: OpenAI-compatible provider embeds via `/embeddings`
- **WHEN** `canonical_rag.provider: openai_compatible` AND the daemon's workspace-init step runs
- **THEN** the daemon POSTs to `<api_base_url>/embeddings` with `{"model": "<model>", "input": [...]}` AND header `Authorization: Bearer <resolved-api-key>`
- **AND** parses the OpenAI embeddings response format
- **AND** the resolved API key comes from `canonical_rag.api_key.value` (inline) OR `std::env::var(canonical_rag.api_key_env)` (env-var path); inline wins if both are set with a WARN log

#### Scenario: Per-workspace store registry
- **WHEN** the daemon manages two repositories AND RAG is enabled for both
- **THEN** the registry contains two distinct `CanonicalRagStore` instances, one per workspace
- **AND** a `query_canonical_specs` call routes to the store matching the calling workspace's basename
- **AND** the stores are independent — embeds from one workspace's specs never surface in the other's results

#### Scenario: Provider failure at init fails open
- **WHEN** `canonical_rag.provider: ollama` AND `api_base_url` points at an unreachable host
- **THEN** the workspace-init RAG step logs a WARN naming the error
- **AND** the workspace's store is NOT registered in the registry
- **AND** subsequent `query_canonical_specs` calls return empty Vec with `error_hint: "rag init failed; see daemon log"`
- **AND** the polling iteration proceeds normally (no gate on RAG availability)
- **AND** subsequent iterations retry the init (no permanent-skip)

#### Scenario: `top_k` is clamped at startup
- **WHEN** `canonical_rag.top_k: 500`
- **THEN** the resolved value is `100` (the max)
- **AND** a WARN log at startup names both the requested AND clamped values

#### Scenario: `api_key` and `api_key_env` mutually exclusive
- **WHEN** both `canonical_rag.api_key.value` AND `canonical_rag.api_key_env` are set
- **THEN** the inline value wins
- **AND** a WARN log at startup names that the env var is being ignored

#### Scenario: `canonical_rag.provider: anthropic` rejected at config-load
- **WHEN** `config.yaml` contains `canonical_rag: { enabled: true, provider: anthropic, model: <m>, api_base_url: <u> }`
- **THEN** config-load fails with `canonical_rag does not support provider 'anthropic'; available providers: ollama, openai_compatible`
- **AND** the daemon exits non-zero before any polling task is spawned

#### Scenario: `canonical_rag.provider: ollama` with `api_key` rejected at config-load
- **WHEN** `config.yaml` contains `canonical_rag: { enabled: true, provider: ollama, model: <m>, api_base_url: <u>, api_key: { value: "anything" } }`
- **THEN** config-load fails with `canonical_rag: ollama does not authenticate; remove api_key field`
- **AND** the daemon exits non-zero

### Requirement: RAG re-embed cadence (workspace init and post-archive)
The RAG pipeline SHALL re-embed canonical specs at two events ONLY:

1. **Workspace init** — the first iteration of a workspace after daemon start (OR after a workspace wipe). The full canonical corpus is embedded synchronously before the iteration's executor invocation.
2. **Post-archive** (when `canonical_rag.reembed_on_archive: true`, default) — after any iteration's archive step that modifies at least one `<workspace>/openspec/specs/<cap>/spec.md` file. ONLY the affected capabilities' embeds are rebuilt, not the entire corpus.

Detection of "archive touched canonical": after the archive commit lands, run `git diff --name-only HEAD~N HEAD -- openspec/specs/` where N is the number of newly-archived commits in this iteration. Each unique `<cap>` directory present in the diff is a capability whose store entries SHALL be rebuilt.

**Disk-backed embedding cache.** Both embed events SHALL consult a per-workspace disk cache before calling the embedding provider: vectors keyed by a content hash over (provider, model, chunk text), stored at `<cache_dir>/rag-embeddings/<workspace-basename>.json` and written atomically (tempfile + rename). A chunk whose key is present loads its vector from the cache with NO provider call; a miss embeds via the provider and writes through. Each cache write SHALL retain only the keys present in the just-built corpus, so entries for deleted or edited chunks do not accumulate. The key's provider+model components mean a provider or model change misses the whole cache and re-embeds naturally — no explicit invalidation machinery. The cache is CACHE-category data: deleting the file (or the directory) is always safe and merely restores the pre-cache full-embed cost on the next event. An unreadable or corrupt cache file SHALL log a WARN, be treated as empty (full embed via the provider), and be overwritten by the subsequent write-through — cache trouble never fails a rebuild that the provider could serve.

Re-embed failures are fail-open: a failed rebuild leaves the existing embeds in place AND logs a WARN. The store may be temporarily stale; the next archive that touches the same capability OR a daemon restart will refresh it.

#### Scenario: Cold start embeds the full corpus
- **WHEN** the daemon starts up against a workspace that has not been embedded before
- **AND** `canonical_rag.enabled: true`
- **THEN** the workspace-init step embeds every `<workspace>/openspec/specs/<cap>/spec.md` file
- **AND** the log records `canonical RAG embedded N chunks across M capabilities for workspace <basename>`
- **AND** the executor's first invocation has access to the populated store

#### Scenario: Archive touching canonical re-embeds affected capabilities
- **WHEN** an iteration's archive step commits a change that modifies `<workspace>/openspec/specs/code-reviewer/spec.md`
- **AND** `canonical_rag.reembed_on_archive: true` (the default)
- **THEN** the post-archive RAG step computes the affected capabilities via `git diff --name-only` against the iteration's commits
- **AND** calls `rebuild_capabilities` for `["code-reviewer"]`
- **AND** existing entries for other capabilities are unchanged
- **AND** the log records `canonical RAG re-embedded 1 capability (code-reviewer) after archive`

#### Scenario: Archive NOT touching canonical does not re-embed
- **WHEN** an iteration archives changes whose deltas include implementation files AND `tasks.md` updates but NO `openspec/specs/<cap>/spec.md` modifications
- **THEN** the post-archive RAG step computes affected capabilities AND finds none
- **AND** no rebuild happens
- **AND** the log records no re-embed activity

#### Scenario: `reembed_on_archive: false` disables post-archive rebuilds
- **WHEN** `canonical_rag.reembed_on_archive: false`
- **THEN** post-archive re-embeds are suppressed entirely
- **AND** stores become stale across canonical-changing archives
- **AND** operators can manually trigger a rebuild via daemon restart OR a future explicit verb (not in this spec)

#### Scenario: Re-embed failure leaves prior embeds intact
- **WHEN** a post-archive rebuild attempt fails (provider unreachable, network blip)
- **THEN** the prior embeds for the affected capabilities are retained in the store
- **AND** a WARN log records the failure naming the capabilities AND the error
- **AND** queries continue to return chunks from the pre-rebuild embeds (stale-but-usable)

#### Scenario: Daemon restart rebuilds the store from the cache
- **WHEN** the daemon is stopped AND restarted later AND the canonical corpus is unchanged since the cache was last written
- **THEN** the in-memory store is empty at startup AND workspace-init rebuilds it
- **AND** every chunk's vector loads from the disk cache with ZERO embedding-provider calls
- **AND** the log records the rebuild naming the cache-hit count

#### Scenario: An edited chunk misses the cache and is re-embedded
- **WHEN** a canonical spec chunk's text changed since the cache was written (via archive-fold or rebuild)
- **THEN** the changed chunk's key misses AND that chunk is embedded via the provider
- **AND** unchanged chunks still load from the cache
- **AND** the write-through after the build contains the new chunk's vector and drops the superseded key

#### Scenario: A provider or model change invalidates the cache naturally
- **WHEN** `canonical_rag`'s embedding provider or model differs from the one the cache was written with
- **THEN** every lookup misses (the key includes provider and model) AND the corpus is fully re-embedded via the provider
- **AND** the subsequent write-through replaces the cache under the new keys

#### Scenario: A corrupt cache degrades to a full embed, never a failure
- **WHEN** the cache file exists but cannot be read or parsed
- **THEN** the rebuild logs a WARN naming the file AND proceeds exactly as a cold start (full provider embed)
- **AND** the write-through afterwards replaces the corrupt file
- **AND** deleting the cache file or directory by hand is always safe
