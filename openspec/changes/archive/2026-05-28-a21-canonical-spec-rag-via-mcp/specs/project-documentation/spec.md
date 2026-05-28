## ADDED Requirements

### Requirement: CONFIG.md, OPERATIONS.md, CHATOPS.md, and DEPLOYMENT.md document the RAG configuration and operator workflow
`docs/CONFIG.md` SHALL include a `canonical_rag:` section documenting every config field. `docs/OPERATIONS.md` SHALL include a "Canonical-spec RAG" operational section covering re-embed cadence, in-memory persistence model, failure modes, AND cost expectations. `docs/CHATOPS.md` SHALL include a one-line note in the implementer-flow section about the new `query_canonical_specs` tool. `docs/DEPLOYMENT.md` SHALL include a "Self-hosted Ollama for RAG" subsection covering the docker-compose quick-start AND the remote-Ollama deployment.

#### Scenario: CONFIG.md documents every `canonical_rag:` field
- **WHEN** an operator reads `docs/CONFIG.md`'s `canonical_rag:` section
- **THEN** every field is documented with type, default, AND a one-line description (`enabled`, `provider`, `model`, `api_base_url`, `api_key_env`, `api_key`, `top_k`, `chunk_strategy`, `reembed_on_archive`)
- **AND** the section notes the mutual-exclusivity of `api_key_env` AND `api_key` (same pattern as `reviewer:`)
- **AND** the section cross-links to OPERATIONS.md for the operational discussion

#### Scenario: OPERATIONS.md describes the cadence and failure modes
- **WHEN** an operator reads `docs/OPERATIONS.md`'s "Canonical-spec RAG" section
- **THEN** the section describes the two re-embed triggers (workspace init; post-archive touching canonical) AND when each fires
- **AND** the section explains in-memory persistence (no disk store; daemon restart re-embeds)
- **AND** the section names the failure modes (provider-error at init → WARN + RAG disabled for the workspace's lifetime; per-query error → empty Vec; the daemon never gates iteration progress on RAG availability)
- **AND** the section gives cost expectations (sub-second embed on GPU; ~30s on CPU for typical corpus; once-per-archive thereafter)

#### Scenario: CHATOPS.md notes the new implementer tool
- **WHEN** an operator reads `docs/CHATOPS.md`'s implementer-flow discussion (or equivalent section)
- **THEN** a one-line note names `query_canonical_specs` AND that results show in the per-change run log
- **AND** the note links to OPERATIONS.md for the full RAG discussion

#### Scenario: DEPLOYMENT.md covers self-hosted Ollama options
- **WHEN** an operator reads `docs/DEPLOYMENT.md`'s "Self-hosted Ollama for RAG" subsection
- **THEN** the subsection describes the bundled `install/ollama-docker-compose.yml` quick-start (the file the install wizard's option 1 copies into `<config_dir>/`)
- **AND** describes pointing at a remote Ollama on a GPU machine via `api_base_url: http://gpu-host:11434`
- **AND** gives hardware suggestions (CPU works; GPU is faster but not required for the corpus size)
- **AND** notes that the docker-compose default pulls `nomic-embed-text` as the entrypoint; operators with bigger hardware can edit the compose file to pull `qwen3-embedding:4b` or larger
