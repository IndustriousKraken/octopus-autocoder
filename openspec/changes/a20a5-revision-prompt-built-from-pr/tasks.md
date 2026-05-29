## 1. Revision prompt template

- [ ] 1.1 In `prompts/implementer-revision.md`, replace the existing `{{change_body}}` placeholder with the new set:
  - `{{pr_body}}` — the PR's body verbatim. Contains the code review section, the changes-list section, and any other PR-rendered context.
  - `{{pr_change_list}}` — newline-separated list of change slugs from the PR's "Changes implemented in this pass" line.
  - `{{agent_implementation_notes}}` — concatenated `## Agent implementation notes` comment bodies from the PR, separated by `---\n` between entries.
  - `{{revision_diff}}` — unchanged.
  - `{{revision_request}}` — unchanged.
- [ ] 1.2 Template prose updates:
  - A new opening paragraph explaining that the LLM is revising the work shown in the PR diff, NOT implementing fresh material. The diff contains the implementation AND the spec deltas (the archive moves).
  - Instruction on multi-change resolution: if the operator's `{{revision_request}}` names a specific change by slug, target that change. Otherwise apply the revision to whichever changes the request's content matches.
  - The LLM SHALL leave the workspace dirty for autocoder to commit; do not invoke `git` or `openspec archive` directly.
- [ ] 1.3 Tests: deserialize the template via the existing `a24` PromptLoader pattern; assert all five placeholders are present in the embedded template's text.

## 2. RevisionContext fields

- [ ] 2.1 In `autocoder/src/revisions.rs` (or wherever `RevisionContext` lives — check `executor` module), extend the struct with three new fields:
  ```rust
  pub struct RevisionContext {
      pub change_name: String,         // existing — kept for log routing & state file naming
      pub pr_diff: String,             // existing
      pub revision_text: String,       // existing
      pub pr_body: String,             // new
      pub pr_change_list: String,      // new — newline-separated slugs
      pub agent_implementation_notes: String, // new — concatenated `## Agent implementation notes` bodies
  }
  ```
- [ ] 2.2 Update every constructor of `RevisionContext` to populate the new fields. Tests that mock the context populate them with fixture strings.

## 3. PR-context fetcher

- [ ] 3.1 In `autocoder/src/github.rs`, add a helper `fetch_pr_revision_context`:
  ```rust
  pub async fn fetch_pr_revision_context(
      api_base: &str,
      token: &str,
      owner: &str,
      repo: &str,
      pr_number: u64,
  ) -> Result<PrRevisionContext>;

  pub struct PrRevisionContext {
      pub body: String,                        // PR body
      pub agent_implementation_notes: String,  // concatenated `## Agent implementation notes` comments
  }
  ```
- [ ] 3.2 Implementation:
  - Fetch the PR body via `GET /repos/{owner}/{repo}/pulls/{n}` (or reuse existing PR-fetch path if it already returns the body — check `list_open_prs_for_head`'s response shape; `PrSummary` already has `body: Option<String>`, so it may suffice).
  - Fetch all PR comments via the existing `list_issue_comments_since(..., None)` (passing `None` for the `since` argument to get everything).
  - Filter comments to those whose body starts with `## Agent implementation notes` (case-sensitive; matches the canonical "Implementer-summary PR comment" requirement's exact heading).
  - Concatenate matched comment bodies in posted-order, separated by `\n\n---\n\n`.
- [ ] 3.3 Tests via mockito:
  - Single-change PR: one matching comment → its body is returned verbatim.
  - Multi-change PR: two matching comments → concatenated with the separator.
  - No matching comments (revise on a PR before notes posted): returns an empty string. The revision proceeds with an empty `{{agent_implementation_notes}}` — the LLM still has spec deltas + diff + revise text.
  - API error fetching the PR: returns `Err`. Caller posts the failure comment.
  - API error fetching comments: returns `Err`. Same handling.

## 4. Dispatcher integration

