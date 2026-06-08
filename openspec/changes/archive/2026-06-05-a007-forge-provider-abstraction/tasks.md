# Implementation tasks

## 1. Define the `Forge` trait

- [x] 1.1 Create a forge module with a `Forge` trait covering every forge API operation: `parse_repo(url) -> (host, project)`; PR/MR lifecycle (`open_pr`, `list_open_prs`, `find_pr_by_head`, `set_pr_draft`); comments (`list_comments_since`, `post_comment`); reviews (`post_review`); fork (`create_fork`); `authorize(commenter) -> AuthLevel`; `branch_url(...)`.
- [x] 1.2 Keep the trait surface to what is GitHub-coupled today — do NOT pull git operations (clone/fetch/branch/commit/push) into it; those stay in `git.rs`, host-neutral.

## 2. Implement `GithubForge` (behavior-identical)

- [x] 2.1 Move the GitHub REST functions currently in `github.rs` behind `GithubForge`: `parse_repo_url` → `GithubForge::parse_repo` (still GitHub-shaped), `create_fork`, PR open/list/find-by-head, `set_pr_draft` (with the existing draft handling), comment list-since/post, review post. Reproduce the exact REST shapes and results.
- [x] 2.2 Move the `author_association` authorization gate (currently in `revisions.rs`) into `GithubForge::authorize`, preserving the `allowed_associations` / `allowed_users` logic exactly.
- [x] 2.3 Implement `GithubForge::branch_url` to produce today's push-only branch hint.

## 3. Provider selection by URL host

- [x] 3.1 Resolve the forge provider from the repository URL host: a GitHub host → `GithubForge`. Existing GitHub configs and token routing are unchanged.
- [x] 3.2 A host with no registered provider returns a clear error naming the host; no forge operation proceeds (this preserves today's non-GitHub-URL rejection until Phase 2 registers `GitlabForge`).

## 4. Route all call sites through the trait

- [x] 4.1 Replace direct `github::` API calls with `Forge` trait calls at every site: `revisions.rs` (comment fetches, authorization, reply posting), `config.rs` (repo-owner resolution via `parse_repo`), the polling loop (open-PR checks, PR creation), the chatops branch-pushed hint, and the reviewer's comment/review posting.
- [x] 4.2 After the sweep, no GitHub REST API call exists outside the forge module (single source of truth).

## 5. Leave the git half untouched

- [x] 5.1 Confirm `git.rs` (clone/fetch/branch/commit/push) is unchanged and continues to use the raw URL and `origin`.

## 6. Tests

- [x] 6.1 The existing GitHub PR/comment/authorization tests pass unchanged, now exercising `GithubForge` through the trait.
- [x] 6.2 A single-source-of-truth check asserts no GitHub REST API call exists outside the forge module.
- [x] 6.3 A GitHub-host URL resolves to `GithubForge`; an unsupported host returns a clear error naming the host.
- [x] 6.4 Authorization through `GithubForge::authorize` yields the same allow/deny decisions as the pre-extraction gate for the same `author_association` / `allowed_users` inputs.
- [x] 6.5 Git clone/fetch/push behavior is unaffected (no routing through the trait).

## 7. Acceptance gate

- [x] 7.1 `cargo test` passes for the autocoder crate.
- [x] 7.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [x] 7.3 `openspec validate a007-forge-provider-abstraction --strict` passes.
