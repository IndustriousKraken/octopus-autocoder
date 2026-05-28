## ADDED Requirements

### Requirement: Canonical-spec RAG configuration and pipeline
autocoder SHALL support a per-workspace retrieval-augmented-context pipeline that embeds the workspace's canonical OpenSpec specs (`openspec/specs/<capability>/spec.md`) into an in-memory vector store AND exposes a retrieval surface for the implementer (via `a21`'s executor MCP requirement) AND for downstream pre-flight checks (`a22`'s change-vs-canon contradiction check). The pipeline is configured via a top-level `canonical_rag:` block in `config.yaml`; an absent block disables the feature entirely. A present block with `enabled: false` also disables; both forms preserve "no behavior change" for operators who don't opt in.

The `canonical_rag:` config block contains: `enabled: bool`, `provider: ollama | openai_compatible`, `model: string`, `api_base_url: string`, `api_key_env: string?` AND `api_key: SecretSource?` (mutually exclusive — inline wins with WARN if both set; same pattern as `reviewer:`), `top_k: usize` (default `10`, clamped `[1, 100]` with WARN), `chunk_strategy: per_requirement | per_scenario | per_capability` (default `per_requirement`), AND `reembed_on_archive: bool` (default `true`).

The embedding pipeline SHALL:
- Build an `EmbedClient` from the provider config — an Ollama adapter calling `<base_url>/api/embed` for Ollama, OR an OpenAI-compatible adapter calling `<base_url>/embeddings` with `Authorization: Bearer <api_key>` for the openai_compatible path.
- Glob `<workspace>/openspec/specs/<cap>/spec.md` files, chunk each per `chunk_strategy`, embed each chunk via the client, AND store `(chunk, embedding, source_path, capability, requirement_title)` tuples in an in-memory `CanonicalRagStore`.
- Maintain a per-workspace store registry keyed by sanitized workspace basename. Multiple managed repos each have their own store; the stores are independent.
- Persist NOTHING to disk. Daemon restart re-embeds from scratch on workspace-init.

Failure modes are fail-open: embedding-provider errors (network, auth, rate-limit) at init log WARN AND omit the workspace's store from the registry. Subsequent queries against the absent store return empty Vec with a structured error hint. The daemon does NOT gate iteration progress on RAG availability; the implementer's non-RAG fallback behavior remains correct.

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

### Requirement: RAG re-embed cadence (workspace init and post-archive)
The RAG pipeline SHALL re-embed canonical specs at two events ONLY:

1. **Workspace init** — the first iteration of a workspace after daemon start (OR after a workspace wipe). The full canonical corpus is embedded synchronously before the iteration's executor invocation.
2. **Post-archive** (when `canonical_rag.reembed_on_archive: true`, default) — after any iteration's archive step that modifies at least one `<workspace>/openspec/specs/<cap>/spec.md` file. ONLY the affected capabilities' embeds are rebuilt, not the entire corpus.

Detection of "archive touched canonical": after the archive commit lands, run `git diff --name-only HEAD~N HEAD -- openspec/specs/` where N is the number of newly-archived commits in this iteration. Each unique `<cap>` directory present in the diff is a capability whose store entries SHALL be rebuilt.

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

#### Scenario: Daemon restart re-embeds from scratch
- **WHEN** the daemon is stopped AND restarted later
- **THEN** the in-memory store is empty at startup (no on-disk persistence)
- **AND** workspace-init re-runs the full embedding pipeline for every configured workspace
- **AND** the cost is `O(N capabilities × M chunks × embed-call-latency)` — typically sub-second on GPU, ~30 seconds on CPU for a typical corpus

### Requirement: Install-wizard graduated RAG-configuration flow
`autocoder install` (interactive mode) SHALL prompt the operator about RAG configuration AND walk them through a graduated set of options designed to find a working RAG setup for their environment without requiring API keys when avoidable. The flow:

1. Prompt: `Configure canonical-specs RAG? (Y/n)`. Default Y. If N, write no `canonical_rag:` block AND continue with the rest of the wizard.
2. If Y, probe Ollama on localhost: HTTP GET `http://localhost:11434/api/tags` with a 2-second timeout.
3. If localhost Ollama is reachable: suggest using it. Prompt for `model` (default `nomic-embed-text` for the docker-default-compatible case; the wizard may suggest `qwen3-embedding:4b` if operator inputs indicate GPU availability — but the spec doesn't mandate GPU detection).
4. If localhost Ollama is NOT reachable, present a four-option menu:
   - **(1) Install local Ollama via docker** — wizard copies `install/ollama-docker-compose.yml` to `<config_dir>/ollama-docker-compose.yml` AND prints the `docker compose -f <path> up -d` command. The wizard does NOT auto-run docker. Writes the `canonical_rag:` block pointing at `http://localhost:11434` so the daemon connects once the operator starts docker.
   - **(2) Remote Ollama** — prompt for `base_url` + `model`. Probe `<base_url>/api/tags`. On success, write the config block. On probe failure, prompt the operator to retry OR fall back to one of the other options.
   - **(3) OpenAI-compatible endpoint** — prompt for `base_url`, `model`, AND api-key source (env var name OR inline value). Probe via a small embed call. Same retry-or-fallback semantics as option 2.
   - **(4) Disable RAG** — write no block; continue with the wizard. Print a one-liner about how to enable later (`canonical_rag:` block in config.yaml).
5. Non-interactive mode: accept flags `--rag-provider <ollama|openai_compatible|none>`, `--rag-base-url <url>`, `--rag-model <model>`, `--rag-api-key-env <name>`. Failing to provide flags with `--rag-provider ollama|openai_compatible` is a startup error.

The wizard's RAG step is testable via the existing `ScriptedIo` test harness with mocked HTTP probes.

#### Scenario: Localhost Ollama detected and chosen
- **WHEN** the wizard reaches the RAG prompt AND `http://localhost:11434/api/tags` returns 200 within 2 seconds
- **AND** the operator confirms `Y` AND accepts the default model
- **THEN** the wizard writes `canonical_rag: { enabled: true, provider: ollama, model: <chosen>, api_base_url: http://localhost:11434, top_k: 10 }` to the config
- **AND** the wizard does NOT install Ollama, pull models, OR spawn docker

#### Scenario: Localhost not detected; docker option chosen
- **WHEN** the localhost probe fails AND the operator picks option 1 (docker)
- **THEN** the wizard copies `install/ollama-docker-compose.yml` from the in-tree path to `<config_dir>/ollama-docker-compose.yml`
- **AND** writes the `canonical_rag:` block pointing at `http://localhost:11434`
- **AND** prints `docker compose -f <config_dir>/ollama-docker-compose.yml up -d` as the operator's explicit next step
- **AND** the wizard does NOT auto-run docker

#### Scenario: Remote Ollama option with successful probe
- **WHEN** the operator picks option 2 AND enters `base_url: http://gpu-host:11434` AND model `qwen3-embedding:4b`
- **AND** the probe of `http://gpu-host:11434/api/tags` succeeds
- **THEN** the wizard writes the corresponding `canonical_rag:` block
- **AND** the wizard does NOT need any further operator action; the daemon's workspace-init will connect on first iteration

#### Scenario: OpenAI-compatible option with probe failure
- **WHEN** the operator picks option 3 AND enters a `base_url` that's unreachable OR an invalid API key
- **THEN** the probe (a tiny embed call) fails AND the wizard reports the error
- **AND** prompts the operator to retry (correct the inputs) OR fall back to option 4 (disable)
- **AND** does NOT write a misconfigured block

#### Scenario: Disable option (4) writes no block
- **WHEN** the operator picks option 4 OR answers `n` at the initial Y/n prompt
- **THEN** no `canonical_rag:` block is written
- **AND** a one-liner explains how to enable RAG later by editing config.yaml AND running `autocoder reload`

#### Scenario: Non-interactive mode requires RAG flags when provider is set
- **WHEN** `autocoder install --non-interactive --rag-provider ollama` is invoked WITHOUT `--rag-base-url`
- **THEN** the install fails fast with an error naming the missing flag
- **AND** no config is written
