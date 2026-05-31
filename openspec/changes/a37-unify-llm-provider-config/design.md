# Design

## Decisions to lock in

### D1. Unify the enum, NOT the config blocks.

Each subsystem (reviewer, canonical_rag, change_internal_contradiction_check_llm) keeps its OWN config block AND its OWN `provider` field. The unification is purely at the enum level: all three reference the same canonical `LlmProvider` enum AND the same per-provider validation rules.

Rationale: operators routinely mix providers across subsystems. The realistic deployment has Anthropic/Grok/OpenRouter for reasoning-heavy completion (reviewer) AND Ollama for embeddings (RAG). A unified config block would force operators to share auth + model + base URL across subsystems that have nothing in common operationally. The cost of three config blocks is operator-visible (a bit more YAML to write) but the value is operator-visible too (independent picks per subsystem).

### D2. `ollama` for chat uses Ollama's native `/api/chat`, NOT the OpenAI-compat shim at `/v1/chat/completions`.

Ollama exposes both endpoints. The OpenAI-compat shim at `/v1/chat/completions` is provided for tools that expect OpenAI's wire shape; the native endpoint at `/api/chat` is what Ollama documents as the canonical chat API.

Going native:

- Eliminates the `/v1` URL trap operators are hitting today (the trap that motivated this change).
- Aligns with Ollama's own documentation AND release contract (the `/api/chat` shape is more stable across Ollama versions than the OpenAI-compat shim, which depends on Ollama maintaining wire compatibility with whatever OpenAI ships).
- Matches the existing convention for `OllamaEmbedClient` (which uses `/api/embed`, NOT `/v1/embeddings`).

The new `OllamaChatClient` is a small (~80 lines of Rust) sibling to `OllamaEmbedClient` AND `AnthropicClient`. It implements the same `LlmClient` trait so the rest of the codebase (reviewer dispatch, contradiction-check dispatch, future LLM-using callers) treats it uniformly.

### D3. `api_key` for `ollama` is FORBIDDEN, not just "ignored."

Ollama silently ignores the `Authorization` header. We could mirror that behavior by accepting an api_key for `ollama` AND silently dropping it on the client side. Instead, the new validation REJECTS the configuration with a clear message.

Reasoning: silently-ignored config fields are operator-visible bug magnets. An operator who sets `provider: ollama` + `api_key: "ollama"` (the dummy from the current hack) wonders if it matters. An operator who tries to "secure" their local Ollama by setting a real API key thinks it's enforced. Both are wrong; both are silent.

Failing fast with `Ollama does not authenticate; remove the api_key field` teaches the model. The error appears at startup, not at first feature trigger.

### D4. `anthropic` for embeddings is REJECTED at config-load, NOT supported via fallback.

Anthropic has no embeddings API as of this writing. The official docs route operators to third-party embedding services. Setting `canonical_rag.provider: anthropic` cannot work; falling back to some other provider AT RUNTIME would silently shift behavior under the operator's feet.

The fail-fast rejection at config-load is unambiguous: the operator sees `canonical_rag does not support provider 'anthropic'; available providers: ollama, openai_compatible` AND picks one. The defensive `Err` in `rag/embedding.rs`'s `anthropic` arm exists as a backstop in case the validation is bypassed; in normal operation it's unreachable.

### D5. Per-provider validity is enforced at config-load, NOT at the enum-variant level.

Alternative design: split `LlmProvider` into `CompletionProvider` (3 variants) AND `EmbeddingProvider` (2 variants). Reviewer field types `CompletionProvider`; RAG field types `EmbeddingProvider`. The schema enforces validity statically.

Rejected because:

- Two-enum maintenance burden grows non-linearly. Adding `bedrock` later means deciding which enum(s) it belongs in. The decision is real but it's an implementation detail, not a schema-shape concern.
- The error message from a malformed YAML field with a static-type mismatch is less actionable than the runtime validation message ("canonical_rag does not support provider 'anthropic'; available providers: ollama, openai_compatible"). Operators are better served by the runtime message.
- Future providers may have asymmetric support across subsystems in ways that aren't predictable (e.g. a hypothetical provider that supports embeddings AND a specific completion model but not generic chat). The runtime-validation model accommodates that without schema churn.

