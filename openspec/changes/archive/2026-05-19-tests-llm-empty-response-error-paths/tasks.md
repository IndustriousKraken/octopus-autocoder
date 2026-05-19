## 1. Add post-2xx-shape error tests for the Anthropic client

- [x] 1.1 `anthropic_errors_when_response_contains_no_text_block` — start a
  mockito server, register a `POST /v1/messages` mock that returns `200`
  with body `{"content":[{"type":"image","source":{"type":"base64","data":"x"}}]}`,
  call `AnthropicClient::complete("hi")`, assert the result is an `Err`
  whose `format!("{err:#}")` contains the substring `no text block`.
- [x] 1.2 `anthropic_errors_when_response_body_is_unparseable_json` —
  register a `POST /v1/messages` mock that returns `200` with body
  `not-json`, call `complete("hi")`, assert the result is an `Err`
  whose message contains the substring `decode failed`.

## 2. Add post-2xx-shape error tests for the OpenAI-compatible client

- [x] 2.1 `openai_compatible_errors_when_choices_array_is_empty` —
  register `POST /chat/completions` returning `200` with body
  `{"choices":[]}`, call `OpenAiCompatibleClient::complete("hi")`,
  assert the result is an `Err` whose message contains
  `no choices`.
- [x] 2.2 `openai_compatible_errors_when_response_body_is_unparseable_json`
  — register `POST /chat/completions` returning `200` with body
  `not-json`, call `complete("hi")`, assert the error message contains
  `decode failed`.
