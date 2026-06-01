## Why

Three LLM-touching config blocks exist in autocoder today (`reviewer:`, `canonical_rag:`, `executor.change_internal_contradiction_check_llm:`), AND each declares its own provider enum with a DIFFERENT variant set:

- `RagProvider` (for `canonical_rag.provider`): `ollama | openai_compatible`.
- `ReviewerProvider` (for `reviewer.provider` AND `change_internal_contradiction_check_llm.provider`): `anthropic | openai_compatible`.

`ollama` is first-class in RAG but unavailable for reviewer. `anthropic` is first-class in reviewer but unavailable for RAG. Operators running Ollama locally for both subsystems (reasonable: Ollama is the obvious local choice for embeddings AND for completion when GPU allows) hit a wall on the reviewer side — there's no `provider: ollama` to pick. The path-of-least-resistance is the hack the operator just hit: set `provider: openai_compatible`, set a dummy `api_key: "ollama"`, AND set `api_base_url: http://<ollama-host>:11434/v1` (with the `/v1` carefully appended so the openai_compatible client's `format!("{}/chat/completions", base)` resolves to Ollama's OpenAI-shim endpoint).

That hack works in principle BUT is silently broken if any one of the three workarounds is missed:

- Missing `api_key` → config-load fails (operator-visible).
- Wrong dummy `api_key` value → request still works (Ollama ignores Authorization header) but operator wonders if it matters.
- Missing `/v1` in URL → request goes to `http://<ollama-host>:11434/chat/completions` (no such endpoint) → 404 on first review attempt → reviewer reports failure → operator confused. This last failure mode is observable in the production config that triggered this change.

Beyond the immediate fix, the schema's incoherence has structural costs:

- Operators can't mentally map "which providers are valid where." The two enums look similar but aren't, AND the docs don't surface the asymmetry in one place.
- The `api_key` field's mandatory-ness is baked per-provider in conflicting ways: `ollama` for RAG accepts an absent key (correct — Ollama doesn't authenticate); `openai_compatible` for reviewer ALWAYS requires a key (incorrect when the operator is using `openai_compatible` to point at a no-auth Ollama). The two subsystems disagree on whether "no auth" is a config state worth modeling.
- Adding a third provider in the future (e.g. `bedrock`, `vllm`, `together_ai`) requires touching two enums in two places, with manual symmetry across the schema. Drift is the default.

This change unifies the provider model into a single canonical `LlmProvider` enum AND fills in the gaps (ollama-for-completion, anthropic-for-RAG-rejected-with-clear-message). Each subsystem keeps its OWN config block (no shared global LLM config); only the provider enum unifies. Operators can pick `reviewer.provider: anthropic` AND `canonical_rag.provider: ollama` independently, which is the natural pairing for most real deployments.

## What Changes