- [ ] 4.1 In `autocoder/src/revisions.rs::process_one_pr`, replace the change-name → `openspec instructions apply` call with `fetch_pr_revision_context(...)`. Populate the new `RevisionContext` fields from the result.
- [ ] 4.2 If `fetch_pr_revision_context` returns `Err`, post a clear failure comment AND do NOT advance the comment-seen marker:
  ```
  ✗ Cannot revise: failed to fetch PR context: <truncated-error-message>. The daemon will retry on the next polling iteration. If this persists, check journalctl for the daemon's GitHub API errors AND verify the bot's token has Read access on this repo.
  ```
  This guarantees transient API errors don't lose the revise comment.
- [ ] 4.3 The existing change-name extraction from PR body (`extract_change_list_from_pr_body`) continues to drive the per-PR state file naming AND the run-log path; the prompt template's `{{pr_change_list}}` value uses the same data.
- [ ] 4.4 Tests:
  - Happy path: mock PR + comments → dispatcher calls executor with all fields populated → `Completed` → commit + push + reply.
  - PR-context fetch failure: dispatcher posts the failure comment AND state's `last_seen_comment_ts` is UNCHANGED.
  - Reviser-initiated revision (the `<!-- reviewer-revision -->` flow): same fetch path; LLM gets the same five inputs.

## 5. build_revision_prompt rewrite

- [ ] 5.1 In `autocoder/src/executor/claude_cli.rs::build_revision_prompt`:
  - Delete the `openspec instructions apply` invocation entirely. This is the wrong source for revision-mode regardless of any failure handling.
  - Delete the `_(original change material unavailable — ...)_` fallback string entirely.
  - Delete the catch-fallback `match out` block.
  - Replace with a single `replace` chain that substitutes `{{pr_body}}`, `{{pr_change_list}}`, `{{agent_implementation_notes}}`, `{{revision_diff}}`, AND `{{revision_request}}` from the `RevisionContext` directly into the template.
- [ ] 5.2 Tests:
  - `build_revision_prompt_substitutes_all_placeholders` (existing — at `claude_cli.rs:2806` per the trace I did earlier): updated to assert the new five placeholders are substituted AND none of the pre-spec strings (`{{change_body}}`, `original change material unavailable`) appears in the output.
  - A new test confirms the function does NOT call any subprocess (verify by capturing process spawns OR by asserting no `Command::new("openspec")` invocation happens).

## 6. Spec deltas

- [ ] 6.1 `openspec/changes/a20a5-revision-prompt-built-from-pr/specs/executor/spec.md` ADDs the `Revision prompt is constructed from PR-sourced material; no degraded-prompt fallback is permitted` requirement covering the template placeholders, the PR-derived input requirement, AND the no-fallback invariant.
- [ ] 6.2 `openspec/changes/a20a5-revision-prompt-built-from-pr/specs/orchestrator-cli/spec.md` ADDs the `Revise dispatcher refuses to invoke the executor when PR-context assembly fails` requirement covering the failure-comment path AND the no-marker-advance-on-transient-failure rule.

## 7. Verification

- [ ] 7.1 `cargo test --bin autocoder` passes — new tests + existing tests after template + context migration.
- [ ] 7.2 `openspec validate a20a5-revision-prompt-built-from-pr --strict` passes.
- [ ] 7.3 `cargo clippy --bin autocoder` produces no new warnings in `executor/claude_cli.rs`, `revisions.rs`, `github.rs`, OR `prompts/implementer-revision.md` at lines I added/modified.
- [ ] 7.4 Manual verification on the live daemon AFTER deploy:
  - On any open autocoder-opened PR (after `a20a4` ships so the dispatcher actually finds the PR), comment `@<bot> revise add a clarifying sentence to the proposal explaining the failure mode this fix addresses`.
  - Within one polling cycle: observe the revision attempt log entry, the `✅ Revision applied: ...` reply on the PR, AND a new force-push to the agent branch with a substantive edit that references the spec material AND the operator's text.
  - Inspect the per-change run log's PROMPT section in the summary log (per `a20a2`): assert it contains the original `## Agent implementation notes` text AND does NOT contain the legacy `_(original change material unavailable...)_` string.
