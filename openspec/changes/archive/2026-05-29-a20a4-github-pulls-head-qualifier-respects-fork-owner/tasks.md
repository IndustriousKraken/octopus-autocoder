## 1. Signature change in `github.rs`

- [x] 1.1 `list_open_prs_for_head` gained `head_owner: &str` between `repo` AND `head_branch`. Internal `head_qualified` construction switched from `format!("{owner}:{head_branch}")` to `format!("{head_owner}:{head_branch}")`. Added doc comment naming the head_owner-from-fork_owner rule AND linking the pattern to `polling_loop.rs::open_pr_exists_for_agent_branch_at`.
- [x] 1.2 `latest_pr_for_head` got the same treatment with the same construction fix.
- [x] 1.3 Doc comments on both helpers explicitly call out the fork-PR-mode regression they fix.

## 2. Call-site updates

- [x] 2.1 `revisions.rs::process_revision_requests_at` computes `head_owner = github_cfg.fork_owner.as_deref().unwrap_or(&owner)` AND passes it to `list_open_prs_for_head`. Inline comment names the a20a4 regression context AND the pre-fix symptom (revise non-functional in fork-PR mode).
- [x] 2.2 `control_socket.rs::fetch_latest_pr` got the parallel fix. Refactored into `fetch_latest_pr` + `fetch_latest_pr_at(api_base, ...)` so the test layer can drive mockito; the public wrapper calls `DEFAULT_API_BASE`.
- [x] 2.3 No change at `polling_loop.rs::open_pr_exists_for_agent_branch_at` — that site already constructs the qualifier correctly via `list_open_prs` (which takes a pre-built `head` string).

## 3. Unit tests for the helpers

- [x] 3.1 Added `list_open_prs_for_head_uses_head_owner_param_for_qualifier` in `github.rs::tests`. mockito mocks `head=fork-owner:agent-q` exactly; the test passes `owner="upstream-owner"` AND `head_owner="fork-owner"`. Pre-fix code would produce `head=upstream-owner:agent-q`, mockito would not match, AND `.expect(1)` would fail.
- [x] 3.2 Added `latest_pr_for_head_uses_head_owner_param_for_qualifier` covering the status helper. Same shape; mockito matchers cover all five query params (head, state, sort, direction, per_page).
- [x] 3.3 12 existing callers migrated by inserting `"owner"` for the new `head_owner` parameter (preserves direct-push semantics since `head_owner == owner` in that mode). All pre-existing tests pass unchanged.

## 4. Integration tests at the call-site layer

- [x] 4.1 Added `revisions::tests::dispatcher_finds_pr_in_fork_pr_mode`. Configures `gh.fork_owner = Some("fork-acc")`. mockito mocks `/repos/owner/repo/pulls?head=fork-acc:agent-q` AND `/repos/owner/repo/issues/99/comments`. Drives `process_revision_requests_at`. Asserts BOTH mocks fire — the pulls-list mock proves the dispatcher used the right qualifier, the comments mock proves the dispatcher proceeded past the empty-list early-return.
- [x] 4.2 Added `control_socket::tests::status_shows_latest_pr_in_fork_pr_mode`. Tests via the new `fetch_latest_pr_at` test entry point. Strict mockito head matcher + assertion that the returned `Option` is `Some(PrSummary { number: 99, .. })`.
- [x] 4.3 Both tests verified by construction: the mockito `.expect(1)` assertion fails against the pre-fix call shape because the wrong head qualifier means the mock never matches.

## 5. Spec deltas

- [x] 5.1 `openspec/changes/a20a4-github-pulls-head-qualifier-respects-fork-owner/specs/orchestrator-cli/spec.md` ADDs the `GitHub pulls filter-by-head queries use fork_owner as the head qualifier owner in fork-PR mode` requirement with 5 scenarios. Validated strict.

## 6. Verification

- [x] 6.1 `cargo test --bin autocoder`: 1674 passed in touched modules + adjacent. Two pre-existing failures in `cli::install::tests::wizard_rag_*` (a21 RAG install-wizard tests) are unrelated to a20a4 — touched modules `github.rs` (58 tests), `revisions.rs` (34), `control_socket.rs` (37) all green.
- [x] 6.2 `openspec validate a20a4-github-pulls-head-qualifier-respects-fork-owner --strict` passes.
- [x] 6.3 `cargo build --release` clean. Clippy on touched files produces no new warnings.
- [ ] 6.4 Manual verification on the live daemon — deferred to operator after the daemon picks up this change. Expected: `@<bot> status autocoder` shows PR #69 (number + URL) instead of `(none)`; existing `revise` comment on PR #69 is processed within one polling cycle.