### D6. Backward compatibility via serde type-alias retention + identical YAML strings.

Existing config files use:

```yaml
reviewer:
  provider: anthropic
canonical_rag:
  provider: ollama
```

After this change, the same YAML continues to load. The implementation:

- `LlmProvider` enum has the three variants with their canonical YAML strings (`anthropic`, `openai_compatible`, `ollama`).
- `RagProvider` AND `ReviewerProvider` are removed from the canonical type set, BUT `pub type RagProvider = LlmProvider; pub type ReviewerProvider = LlmProvider;` aliases are retained for any external-crate or test-code consumers that imported the old types.
- The struct field types (`ReviewerConfig::provider: ReviewerProvider`) change to `LlmProvider`, but the alias makes this transparent to existing callers.

No serde migration needed. No config-file rewrite needed. Operators who never set a forbidden combination (anthropic-for-RAG, openai_compatible-without-api_key, ollama-with-api_key) see zero behavioral change.

Operators who DO have a forbidden combination today see the new error at the next `systemctl restart autocoder`. This is the right time to surface the failure — at deploy, not at first feature trigger.

### D7. Validation runs ONCE at config-load, NOT lazily.

Each subsystem's config-load helper (the existing `ReviewerConfig::resolve_*`, `RagConfig::validate`, etc.) gains a single `validate_llm_provider_combo(&self, subsystem_name) -> Result<()>` call. The helper checks:

1. Is `provider` valid for this subsystem? (RAG rejects anthropic; reviewer accepts all three.)
2. Is `api_key` present-or-absent matching the provider's auth model? (anthropic + openai_compatible require it; ollama forbids it.)
3. Is `api_base_url` shaped correctly for the provider? (anthropic doesn't require it — defaults to `https://api.anthropic.com`; openai_compatible AND ollama require it — no sensible default.)

The validation is purely shape-checking; it doesn't probe network reachability. Network failures are runtime concerns AND are surfaced when the feature first triggers (existing behavior).

## Open questions for the implementer

- **`OllamaChatClient` payload shape.** Ollama's `/api/chat` accepts `{"model": "...", "messages": [...], "stream": false}` AND returns `{"message": {"role": "assistant", "content": "..."}, "done": true, ...}`. The client should set `stream: false` to get a single-response payload (matching the existing `AnthropicClient` AND `OpenAiCompatibleClient` shape). Streaming support is out of scope; future change if needed.
- **Default `model` per provider.** No defaults today; the operator MUST set `model`. Could add defaults (`claude-opus-4-7` for anthropic, `gpt-4` for openai_compatible, no default for ollama) but operators tend to want explicit control here. Defer unless asked.
- **`install` wizard provider list.** The existing wizard prompts for `--rag-provider <ollama|openai_compatible|none>` AND `--reviewer-provider <anthropic|openai_compatible|none>`. After this change, the reviewer prompt SHOULD also offer `ollama`. The wizard update is a tasks.md item; the spec binds the schema validation rules, not the wizard's exact prompt phrasing.
- **Migration helper.** Could add a `autocoder config migrate-llm-providers` subcommand that rewrites operator configs to remove the `openai_compatible`+ollama-shim hack. Probably overkill; the operator's existing config keeps working AND the documented new shape is short enough that operators migrate manually if they want. Document in `config.example.yaml`'s comments.

## Stack interaction

This change is independent of every queued change. Plays well with everything:

- a2705 (revise strict-since filter), a31 (revise lifecycle notifications), a32-equivalents — purely PR-side, unrelated to LLM providers.
- a27a* (outcome tools, iteration request, acceptance scan) — purely executor-side, unrelated.
- a33 (code-review trigger), a34 (spec-storage routing), a35 (paths globals removal), a36 (inspect CLI) — all unrelated.

Recommended deploy: any time. The reviewer-config-currently-broken-against-Ollama failure mode is operator-visible TODAY, but the immediate workaround (add `/v1` to the api_base_url) bypasses the bug without needing this change to ship. Land a37 when convenient; operators with the workaround are unaffected; operators who haven't migrated yet get the cleaner schema.
