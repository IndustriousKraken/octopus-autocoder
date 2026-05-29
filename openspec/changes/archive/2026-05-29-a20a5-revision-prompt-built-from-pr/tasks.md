## 1. Revision prompt template

- [x] 1.1 Rewrote `prompts/implementer-revision.md` with new five-placeholder set: `{{pr_body}}`, `{{pr_change_list}}`, `{{agent_implementation_notes}}`, `{{revision_diff}}`, `{{revision_request}}`. Removed `{{change_body}}` entirely.
- [x] 1.2 Template prose updated: opening paragraph names PR-as-source-of-truth; multi-change resolution guidance (name-match first, content-match otherwise, apply-to-all for generic requests); explicit instruction to leave workspace dirty and not invoke `git`/`openspec archive`.
- [x] 1.3 Template parse verified by the regression test (`build_revision_prompt_substitutes_all_placeholders`) which asserts all five placeholders appear in the rendered output.

## 2. RevisionContext fields

- [x] 2.1 Extended `RevisionContext` struct in `autocoder/src/revisions.rs:54` with three new fields: `pr_body: String`, `pr_change_list: String`, `agent_implementation_notes: String`. Existing fields unchanged.
- [x] 2.2 Updated the constructor in `execute_revision` to populate all six fields. Tests that construct `RevisionContext` directly (`build_revision_prompt_substitutes_all_placeholders`, `build_revision_prompt_does_not_invoke_openspec`) populate the new fields with fixture strings.

## 3. PR-context fetcher

- [x] 3.1 Decided against a separate `fetch_pr_revision_context` helper — the PR body is already in scope at the dispatch site (`pr.body` on `PrSummary`), `extract_change_list_from_pr_body` already exists, AND `list_issue_comments_since` already exists. Added `extract_agent_implementation_notes(comments: &[IssueComment]) -> String` helper in `revisions.rs` that filters by `## Agent implementation notes` exact-prefix match AND concatenates with `\n\n---\n\n` separator.
- [x] 3.2 The dispatcher fetches all-time PR comments by calling `list_issue_comments_since(..., chrono::DateTime::<Utc>::UNIX_EPOCH)` lazily (only when a trigger comment is detected — not on every iteration). The body is already available; the change list is already extracted.
- [x] 3.3 Tests in `revisions::tests`:
  - `extract_agent_notes_returns_single_match` — one matching comment returned verbatim.
  - `extract_agent_notes_concatenates_multiple_with_separator` — two matches joined with the canonical separator.
  - `extract_agent_notes_returns_empty_when_no_matches` — three comments, none match → empty string. The revision still proceeds (the LLM has diff + body + revision text).
  - `extract_agent_notes_requires_exact_heading_prefix` — indented headings, lowercase variants, and apostrophe variants do NOT match. Guards against false-matches that would confuse the LLM.

## 4. Dispatcher integration

- [x] 4.1 In `revisions.rs::process_one_pr`, before calling `execute_revision`: fetch all-time comments via `list_issue_comments_since(..., UNIX_EPOCH)` AND extract the agent implementation notes via the new helper. Build `pr_body` from `pr.body.clone().unwrap_or_default()`. Build `pr_change_list` from `change_list.join("\n")` (reusing the same `extract_change_list_from_pr_body` result the per-PR state-naming uses). Pass all three to `execute_revision`.
- [x] 4.2 On comments-fetch `Err`: post the canonical refusal comment (`✗ Cannot revise: failed to fetch PR context: <truncated-error>. The daemon will retry on the next polling iteration. ...`), `break` out of the trigger loop, AND do NOT advance `latest_seen` (the comment-seen marker) AND do NOT call `write_state`. The next polling iteration's dispatcher re-attempts the assembly.
- [x] 4.3 The existing change-name extraction (`extract_change_list_from_pr_body(pr.body.as_deref())`) continues to drive per-PR state-file naming AND log routing; the new `pr_change_list` field uses the same data formatted for the LLM.
- [x] 4.4 Verification via existing tests: the 13 `dispatcher_*` integration tests in `revisions::tests` all continue to pass (their `Matcher::Any` query matchers accept the additional `list_issue_comments_since` call). The new `extract_agent_*` unit tests cover the new logic in isolation.

