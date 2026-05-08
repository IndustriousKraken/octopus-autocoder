## Context

The verification architecture for this project is three orthogonal layers (per the `verification_architecture` design memo): verifier (spec audit, future change), reviewer (code quality, this change), drift audit (periodic whole-repo, future change). This change implements the reviewer layer. The scope is intentionally narrow — "is the code good code?" — and explicitly does NOT include "did the diff implement the spec?" That second question is the verifier's responsibility in a separate change.

## Goals / Non-Goals

**Goals:**
- A `code-reviewer` module that accepts a unified diff and returns `ReviewReport { verdict, markdown }`.
- PR-time integration: the report appears in the PR body under `## Code Review`; `Block` verdict produces a draft PR.
- Configurable LLM provider/model so the user can choose Claude, GPT-4, or any OpenAI-compatible endpoint (Grok, OpenRouter, local Ollama, etc.).
- Opt-in via config block; absent or `enabled: false` block means the reviewer step is skipped entirely.
- A default prompt template that explicitly enforces the code-quality-only scope.

**Non-Goals:**
- Spec compliance checks. The default prompt is firm about this; user-supplied templates are user-owned.
- Auto-rejection or auto-modification of code. The reviewer reports; humans decide.
- Multiple reviewers in parallel and verdict aggregation. One reviewer per pass.
- Streaming or incremental review. The full diff is sent in one request.
- Inline PR comments. The report goes in the PR body only.

## Decisions

- **Verdict shape:**

  ```rust
  pub enum ReviewVerdict { Pass, Concerns, Block }
  pub struct ReviewReport { verdict: ReviewVerdict, markdown: String }
  ```

  The reviewer is required to emit a verdict line at the start of its response: `VERDICT: Pass`, `VERDICT: Concerns`, or `VERDICT: Block` (case-insensitive). The remainder is parsed into `markdown`. If the verdict line is absent or unparseable, the orchestrator defaults the verdict to `Concerns` and prepends a parse-failure note to the markdown.
- **Default prompt template (`prompts/code-review-default.md`):** firm scope statement at the top — *"You are reviewing code quality only. Do NOT assess whether the diff implements the spec; that is handled separately by the verifier step."* Rubric points: security (injection, auth, secrets, deserialization), error handling, naming, style, language idioms, dead code, obvious bugs. Format requirement: respond with a `VERDICT:` line followed by markdown bullets grouped by rubric point.
- **Diff extraction:** `git diff <base_branch>...<agent_branch>` (three-dot syntax: changes on agent_branch since divergence from base_branch). Captured as a `String` and substituted into the prompt template.
- **Provider abstraction:** `src/llm.rs` exposes `pub trait LlmClient: Send + Sync { async fn complete(&self, prompt: &str) -> Result<String> }`. Two concrete implementations: `AnthropicClient` (calls `https://api.anthropic.com/v1/messages`) and `OpenAiCompatibleClient` (calls `<api_base_url>/chat/completions`, default base url `https://api.openai.com/v1`).
- **Config block:**

  ```yaml
  reviewer:
    enabled: true
    provider: anthropic   # or openai_compatible
    model: claude-sonnet-4-6
    api_key_env: ANTHROPIC_API_KEY
    api_base_url: https://api.anthropic.com  # optional; provider-default if omitted
    prompt_template_path: ./prompts/code-review-default.md  # optional; built-in default if omitted
  ```

- **PR body composition:** when reviewer is enabled, the body has two sections: (1) the existing list of archived changes; (2) `## Code Review` containing the reviewer's `markdown`. When the reviewer is disabled, only section 1 is included. When the reviewer fails, section 2 contains only `(reviewer failed: <reason>)` so operators see why.
- **Block → draft PR:** on `Block` verdict, the PR creation request sets `draft: true` in the GitHub API call. If the API rejects the `draft` flag (older API, unsupported repo type), the orchestrator falls back to creating the PR with `draft: false` and immediately applying a `do-not-merge` label via the GitHub issues-labels endpoint. The fallback is logged.
- **Failure non-fatal:** if the reviewer's API call errors (network, auth, model error), the orchestrator logs the error, treats it as `verdict: not-applicable`, and PROCEEDS with PR creation. A failed reviewer must not block the orchestrator from delivering the PR.
- **Diff size budget:** if the diff exceeds 100,000 characters, the reviewer truncates with a `[diff truncated to 100k chars]` header and proceeds. Truncated diffs are biased toward `Concerns` because the reviewer cannot see the full picture; the prompt template instructs the model accordingly.

## Risks / Trade-offs

- **Risk:** Reviewer scope drifts back into spec compliance over time as users edit prompts.
  - **Mitigation:** The default prompt is firm. Custom templates are user-owned; if the user wants spec audit they can do it, but the project default scope is clear and documented.
- **Risk:** Reviewer API key leaks via log lines.
  - **Mitigation:** Token loaded from env var named in `reviewer.api_key_env`. Never logged. Request bodies not logged.
- **Risk:** Block-verdict PRs are quietly merged by anyone with write access who flips draft state.
  - **Mitigation:** The orchestrator does its part (sets draft / applies label). Enforcement requires a repo branch-protection rule, which is the user's responsibility. README documents the recommended config.
- **Risk:** Long diffs exceed the model's context window.
  - **Mitigation:** Truncation at 100k chars with a clear note. Users can switch to larger-context models if their reviews are routinely truncated.
- **Risk:** A failed reviewer silently blocks PR creation.
  - **Mitigation:** Failure is non-fatal: PR is still created with a "(reviewer failed)" note. Operators can inspect logs.
- **Risk:** False sense of security — Block verdict feels stronger than it is.
  - **Mitigation:** README is explicit that the reviewer is one of three layers; spec correctness is the verifier's job (when built); humans remain authoritative.
