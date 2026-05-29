## 1. Signature change in `github.rs`

- [ ] 1.1 In `autocoder/src/github.rs`, change `list_open_prs_for_head` (around line 811) signature to add `head_owner: &str` as the second-to-last parameter:
  ```rust
  pub async fn list_open_prs_for_head(
      api_base: &str,
      token: &str,
      owner: &str,        // upstream, used in URL path
      repo: &str,
      head_owner: &str,   // NEW: fork owner in fork-PR mode, upstream in direct mode
      head_branch: &str,
  ) -> Result<Vec<PrSummary>>
  ```
  Update the internal construction at line 819 to `format!("{head_owner}:{head_branch}")`.
- [ ] 1.2 Change `latest_pr_for_head` (around line 109) signature with the same `head_owner: &str` insertion. Update the internal construction at line 135 to `format!("{head_owner}:{head_branch}")`.
- [ ] 1.3 Add a doc-comment block above each function naming the head-owner-from-fork_owner rule AND linking the caller-construction pattern to `polling_loop.rs::open_pr_exists_for_agent_branch_at` (the existing correct call site that this aligns with).

## 2. Call-site updates

- [ ] 2.1 In `autocoder/src/revisions.rs::process_revision_requests_at` (around line 359), compute `head_owner` before the `list_open_prs_for_head` call:
  ```rust
  let head_owner = github_cfg.fork_owner.as_deref().unwrap_or(&owner);
  let open_prs = github::list_open_prs_for_head(
      api_base, &token, &owner, &repo_name, head_owner, &repo.agent_branch,
  ).await.with_context(...)?;
  ```
- [ ] 2.2 In `autocoder/src/control_socket.rs::fetch_latest_pr` (around line 713), same fix. The function already receives `github_cfg: &GithubConfig`; compute `head_owner` from it AND pass to `latest_pr_for_head`.
- [ ] 2.3 `polling_loop.rs::open_pr_exists_for_agent_branch_at` already constructs the qualifier correctly via `list_open_prs` (line 4179-4180). NO change needed here.

## 3. Unit tests for the helpers

- [ ] 3.1 In `github.rs::tests`, add `list_open_prs_for_head_uses_head_owner_param_for_qualifier`:
  - Fixture: mockito stub at `/repos/upstream/myrepo/pulls?state=open&head=fork-owner:agent-q&per_page=100` returns `[]` (empty list — the assertion is about the QUERY SHAPE, not the result).
  - Call: `list_open_prs_for_head(&server.url, "tok", "upstream", "myrepo", "fork-owner", "agent-q").await`
  - Assert: the mockito mock matched (i.e., the request used `head=fork-owner:agent-q`, NOT `head=upstream:agent-q`). Pre-fix code would have produced the wrong head; this test would fail against it.
- [ ] 3.2 Same shape for `latest_pr_for_head`. The query also includes `state=all`, `sort=created`, `direction=desc`, `per_page=1` — match those too.
- [ ] 3.3 Verify the EXISTING tests at lines 1382, 1415, 1748, 1787 either (a) continue to pass under the migrated signature by passing the same value for `owner` AND `head_owner` (direct-push case), OR (b) get updated to exercise the fork-PR mode explicitly. Prefer (a) — minimal churn; the new tests in 3.1 / 3.2 cover the fork-mode case.

## 4. Integration tests at the call-site layer

- [ ] 4.1 In `revisions.rs::tests`, add `revise_dispatcher_finds_pr_in_fork_pr_mode`:
  - Fixture config with `github.fork_owner = "fork-owner"`, an agent branch, AND the upstream owner.
  - mockito intercepts the `head=fork-owner:agent-q` query AND returns one fake `PrSummary` with `number=123`.
  - Drive `process_revision_requests_at` against the mock.
  - Assert: the dispatcher proceeds past the empty-list early-return — measurable via either a subsequent comment-fetch GET (which the mock can also intercept) OR a tracing breadcrumb the test reads via a custom subscriber.
- [ ] 4.2 In `control_socket.rs::tests`, add `status_shows_latest_pr_in_fork_pr_mode`:
  - Fixture config with `github.fork_owner` set.
  - mockito returns a `PrListItem` only for `head=fork-owner:agent-q`.
  - Drive `fetch_latest_pr(&repo, &github_cfg)`.
  - Assert: returns `Some(PrSummary { ... })`, NOT `None`.
- [ ] 4.3 Both tests SHALL fail against pre-fix code. The pre-fix call site passes `&owner` (upstream) as the head qualifier, so the mockito mock keyed on `head=fork-owner:agent-q` never matches; the API call falls back to an unstubbed path which mockito treats as an error (OR returns the default 404). The test failures specifically name the head-qualifier mismatch in their assertions.

## 5. Spec deltas

- [ ] 5.1 `openspec/changes/a20a4-github-pulls-head-qualifier-respects-fork-owner/specs/orchestrator-cli/spec.md` ADDs: `GitHub pulls filter-by-head queries use fork_owner as the head qualifier owner in fork-PR mode`. Body covers the construction-site invariant AND the call-path coverage requirement (revise dispatcher + status reply named explicitly).

## 6. Verification

- [ ] 6.1 `cargo test --bin autocoder` passes — four new tests + existing tests after signature migration.
- [ ] 6.2 `openspec validate a20a4-github-pulls-head-qualifier-respects-fork-owner --strict` passes.
- [ ] 6.3 `cargo clippy --bin autocoder` produces no new warnings in `github.rs`, `revisions.rs`, OR `control_socket.rs` at lines I added/modified.
- [ ] 6.4 Manual verification (post-deploy, fork-PR-mode daemon, an open PR):
  - Run `@<bot> status <repo>` — assert: shows the PR number AND URL, NOT `(none)`.
  - Comment `@<bot> revise add a small clarifying sentence to the proposal` on an open PR.
  - Within one poll cycle, observe: revision attempt log entry, the `✅ Revision applied: ...` reply on the PR, AND a new force-push to the agent branch.
  - On a direct-push-mode daemon (a separate repo if available), verify the same flows continue to work — the head_owner correctly resolves to the upstream owner when `fork_owner` is unset.
