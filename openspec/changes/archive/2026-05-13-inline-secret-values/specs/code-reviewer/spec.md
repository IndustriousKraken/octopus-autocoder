## MODIFIED Requirements

### Requirement: AI-driven code-quality review
The code-reviewer SHALL accept a unified diff and a change summary, send
them to a configured LLM API, and return a structured
`ReviewReport { verdict, markdown }`. The review SHALL focus on code
quality (security, error handling, naming, style, language idioms,
obvious bugs) and SHALL NOT assess whether the diff correctly implements
any spec — that is a separate verification concern handled in its own
change.

#### Scenario: Successful review with parseable verdict (env-var key)
- **WHEN** `code_reviewer.review(diff, summary)` is called AND the
  configured LLM returns a response whose first non-empty line matches
  `(?i)^VERDICT:\s*(Pass|Concerns|Block)\s*$` AND
  `reviewer.api_key` is unset
- **THEN** the function returns `Ok(ReviewReport { verdict: <parsed value>, markdown: <remainder of response> })`
- **AND** the underlying HTTP call to the LLM API uses the
  `Authorization`/`x-api-key` scheme appropriate to the configured
  provider, with the token sourced from the environment variable named
  in `reviewer.api_key_env`

#### Scenario: Successful review with parseable verdict (inline key)
- **WHEN** `code_reviewer.review(diff, summary)` is called AND
  `reviewer.api_key` is set to `{ value: "..." }`
- **THEN** the underlying HTTP call uses the inline value verbatim as
  the token
- **AND** `reviewer.api_key_env`'s named environment variable is NOT
  consulted, regardless of whether it is set

#### Scenario: Both inline and env-var key set
- **WHEN** `reviewer.api_key` is set AND `reviewer.api_key_env` names an
  env var that is also set
- **THEN** the inline value wins
- **AND** autocoder emits exactly one `warn`-level log line at startup
  noting that `reviewer.api_key` takes precedence and the env var named
  by `reviewer.api_key_env` is being ignored

#### Scenario: Unparseable response
- **WHEN** the LLM response does not begin with a valid `VERDICT:` line
- **THEN** the function returns `Ok(ReviewReport { verdict: Concerns, markdown: "[reviewer response did not include a valid verdict line]\n\n<raw response>" })`

#### Scenario: Diff exceeds size budget
- **WHEN** the input diff exceeds 100,000 characters
- **THEN** the code-reviewer truncates the diff to 100,000 characters
  AND prepends `[diff truncated to 100k chars]` to the truncated
  content before substituting it into the prompt template
- **AND** the resulting `markdown` makes the truncation visible to a
  human reader (the LLM is instructed via the default template to
  acknowledge truncation)

#### Scenario: LLM API failure
- **WHEN** the LLM API returns a non-2xx response or the HTTP request
  errors at the transport layer
- **THEN** `code_reviewer.review` returns `Err(_)` whose text contains
  the response status (or transport error description) and, when the
  response body is available, a snippet of it (truncated to 500 chars)
