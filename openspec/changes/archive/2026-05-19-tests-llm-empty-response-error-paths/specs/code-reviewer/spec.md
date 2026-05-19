## ADDED Requirements

### Requirement: LLM client surfaces an actionable error when a 2xx response is unusable
The code-reviewer's LLM-client layer (`AnthropicClient`, `OpenAiCompatibleClient`) SHALL distinguish "transport failed / HTTP error" (already covered by the non-2xx scenarios) from "transport succeeded but the response body cannot be turned into review text". For the latter case, the client SHALL return `Err(_)` whose message names the specific shape problem so the operator can tell from logs whether to retry, switch model, or escalate.

#### Scenario: Anthropic returns 2xx with no text content block
- **WHEN** an `AnthropicClient::complete` call gets a `200` response whose `content` array contains only non-text blocks (e.g. only `image` or `tool_use` entries)
- **THEN** the call returns `Err(_)` whose `format!("{err:#}")` contains a substring naming the missing-text-block condition (e.g. `no text block`)
- **AND** the error message does NOT claim the HTTP call failed (preserving the operator's ability to tell shape errors from transport errors in logs)

#### Scenario: Anthropic returns 2xx with unparseable JSON body
- **WHEN** an `AnthropicClient::complete` call gets a `200` whose body is not valid JSON of shape `AnthropicResponse`
- **THEN** the call returns `Err(_)` whose message contains a substring naming the decode failure (e.g. `decode failed`)

#### Scenario: OpenAI-compatible returns 2xx with empty choices array
- **WHEN** an `OpenAiCompatibleClient::complete` call gets a `200` with body `{"choices":[]}`
- **THEN** the call returns `Err(_)` whose message contains a substring naming the empty-choices condition (e.g. `no choices`)

#### Scenario: OpenAI-compatible returns 2xx with unparseable JSON body
- **WHEN** an `OpenAiCompatibleClient::complete` call gets a `200` whose body is not valid JSON of shape `OpenAiResponse`
- **THEN** the call returns `Err(_)` whose message contains a substring naming the decode failure (e.g. `decode failed`)
