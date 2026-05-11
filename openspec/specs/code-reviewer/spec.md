# code-reviewer Specification

## Purpose
TBD - created by archiving change reviewer-integration. Update Purpose after archive.
## Requirements
### Requirement: AI-driven code-quality review
The code-reviewer SHALL accept a unified diff and a change summary, send them to a configured LLM API, and return a structured `ReviewReport { verdict, markdown }`. The review SHALL focus on code quality (security, error handling, naming, style, language idioms, obvious bugs) and SHALL NOT assess whether the diff correctly implements any spec — that is a separate verification concern handled in its own change.

#### Scenario: Successful review with parseable verdict
- **WHEN** `code_reviewer.review(diff, summary)` is called AND the configured LLM returns a response whose first non-empty line matches `(?i)^VERDICT:\s*(Pass|Concerns|Block)\s*$`
- **THEN** the function returns `Ok(ReviewReport { verdict: <parsed value>, markdown: <remainder of response> })`
- **AND** the underlying HTTP call to the LLM API uses the `Authorization`/`x-api-key` scheme appropriate to the configured provider, with the token sourced from the environment variable named in `reviewer.api_key_env`

#### Scenario: Unparseable response
- **WHEN** the LLM response does not begin with a valid `VERDICT:` line
- **THEN** the function returns `Ok(ReviewReport { verdict: Concerns, markdown: "[reviewer response did not include a valid verdict line]\n\n<raw response>" })`

#### Scenario: Diff exceeds size budget
- **WHEN** the input diff exceeds 100,000 characters
- **THEN** the code-reviewer truncates the diff to 100,000 characters AND prepends `[diff truncated to 100k chars]` to the truncated content before substituting it into the prompt template
- **AND** the resulting `markdown` makes the truncation visible to a human reader (the LLM is instructed via the default template to acknowledge truncation)

#### Scenario: LLM API failure
- **WHEN** the LLM API returns a non-2xx response or the HTTP request errors at the transport layer
- **THEN** `code_reviewer.review` returns `Err(_)` whose text contains the response status (or transport error description) and, when the response body is available, a snippet of it (truncated to 500 chars)

### Requirement: Default prompt template enforces code-quality scope
The code-reviewer SHALL ship a default prompt template that explicitly limits the review to code-quality concerns and instructs the LLM not to assess spec compliance.

#### Scenario: Default template is shipped with the binary
- **WHEN** autocoder binary is built
- **THEN** a file named `prompts/code-review-default.md` is included in the project repository at the relative path `prompts/code-review-default.md`
- **AND** the template's text contains the literal scope statement: `"You are reviewing code quality only. Do NOT assess whether the diff implements the spec; that is handled separately by the verifier step."`
- **AND** the template specifies the required response format: a verdict line followed by markdown bullets

#### Scenario: User-provided template overrides default
- **WHEN** `reviewer.prompt_template_path` is set in config
- **THEN** the code-reviewer reads the template from that path at startup and uses it instead of the default
- **AND** if the path does not exist or fails to read, startup returns a `Err(_)` naming the path
- **AND** no scope enforcement is performed on user-supplied templates (custom templates are user-owned)

