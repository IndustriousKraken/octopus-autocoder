## ADDED Requirements

### Requirement: `reviewer.provider` accepts `ollama` as a first-class provider via a new `OllamaChatClient`

The `reviewer.provider` field SHALL accept `ollama` alongside the existing `anthropic` AND `openai_compatible` values (per the orchestrator-cli canonical `LlmProvider` enum). When `provider: ollama`, the reviewer SHALL invoke a new `OllamaChatClient` that POSTs to `<api_base_url>/api/chat` using Ollama's native chat API.

The `OllamaChatClient` SHALL:

- POST to `<api_base_url>/api/chat` (trailing slashes on `api_base_url` are trimmed). NOT to `<api_base_url>/v1/chat/completions` (Ollama's OpenAI-compat shim — operators using `openai_compatible` to point at Ollama AND including `/v1` in their base URL continue to use that path AS A LEGACY OPTION, but the canonical Ollama path is the native one).
- Send body shape `{"model": <model>, "messages": [{"role": "user", "content": <prompt>}], "stream": false}`. The `stream: false` flag SHALL be explicit so Ollama returns a single-response payload (matching the existing `AnthropicClient` AND `OpenAiCompatibleClient` shapes).
- NOT send an `Authorization` header. Ollama does not authenticate; the per-provider auth-semantics requirement REJECTS `api_key` at config-load when `provider: ollama`, so no key is ever in scope to send.
- Parse the response shape `{"message": {"role": "assistant", "content": "<text>"}, "done": true, ...}` AND return the `message.content` string as the completion.
- On non-2xx HTTP status, return `Err` with the status code AND the first 500 characters of the response body (matching the existing `OpenAiCompatibleClient` error shape).
- On 2xx with a malformed-JSON OR schema-mismatched body, return `Err` with a clear decode-failure message naming `OllamaChatClient`.

The `OllamaChatClient` SHALL implement the same `LlmClient` trait that `AnthropicClient` AND `OpenAiCompatibleClient` implement. Reviewer dispatch (`llm::build_from_config`), contradiction-check dispatch, AND any future LLM-using caller dispatch SHALL match the new `LlmProvider::Ollama` variant AND construct the new client.

Operators using Ollama for the reviewer SHALL configure the bare Ollama host URL (e.g. `api_base_url: http://localhost:11434`) WITHOUT the `/v1` suffix. The new client targets Ollama's native path; the `/v1` suffix is only relevant to the legacy `openai_compatible`-pointed-at-Ollama configuration shape.

#### Scenario: `provider: ollama` for reviewer constructs the new client
- **WHEN** `reviewer.provider: ollama` AND `reviewer.api_base_url: http://10.42.11.10:11434` AND `reviewer.model: qwen2.5-coder:32b`
- **AND** the reviewer is invoked for a code review
- **THEN** the underlying `LlmClient` is an `OllamaChatClient`
- **AND** the HTTP POST target is `http://10.42.11.10:11434/api/chat`
- **AND** the request body contains `"model": "qwen2.5-coder:32b"`, `"messages": [...]`, AND `"stream": false`
- **AND** the request does NOT include an `Authorization` header

#### Scenario: `OllamaChatClient` parses successful response into `LlmClient::complete` result
- **WHEN** `OllamaChatClient::complete("review this diff: ...")` is invoked AND the mock Ollama server returns 200 with `{"message":{"role":"assistant","content":"VERDICT: Pass\n\nLooks good."},"done":true}`
- **THEN** the function returns `Ok("VERDICT: Pass\n\nLooks good.")`

#### Scenario: `OllamaChatClient` surfaces non-2xx as actionable error
- **WHEN** the mock Ollama server returns 404 with body `{"error":"model 'nonexistent' not found"}`
- **THEN** `OllamaChatClient::complete` returns `Err` containing `404` AND the first 500 characters of the body

#### Scenario: `OllamaChatClient` surfaces malformed response as actionable error
- **WHEN** the mock Ollama server returns 200 with body `{"unexpected_shape": true}` (no `message.content`)
- **THEN** `OllamaChatClient::complete` returns `Err` with a message naming `OllamaChatClient` AND the decode failure

#### Scenario: Legacy openai_compatible-pointed-at-Ollama config continues to work
- **WHEN** an existing config has `reviewer.provider: openai_compatible`, `reviewer.api_key.value: "ollama"` (dummy), AND `reviewer.api_base_url: http://10.42.11.10:11434/v1`
- **THEN** config-load succeeds (the `openai_compatible` provider requires `api_key`, which is present; the dummy value is accepted)
- **AND** review invocations POST to `http://10.42.11.10:11434/v1/chat/completions` (Ollama's OpenAI-compat shim)
- **AND** Ollama returns a successful response (the shim is functional)
- **AND** the operator can migrate to `provider: ollama` + bare base URL + no api_key at their discretion without behavioral regression
