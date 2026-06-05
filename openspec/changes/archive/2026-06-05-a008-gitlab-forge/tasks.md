# Implementation tasks

## 1. `GitlabForge` provider (forge module)

- [x] 1.1 `parse_repo`: extract the GitLab host AND the `namespace/project` path; URL-encode the path for the GitLab `:id` form (`namespace%2Fproject`), supporting nested groups.
- [x] 1.2 MR lifecycle against the GitLab API: `open_pr` → `POST /projects/:id/merge_requests`; `list_open_prs` → `GET /projects/:id/merge_requests?state=opened`; `find_pr_by_head` → match by `source_branch`; `set_pr_draft` → toggle the `Draft:` title prefix via `PUT /projects/:id/merge_requests/:iid`.
- [x] 1.3 Comments: `list_comments_since` AND `post_comment` via `GET`/`POST /projects/:id/merge_requests/:iid/notes`.
- [x] 1.4 Reviews: `post_review` → approve maps to `POST /projects/:id/merge_requests/:iid/approve`; request-changes AND comment map to an MR note (GitLab has no request-changes state).
- [x] 1.5 Authorization: `authorize` reads the commenter's project access level (e.g. `GET /projects/:id/members/all/:user_id`) AND authorizes Developer (30) and above; Reporter (20) AND Guest (10) are denied.
- [x] 1.6 `branch_url`: produce the GitLab MR-create hint (`glab mr create` / MR web URL) for the push-only path.

## 2. Forge config block + selection (`config.rs`)

- [x] 2.1 Parse an optional per-repo `forge:` block: `kind` (`github` | `gitlab`), `host`, optional `api_base`, token route.
- [x] 2.2 Implement the selection precedence: explicit `forge:` block authoritative → else `github.com` host → `GithubForge` → else the no-provider error. GitLab is reachable ONLY via an explicit `forge: { kind: gitlab }` (no host-sniffing).
- [x] 2.3 Support GitHub Enterprise: `kind: github` with a self-hosted `host`/`api_base` drives `GithubForge` against that endpoint.
- [x] 2.4 Source the provider token from the forge block's token route through the existing token-routing mechanism.

## 3. Push-only hint

- [x] 3.1 Make the push-only branch hint forge-specific via `Forge::branch_url` (GitHub `gh pr create` vs GitLab `glab mr create` / MR web URL).

## 4. Tests

- [x] 4.1 `forge: { kind: gitlab }` selects `GitlabForge`; `github.com` with no block selects `GithubForge`; `kind: github` + `api_base` selects `GithubForge` against the GHE endpoint; a GitLab-host URL with no block returns the no-provider error.
- [x] 4.2 MR create/list/find-by-source-branch round-trips against recorded GitLab API fixtures.
- [x] 4.3 `set_pr_draft` toggles the `Draft:` title prefix on/off.
- [x] 4.4 `post_review` approve → approval call; request-changes/comment → an MR note.
- [x] 4.5 `authorize` authorizes Developer/Maintainer/Owner AND denies Reporter/Guest.
- [x] 4.6 `parse_repo` handles a nested-group GitLab URL and URL-encodes the project path.

## 5. Acceptance gate

- [x] 5.1 `cargo test` passes for the autocoder crate.
- [x] 5.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [x] 5.3 `openspec validate a008-gitlab-forge --strict` passes.
