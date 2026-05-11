## 1. Config additions

- [ ] 1.1 Add `reviewer: Option<ReviewerConfig>` to the top-level `Config`. Apply `#[serde(deny_unknown_fields)]`.
- [ ] 1.2 `ReviewerConfig` fields: `enabled: bool` (default false), `provider: ReviewerProvider` (enum: `Anthropic`, `OpenAiCompatible`), `model: String`, `api_key_env: String`, optional `api_base_url: String`, optional `prompt_template_path: PathBuf`.
- [ ] 1.3 Update `config.example.yaml` to demonstrate the reviewer block (commented out by default to make opt-in clear).
- [ ] 1.4 **Verify:** `cargo test config::tests::loads_with_reviewer`, `config::tests::reviewer_disabled_by_default` (absent block parses to `None`).

## 2. LLM client abstraction

- [ ] 2.1 Create `src/llm.rs` with `pub trait LlmClient: Send + Sync { async fn complete(&self, prompt: &str) -> Result<String> }`.
- [ ] 2.2 Implement `pub struct AnthropicClient` POSTing to `https://api.anthropic.com/v1/messages` with header `x-api-key: <token>`, `anthropic-version: 2023-06-01`, body `{ model, max_tokens, messages: [{ role: "user", content: prompt }] }`. Returns the first `content` block's `text` field.
- [ ] 2.3 Implement `pub struct OpenAiCompatibleClient` POSTing to `<api_base_url>/chat/completions` with header `Authorization: Bearer <token>`, body `{ model, messages: [{ role: "user", content: prompt }] }`. Returns `choices[0].message.content`.
- [ ] 2.4 Implement `pub fn build_from_config(cfg: &ReviewerConfig) -> Result<Box<dyn LlmClient>>` that constructs the correct client per `provider`.
- [ ] 2.5 **Verify:** `cargo test llm::tests::*` against `mockito` HTTP fixtures: each provider serializes the right request body, parses the right response shape, surfaces non-2xx as errors whose text contains the status code.

## 3. Code reviewer module

- [ ] 3.1 Create `src/code_reviewer.rs` with `pub struct CodeReviewer { client: Box<dyn LlmClient>, template: String }`.
- [ ] 3.2 Implement `pub async fn review(&self, diff: &str, change_summary: &str) -> Result<ReviewReport>`. Substitutes `{{diff}}` and `{{change_summary}}` in the template, calls `client.complete`, parses the response.
- [ ] 3.3 Parse the response: the first non-empty line MUST match the regex `(?i)^VERDICT:\s*(Pass|Concerns|Block)\s*$`. If matched, set the verdict and use the rest as `markdown`. If unmatched, set verdict to `Concerns` and prepend `[reviewer response did not include a valid verdict line]\n\n` to the raw response.
- [ ] 3.4 If `diff.len() > 100_000`, truncate to 100,000 chars and prepend `[diff truncated to 100k chars]\n` before substitution.
- [ ] 3.5 Ship `prompts/code-review-default.md` containing the default template. The template MUST contain the scope-statement line: `"You are reviewing code quality only. Do NOT assess whether the diff implements the spec; that is handled separately by the verifier step."` MUST contain the verdict format instruction with example. MUST list rubric points: security, error handling, naming, style, language idioms, dead code, obvious bugs.
- [ ] 3.6 **Verify:** `cargo test code_reviewer::tests::parses_pass_verdict`, `parses_block_verdict`, `case_insensitive_verdict`, `defaults_to_concerns_on_unparseable`, `truncates_huge_diff`, `substitutes_template_variables`.

## 4. PR creation integration

- [ ] 4.1 Update `src/github.rs` `create_pull_request` signature: add `review_report: Option<&ReviewReport>` and `draft: bool` parameters.
- [ ] 4.2 If `review_report` is `Some(r)`, append `\n\n## Code Review\n\n{r.markdown}` to the PR body. Otherwise leave body unchanged.
- [ ] 4.3 If `draft: true`, include `"draft": true` in the JSON body.
- [ ] 4.4 If the PR creation response indicates the `draft` field is unsupported (specific GitHub error message or status), retry the request with `draft: false`, then on success POST to `https://api.github.com/repos/<owner>/<repo>/issues/<pr_number>/labels` with body `{ "labels": ["do-not-merge"] }`. Log that the label fallback was applied.
- [ ] 4.5 **Verify:** `cargo test github::tests::body_includes_review_section`, `github::tests::draft_flag_serialized`, `github::tests::label_fallback_on_draft_unsupported`.

## 5. Orchestrator wiring

- [ ] 5.1 In `execute_one_pass`, after the queue walk completes and BEFORE `git::push_force_with_lease`:
  - If `config.reviewer` is `None` or `enabled: false`: set `review_report = None` and `draft = false`.
  - Otherwise: capture the diff via a new `git::diff_three_dot(workspace, base, head) -> Result<String>` helper. Build the change summary text (a bulleted list of archived change names). Call `reviewer.review(&diff, &summary).await`. On `Ok(report)`: set `review_report = Some(report)` and `draft = matches!(report.verdict, ReviewVerdict::Block)`. On `Err(e)`: log `"reviewer failed: {e:#}"`, build a synthetic report with `verdict: Concerns` and `markdown: format!("(reviewer failed: {e})")`, set `review_report = Some(synthetic)` and `draft = false`.
- [ ] 5.2 Pass `review_report.as_ref()` and `draft` through to `github::create_pull_request`.
- [ ] 5.3 **Verify:** Unit test `polling_loop::tests::reviewer_block_marks_pr_draft` using a fixture reviewer that returns each `ReviewVerdict` variant in turn. Assert: (a) `Approve` and `Concerns` produce a non-draft PR request with a `## Code Review` section in the body; (b) `Block` produces a draft PR request with the same section; (c) reviewer error path injects the synthetic-`Concerns` report and `draft: false`. PR request shape is observed via the existing `mockito` harness from `github::tests`.

## 6. Documentation

- [ ] 6.1 Update `README.md` with a "Code Review" section: scope (code quality, NOT spec compliance), the config block, the recommended branch-protection settings to enforce draft state.
- [ ] 6.2 Document `prompts/code-review-default.md` and that users can override via `reviewer.prompt_template_path`. Note explicitly: custom templates are user-owned; the project does not enforce scope on overrides.
