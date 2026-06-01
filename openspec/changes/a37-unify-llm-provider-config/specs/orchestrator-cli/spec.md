## ADDED Requirements

### Requirement: Canonical `LlmProvider` enum AND per-provider auth semantics

The autocoder config schema SHALL define a single canonical `LlmProvider` enum with three variants AND their YAML strings:

- `anthropic` — Anthropic's hosted API (`https://api.anthropic.com` default).
- `openai_compatible` — Any OpenAI-API-shaped endpoint (OpenAI itself, Grok, OpenRouter, vLLM, local OpenAI-compat shims, etc.).
- `ollama` — Ollama's native API (`<base>/api/chat` for completion, `<base>/api/embed` for embeddings).

`LlmProvider` SHALL be the type of the `provider` field across every LLM-touching config block: `reviewer:`, `canonical_rag:`, AND `executor.change_internal_contradiction_check_llm:`. Backward compatibility: the existing `RagProvider` AND `ReviewerProvider` enum names SHALL be retained as type aliases (`pub type RagProvider = LlmProvider;` etc.) so external-crate or test-code consumers compile unchanged. Existing config files using `provider: anthropic`, `provider: openai_compatible`, AND `provider: ollama` parse identically post-spec.

The `api_key` field's mandatory-ness SHALL be determined by the resolved provider, NOT by the subsystem:

- `anthropic` → `api_key` REQUIRED (either via `api_key.value` inline OR `api_key_env` pointing at a set env var). Config-load fails-fast if absent.
- `openai_compatible` → `api_key` REQUIRED. Same fail-fast rule.
- `ollama` → `api_key` FORBIDDEN. Config-load fails-fast if the operator sets one with the message `<subsystem>: ollama does not authenticate; remove api_key field`. This is a behavioral departure from "silently ignore" — operators learn the auth model at startup rather than carrying dummy values forward.

The `api_base_url` field's mandatory-ness SHALL similarly be provider-driven:

