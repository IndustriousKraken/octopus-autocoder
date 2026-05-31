# Tasks

## 1. Canonical `LlmProvider` enum

- [ ] 1.1 In `autocoder/src/config.rs`, define `pub enum LlmProvider { Anthropic, OpenAiCompatible, Ollama }` with `#[serde(rename_all = "snake_case")]` so YAML values are `anthropic`, `openai_compatible`, `ollama`.
- [ ] 1.2 Add type aliases `pub type RagProvider = LlmProvider;` AND `pub type ReviewerProvider = LlmProvider;` so existing call sites compile unchanged.
- [ ] 1.3 Remove the old `RagProvider` AND `ReviewerProvider` enum definitions (the type aliases above replace them).
- [ ] 1.4 Unit-test: each variant round-trips through serde (YAML deserialize → enum → YAML serialize) with the canonical string. Backward compat: a config file pre-spec parses identically post-spec.

## 2. Per-provider auth validator

- [ ] 2.1 Add a helper `pub fn validate_llm_provider_config(provider: LlmProvider, api_key: Option<&SecretSource>, api_base_url: Option<&str>, subsystem: &str) -> Result<()>` in `autocoder/src/config.rs`.
- [ ] 2.2 Validation rules per provider:
  - `Anthropic`: `api_key` REQUIRED. `api_base_url` OPTIONAL (defaults to `https://api.anthropic.com`).
  - `OpenAiCompatible`: `api_key` REQUIRED. `api_base_url` REQUIRED.
  - `Ollama`: `api_key` FORBIDDEN. `api_base_url` REQUIRED.
- [ ] 2.3 Error messages reference the `subsystem` name AND the offending field. Examples:
  - `reviewer: anthropic requires api_key; set reviewer.api_key.value or reviewer.api_key_env`
  - `canonical_rag: ollama does not authenticate; remove api_key field`
  - `change_internal_contradiction_check_llm: openai_compatible requires api_base_url; set the field to e.g. https://api.openai.com/v1`
- [ ] 2.4 Unit-test each (provider, api_key-present-or-absent, api_base_url-present-or-absent) combination with the expected error message OR success.

## 3. Per-subsystem provider-validity validator

- [ ] 3.1 Add a helper `pub fn validate_provider_for_subsystem(provider: LlmProvider, subsystem: SubsystemKind) -> Result<()>` where `SubsystemKind` is an internal enum with variants `Reviewer`, `CanonicalRag`, `ContradictionCheck`.
- [ ] 3.2 Validity table:
  - `Reviewer` → `Anthropic | OpenAiCompatible | Ollama` (all valid).
  - `ContradictionCheck` → `Anthropic | OpenAiCompatible | Ollama` (all valid).
  - `CanonicalRag` → `OpenAiCompatible | Ollama` (anthropic invalid).
- [ ] 3.3 Error message: `<subsystem> does not support provider '<rejected>'; available providers: <valid list, comma-separated>`.
- [ ] 3.4 Unit-test the matrix.

## 4. Wire validators into existing config-load paths

- [ ] 4.1 `ReviewerConfig::resolve_*` (OR its equivalent during config-load) calls both validators with `subsystem: "reviewer"` / `SubsystemKind::Reviewer`.
- [ ] 4.2 `RagConfig::validate` (OR its equivalent) calls both validators with `subsystem: "canonical_rag"` / `SubsystemKind::CanonicalRag`.
- [ ] 4.3 `ContradictionCheckLlmConfig` validation calls both validators with `subsystem: "change_internal_contradiction_check_llm"` / `SubsystemKind::ContradictionCheck`.
- [ ] 4.4 Integration test: a config file with `canonical_rag.provider: anthropic` fails `Config::load_from` with the documented message AND the daemon exits non-zero before any polling task is spawned.
- [ ] 4.5 Integration test: a config file with `reviewer.provider: ollama` AND `api_key.value: "anything"` fails `Config::load_from` with the documented message.
- [ ] 4.6 Integration test: a config file with `reviewer.provider: ollama` AND no `api_key` AND `api_base_url: http://localhost:11434` loads cleanly.