## 5. build_revision_prompt rewrite

- [x] 5.1 Replaced `build_revision_prompt` in `executor/claude_cli.rs:312`:
  - Removed the `std::process::Command::new("openspec")` invocation entirely.
  - Removed the placeholder fallback strings (`_(original change material unavailable — ...)_`) entirely.
  - Removed the `match out` error-handling block entirely.
  - Function body is now a single `.replace()` chain substituting all five new placeholders from the `RevisionContext`. Added three new placeholder constants: `REVISION_PR_BODY_PLACEHOLDER`, `REVISION_PR_CHANGE_LIST_PLACEHOLDER`, `REVISION_AGENT_NOTES_PLACEHOLDER`.
  - Updated doc comment to describe the a20a5 architecture AND name the canonical invariant (no degraded-prompt path).
- [x] 5.2 Tests in `executor::claude_cli::tests`:
  - `build_revision_prompt_substitutes_all_placeholders` — rewritten. Asserts: all five context fields appear in output (DIFF_HERE, REVISION_HERE, PR_BODY_HERE, a17-foo, a18-bar, AGENT_NOTES_HERE); all five new section markers present (CHANGES IN THIS PR, PR BODY, ORIGINAL AGENT IMPLEMENTATION NOTES, PR DIFF, REVISION REQUEST); no legacy/forbidden strings appear (no placeholder fallback string, no `{{change_body}}`, no unrendered new placeholders, no pre-a20a5 `BEGIN ORIGINAL CHANGE` marker).
  - `build_revision_prompt_does_not_invoke_openspec` — new regression test asserting the output never contains the pre-a20a5 subprocess-reference strings under any input.

## 6. Spec deltas

- [x] 6.1 `openspec/changes/a20a5-revision-prompt-built-from-pr/specs/executor/spec.md` ADDs two requirements: `Revision prompt is constructed from PR-sourced material; no degraded-prompt fallback is permitted` (with 4 scenarios) AND `Prompt construction is gated by an explicit availability check at the caller` (with 2 scenarios codifying the architectural invariant against future regressions).
- [x] 6.2 `openspec/changes/a20a5-revision-prompt-built-from-pr/specs/orchestrator-cli/spec.md` ADDs `Revise dispatcher refuses to invoke the executor when PR-context assembly fails` (with 6 scenarios covering happy-path, fetch failures, persistence, no-notes case, AND the no-fallback invariant).

## 7. Verification

- [x] 7.1 `cargo test --bin autocoder revisions::` passes (38 tests, including 4 new extract_agent_notes tests); `cargo test --bin autocoder executor::claude_cli::tests` passes (63 tests, including 2 new revision-prompt tests).
- [x] 7.2 `openspec validate a20a5-revision-prompt-built-from-pr --strict` passes.
- [x] 7.3 `cargo build --release` clean. `cargo clippy --bin autocoder` produces no new warnings in touched files (added one `#[allow(clippy::too_many_arguments)]` on `execute_revision` matching the established codebase pattern for `handle_message_with_context`-style call sites; the pre-existing format-string warnings at lines 660, 720, 756 are unchanged).
- [x] 7.4 Two pre-existing failures in `cli::install::tests::wizard_rag_*` (a21 RAG install-wizard tests) are unrelated to a20a5 — touched modules `revisions.rs` AND `executor/claude_cli.rs` are fully green.
- [ ] 7.5 Manual verification on the live daemon (after deploy of a20a4 + a20a5): comment `@<bot> revise add a clarifying sentence to the proposal` on any open autocoder-opened PR. Within one polling cycle, observe: the revision attempt log entry; the `✅ Revision applied: ...` reply on the PR; a new force-push to the agent branch with substantive edit referencing the spec material AND the operator's text. The per-change run log's PROMPT section in the summary log (per a20a2) should contain the original `## Agent implementation notes` text AND NOT contain the legacy `_(original change material unavailable...)_` placeholder.