**New canonical `LlmProvider` enum** with three variants: `anthropic`, `openai_compatible`, `ollama`. Replaces both `RagProvider` AND `ReviewerProvider` at the schema level. Serde aliases preserve backward compatibility for existing config files (the operator's `provider: ollama` AND `provider: openai_compatible` AND `provider: anthropic` strings continue to parse exactly as today).

**Per-provider `api_key` semantics — driven by provider, NOT by subsystem**:

- `anthropic` — `api_key` REQUIRED. Config-load fails-fast if absent.
- `openai_compatible` — `api_key` REQUIRED. Hosted services (Grok, OpenRouter, OpenAI, Anthropic-via-shim) all need it.
- `ollama` — `api_key` FORBIDDEN. Config-load fails-fast if the operator sets one, with a clear message naming `Ollama does not authenticate; remove the api_key field` so the operator learns the model rather than carrying the dummy value forward.

**Per-subsystem provider validity** (enforced at config-load):

- `reviewer.provider`: any of `anthropic | openai_compatible | ollama`. All three valid because the reviewer issues completion calls AND all three providers support completion.
- `change_internal_contradiction_check_llm.provider`: same as reviewer. All three valid.
- `canonical_rag.provider`: `openai_compatible | ollama` only. Anthropic does NOT expose an embeddings API (no `/embeddings` route, no model that does embeddings). Setting `provider: anthropic` for RAG fails config-load with `canonical_rag does not support provider 'anthropic'; available providers: ollama, openai_compatible` so the operator learns the model immediately.

**New `OllamaChatClient` for completion-using subsystems**. The current codebase has an `OllamaEmbedClient` (for RAG) BUT no Ollama completion client. To support `provider: ollama` for the reviewer AND contradiction-check, a sibling `OllamaChatClient` is added that POSTs to `<base_url>/api/chat` using Ollama's native chat API (NOT the OpenAI-compat shim at `/v1/chat/completions`). The native endpoint is the idiomatic choice — fewer translation hops, AND a long-term-stable contract Ollama publishes per release.

**URL convention preserved across all subsystems**: the operator's `api_base_url` is the API root. Each provider's client knows what path to append:

- `anthropic` → `<base>/v1/messages` (existing behavior, unchanged).
- `openai_compatible` → `<base>/chat/completions` for chat OR `<base>/embeddings` for embeddings (existing behavior, unchanged; operators include `/v1` in the base for hosted services that require it).
- `ollama` → `<base>/api/chat` for chat (NEW), `<base>/api/embed` for embeddings (existing behavior, unchanged).

This means operators using Ollama for the reviewer just point at the bare Ollama host (`http://10.42.11.10:11434`) without the `/v1` workaround — the new Ollama chat client uses Ollama's native path, NOT the OpenAI-compat shim.

**Fail-fast config-load validation** runs the per-subsystem provider check AND the per-provider auth check AT STARTUP, not on first feature trigger. Operators who deploy a misconfigured reviewer don't discover the problem hours later when the first code review fires; they discover it at `systemctl restart autocoder` with a clear message.

**Backward compatibility** via serde aliases. The struct field types change from `RagProvider`/`ReviewerProvider` to `LlmProvider`, but the YAML strings (`anthropic`, `openai_compatible`, `ollama`) are unchanged. Existing config files load identically. The only new failure mode is the per-provider auth validation (operators who currently set `provider: openai_compatible` with a dummy `api_key: "ollama"` AND a base URL pointing at Ollama can EITHER keep the hack working OR migrate to `provider: ollama` with no `api_key` AND a bare base URL — both work after this change).

## Impact

- **Affected specs:**
  - `orchestrator-cli` — ADDED requirements for: the canonical `LlmProvider` enum, per-provider `api_key` semantics, per-subsystem provider-validity rules, AND the fail-fast config-load validation. MODIFIED the existing `canonical_rag:` config-block requirement to reference `LlmProvider` AND to add the new "anthropic rejected with clear message" scenario.
  - `code-reviewer` — ADDED requirement for `reviewer.provider` to accept `ollama` AND for the new `OllamaChatClient` shape. The existing canonical "AI-driven code-quality review" requirement is unchanged in body (the review behavior doesn't depend on which provider is used); the new requirement extends the provider surface.
- **Affected code:**
  - `autocoder/src/config.rs` — new `LlmProvider` enum with serde aliases for `RagProvider` AND `ReviewerProvider` (both retained as type aliases for transition; new code uses `LlmProvider`). `ReviewerConfig`, `RagConfig`, AND `ContradictionCheckLlmConfig` switch their `provider` field types. New `validate_llm_config(provider, api_key, subsystem_name) -> Result<()>` helper called from each subsystem's config-load path.
  - `autocoder/src/llm.rs` — new `OllamaChatClient` (sibling to `AnthropicClient` AND `OpenAiCompatibleClient`) implementing the `LlmClient` trait against Ollama's `/api/chat` endpoint. `build_from_config` matches the new `LlmProvider::Ollama` variant AND constructs the new client.
  - `autocoder/src/rag/embedding.rs` — the existing dispatch (`ollama` → `OllamaEmbedClient`, `openai_compatible` → OpenAI-compat embed client) gains an `anthropic` arm that returns `Err(anyhow!("anthropic does not support embeddings; configure provider: ollama or openai_compatible"))`. Defensive; the config-load check should make this unreachable in practice.
  - Tests across config + llm modules covering the new validation paths AND the new client.
- **Operator-visible behavior:**
  - Operators using Ollama for the reviewer can configure `provider: ollama` with no `api_key` AND a bare base URL. No `/v1` workaround needed.
  - Operators using the existing `openai_compatible`+ollama-shim hack continue to work unchanged.
  - Operators with misconfigured providers (anthropic for RAG, ollama with an api_key, openai_compatible without an api_key) see fail-fast config-load errors with actionable messages at daemon startup.
  - `autocoder install --reconfigure reviewer` (the existing install-wizard subcommand) gains the new `ollama` option in its provider prompt.
- **Backward compatibility:** existing config files parse identically (the YAML strings are unchanged; only the underlying Rust type changes). Existing operators see no behavioral change unless they currently have a misconfigured provider that the new fail-fast validation catches.
- **Dependencies:** none. Independent of every other queued change. Can land in any order.
- **Acceptance:** `cargo test` passes; `openspec validate a37-unify-llm-provider-config --strict` passes. Tests:
  - `LlmProvider` enum round-trips through serde with each variant string (`anthropic`, `openai_compatible`, `ollama`).
  - `RagProvider` AND `ReviewerProvider` type aliases continue to compile (transitional retention).
  - `reviewer.provider: ollama` with no `api_key` AND `api_base_url: http://localhost:11434` loads cleanly.
  - `reviewer.provider: ollama` WITH an `api_key` fails config-load with the documented "Ollama does not authenticate" message.
  - `canonical_rag.provider: anthropic` fails config-load with the documented "canonical_rag does not support provider 'anthropic'" message.
  - `reviewer.provider: openai_compatible` without an `api_key` fails config-load with the documented "openai_compatible requires api_key" message.
  - `OllamaChatClient::complete(prompt)` against a mock Ollama server POSTs to `/api/chat` with the documented payload shape AND returns the parsed completion content.
  - End-to-end: a reviewer invocation against a mock Ollama server returns a `ReviewResult` (no permission-shim, no URL surgery).
  - Backward-compat: an existing config with `reviewer.provider: openai_compatible` + dummy `api_key: "ollama"` + `api_base_url: http://localhost:11434/v1` continues to load AND function (the hack still works for operators who don't migrate).