- `anthropic` → OPTIONAL (defaults to `https://api.anthropic.com`).
- `openai_compatible` → REQUIRED (no sensible default for a generic compat endpoint).
- `ollama` → REQUIRED (operator's Ollama host).

The `api_base_url` SHALL be treated as the API root by every provider's client. Each client knows what protocol-specific path to append:

- `anthropic` → `<base>/v1/messages`.
- `openai_compatible` → `<base>/chat/completions` (for chat) OR `<base>/embeddings` (for embeddings).
- `ollama` → `<base>/api/chat` (for chat) OR `<base>/api/embed` (for embeddings).

Operators using `openai_compatible` against hosted services that require `/v1` in the URL (OpenAI, Grok, OpenRouter) SHALL include `/v1` in their `api_base_url`. The client does NOT auto-append `/v1`; the convention is "operator owns the API root."

Validation runs ONCE at config-load (not lazily). A misconfigured provider surfaces as a fail-fast error at `systemctl restart autocoder`, not as a 404 OR permission error on first feature trigger.

#### Scenario: `LlmProvider` round-trips through serde
- **WHEN** a config file contains `provider: anthropic` (OR `openai_compatible`, OR `ollama`)
- **THEN** the field deserializes into `LlmProvider::Anthropic` (resp. `OpenAiCompatible`, `Ollama`)
- **AND** re-serializing produces the same YAML string

#### Scenario: `RagProvider` AND `ReviewerProvider` aliases compile
- **WHEN** code references the type names `RagProvider` OR `ReviewerProvider`
- **THEN** the names resolve to `LlmProvider` via type aliases
- **AND** no source-code change is required to consumers of the old type names

#### Scenario: `anthropic` requires `api_key`
- **WHEN** a config block sets `provider: anthropic` AND omits both `api_key` AND `api_key_env`
- **THEN** config-load fails with `<subsystem>: anthropic requires api_key; set <subsystem>.api_key.value or <subsystem>.api_key_env`
- **AND** the daemon exits non-zero before any polling task is spawned

#### Scenario: `openai_compatible` requires `api_key`
- **WHEN** a config block sets `provider: openai_compatible` AND omits both `api_key` AND `api_key_env`
- **THEN** config-load fails with `<subsystem>: openai_compatible requires api_key; set <subsystem>.api_key.value or <subsystem>.api_key_env`

#### Scenario: `openai_compatible` requires `api_base_url`
- **WHEN** a config block sets `provider: openai_compatible` AND omits `api_base_url`
- **THEN** config-load fails with `<subsystem>: openai_compatible requires api_base_url; set the field to e.g. https://api.openai.com/v1`

#### Scenario: `ollama` forbids `api_key`
- **WHEN** a config block sets `provider: ollama` AND sets `api_key.value` OR `api_key_env`
- **THEN** config-load fails with `<subsystem>: ollama does not authenticate; remove api_key field`
- **AND** the failure message names that Ollama silently ignores Authorization headers

#### Scenario: `ollama` requires `api_base_url`
- **WHEN** a config block sets `provider: ollama` AND omits `api_base_url`
- **THEN** config-load fails with `<subsystem>: ollama requires api_base_url; set the field to e.g. http://localhost:11434`

#### Scenario: `anthropic` defaults `api_base_url` cleanly
- **WHEN** a config block sets `provider: anthropic`, `api_key.value: <some-key>`, AND omits `api_base_url`
- **THEN** config-load succeeds
- **AND** the resolved `api_base_url` is `https://api.anthropic.com`

### Requirement: Per-subsystem provider validity is enforced at config-load

Different LLM-using subsystems have different supported provider sets. Validity SHALL be enforced at config-load with a clear actionable error.

Subsystem validity table:

- `reviewer.provider` → `anthropic | openai_compatible | ollama` (all three valid; reviewer does completion).
- `executor.change_internal_contradiction_check_llm.provider` → `anthropic | openai_compatible | ollama` (all three valid; same shape as reviewer).
- `canonical_rag.provider` → `openai_compatible | ollama` (anthropic INVALID; Anthropic does not expose an embeddings API).

When an operator picks a provider NOT in a subsystem's valid set, config-load SHALL fail with the message `<subsystem> does not support provider '<rejected>'; available providers: <comma-separated valid list>` AND the daemon SHALL exit non-zero before any polling task is spawned.

#### Scenario: `canonical_rag.provider: anthropic` rejected
- **WHEN** a config file contains `canonical_rag: { enabled: true, provider: anthropic, ... }`
- **THEN** config-load fails with `canonical_rag does not support provider 'anthropic'; available providers: ollama, openai_compatible`
- **AND** the daemon exits non-zero

#### Scenario: `reviewer.provider: ollama` accepted
- **WHEN** a config file contains `reviewer: { enabled: true, provider: ollama, model: <model>, api_base_url: http://localhost:11434, ... }` (no api_key)
- **THEN** config-load succeeds
- **AND** the resolved reviewer config carries `LlmProvider::Ollama` AND `api_base_url: http://localhost:11434`
- **AND** the daemon proceeds with normal startup

#### Scenario: `change_internal_contradiction_check_llm.provider: ollama` accepted
- **WHEN** the contradiction-check LLM is configured with `provider: ollama` AND a base URL AND no api_key
- **THEN** config-load succeeds
- **AND** the contradiction-check uses the new `OllamaChatClient`

## MODIFIED Requirements

### Requirement: Canonical-spec RAG configuration and pipeline

autocoder SHALL support a per-workspace retrieval-augmented-context pipeline that embeds the workspace's canonical OpenSpec specs (`openspec/specs/<capability>/spec.md`) into an in-memory vector store AND exposes a retrieval surface for the implementer (via `a21`'s executor MCP requirement) AND for downstream pre-flight checks (`a22`'s change-vs-canon contradiction check). The pipeline is configured via a top-level `canonical_rag:` block in `config.yaml`; an absent block disables the feature entirely. A present block with `enabled: false` also disables; both forms preserve "no behavior change" for operators who don't opt in.

The `canonical_rag:` config block contains: `enabled: bool`, `provider: LlmProvider` (subsystem-valid subset: `ollama | openai_compatible`; `anthropic` is rejected at config-load per the per-subsystem provider-validity requirement), `model: string`, `api_base_url: string` (required for both valid providers), `api_key_env: string?` AND `api_key: SecretSource?` (mutually exclusive — inline wins with WARN if both set; same pattern as `reviewer:`; FORBIDDEN entirely when `provider: ollama` per the per-provider auth-semantics requirement), `top_k: usize` (default `10`, clamped `[1, 100]` with WARN), `chunk_strategy: per_requirement | per_scenario | per_capability` (default `per_requirement`), AND `reembed_on_archive: bool` (default `true`).

The embedding pipeline SHALL:
- Build an `EmbedClient` from the provider config — an Ollama adapter calling `<base_url>/api/embed` for Ollama, OR an OpenAI-compatible adapter calling `<base_url>/embeddings` with `Authorization: Bearer <api_key>` for the openai_compatible path.
- Glob `<workspace>/openspec/specs/<cap>/spec.md` files, chunk each per `chunk_strategy`, embed each chunk via the client, AND store `(chunk, embedding, source_path, capability, requirement_title)` tuples in an in-memory `CanonicalRagStore`.
- Maintain a per-workspace store registry keyed by sanitized workspace basename. Multiple managed repos each have their own store; the stores are independent.
- Persist NOTHING to disk. Daemon restart re-embeds from scratch on workspace-init.

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