## 5. New `OllamaChatClient`

- [ ] 5.1 In `autocoder/src/llm.rs`, add `pub struct OllamaChatClient { api_base: String, model: String }` (no `api_key` field — Ollama doesn't authenticate).
- [ ] 5.2 Implement `LlmClient::complete(&self, prompt: &str) -> Result<String>`:
  - POST to `<api_base>/api/chat` (trim trailing slashes off api_base).
  - Body: `{"model": <model>, "messages": [{"role": "user", "content": <prompt>}], "stream": false}`.
  - No `Authorization` header.
  - Parse response: `{"message": {"role": "assistant", "content": "<text>"}, "done": true, ...}`. Return the `message.content` string.
  - On non-2xx, return `Err` with the status + first 500 chars of the response body (matching `OpenAiCompatibleClient`'s error shape).
  - On 2xx with malformed JSON, return `Err` with a clear decode-failure message.
- [ ] 5.3 Update `llm::build_from_config` AND `llm::build_from_contradiction_check_config` to construct `OllamaChatClient` when `provider == LlmProvider::Ollama`.
- [ ] 5.4 Unit-test against a mockito-style mock server:
  - Successful response with `{"message":{"content":"the answer"}}` returns `Ok("the answer")`.
  - 404 response returns `Err` containing `404`.
  - 200 response with non-JSON body returns `Err` with a decode-failure message.
  - 200 response with JSON missing `message.content` returns `Err`.
- [ ] 5.5 Verify no `Authorization` header is sent (assert via mock server expectations).

## 6. RAG embedding-side anthropic rejection

- [ ] 6.1 In `autocoder/src/rag/embedding.rs`, the dispatch from `LlmProvider` to embed client gains an `Anthropic` arm that returns `Err(anyhow!("anthropic does not support embeddings; configure canonical_rag.provider as ollama or openai_compatible"))`. This is a defensive backstop; config-load validation should make it unreachable in practice.
- [ ] 6.2 Unit-test that the dispatch panics OR errors cleanly when given `LlmProvider::Anthropic` (depending on whether the codepath is gated by `unreachable!` or `?` propagation).

## 7. Install wizard update

- [ ] 7.1 In `autocoder/src/cli/install.rs` (OR the wizard module), extend the reviewer-provider prompt to offer `ollama` alongside `anthropic` AND `openai_compatible`. The flag form `--reviewer-provider ollama` SHALL be accepted.
- [ ] 7.2 When the operator picks `ollama` for the reviewer, the wizard prompts for `api_base_url` (with `http://localhost:11434` as the suggested default) AND `model` (no default; operator-driven). The wizard does NOT prompt for `api_key`.
- [ ] 7.3 Integration-test the new wizard branch (mirrors the existing `wizard_rag_*` tests; one for the `ollama` choice).

## 8. config.example.yaml documentation

- [ ] 8.1 Add an `ollama` example block to the reviewer section:
  ```yaml
  # Local Ollama for the reviewer (example):
  # reviewer:
  #   enabled: true
  #   provider: ollama
  #   model: qwen2.5-coder:32b
  #   api_base_url: http://localhost:11434
  #   # no api_key — Ollama does not authenticate
  #   prompt_budget_chars: 300000
  #   mode: per_change
  ```
- [ ] 8.2 Update the `canonical_rag:` section's comment to name `ollama | openai_compatible` AND note that `anthropic` is rejected at config-load.
- [ ] 8.3 Note in both sections that `api_base_url` is the API root; the client appends its own protocol-specific path (`/v1/messages` for anthropic, `/chat/completions` for openai_compatible, `/api/chat` for ollama).

## 9. Validation

- [ ] 9.1 `cargo test` passes.
- [ ] 9.2 `cargo clippy` produces no NEW warnings against the existing baseline.
- [ ] 9.3 `openspec validate a37-unify-llm-provider-config --strict` passes.
