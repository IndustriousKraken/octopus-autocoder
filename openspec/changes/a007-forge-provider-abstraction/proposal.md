## Why

The core loop already works against non-GitHub hosts *by accident*: the git half — clone, fetch, branch, commit, push — uses the raw URL and the `origin` remote, so it is host-neutral. What does NOT work is everything routed through `github.rs`: `parse_repo_url` rejects non-GitHub URLs, and the REST calls are GitHub-shaped (`/repos/{owner}/{repo}/pulls`). A user on a private GitLab instance therefore gets autonomous implementation and a pushed agent branch, but opens the merge request by hand. Self-hosted GitLab is concentrated exactly where this tool matters (security/pentest shops, air-gapped and compliance-controlled infra, operators on their own servers), so first-class GitLab — and, for free, GitHub Enterprise — is worth the abstraction.

This change is **Phase 1**: the load-bearing, behavior-preserving extraction. Every GitHub API call moves behind a `Forge` trait with a single `GithubForge` provider that reproduces today's behavior exactly. Nothing user-visible changes; the existing GitHub tests are the proof. It is the same trait-with-providers shape already used for `CliStrategy`, and it sets up `GitlabForge` (Phase 2) and fork/issues parity (Phase 3) as pure additions. Phase 2's open decisions (GitLab access-level mapping, draft-MR semantics, config shape) do not gate this step.

## What Changes

**A `Forge` trait owns every forge API operation.** The trait surface is everything GitHub-coupled today: repository-URL parsing (`parse_repo`, replacing the GitHub-only `parse_repo_url`); PR/MR lifecycle (`open_pr`, `list_open_prs`, `find_pr_by_head`, `set_pr_draft`); comments (`list_comments_since`, `post_comment`); reviews (`post_review`); fork (`create_fork`); commenter authorization (`authorize`); and the push-only branch hint (`branch_url`). The concrete provider is selected **per repository by URL host**.

**`GithubForge` is the only provider and is behavior-identical.** Today's `github.rs` REST shapes, the `author_association`-based authorization gate (currently in `revisions.rs`), and the draft-PR handling all move behind `GithubForge` unchanged. No second provider, no GitLab, no new operator config, no behavior change.

**The git operations stay outside the trait.** Clone/fetch/branch/commit/push (`git.rs`) already use the raw URL and `origin`; they are host-neutral and are not part of the forge surface.

**Single source of truth.** After the change, no direct GitHub REST call exists outside the forge module; every forge call site goes through the trait. A repository whose host has no registered forge provider returns a clear error (preserving today's rejection of non-GitHub URLs until Phase 2 adds `GitlabForge`).

## Impact

- **Affected specs:** `git-workflow-manager` — ADD `Forge provider abstraction`.
- **Affected code:** a new forge module (the `Forge` trait + `GithubForge`); `github.rs` API functions (`parse_repo_url`, `create_fork`, PR open/list/find, set-draft, comment list/post, review post) move behind `GithubForge`; the `author_association` gate in `revisions.rs` becomes `GithubForge::authorize`; call sites in `revisions.rs`, `config.rs`, the polling loop, the chatops branch-hint, and the reviewer's comment posting route through the trait. `git.rs` is untouched.
- **Operator-visible behavior:** none — GitHub users see identical behavior, verified by the existing GitHub tests. The GitHub-specific requirements in `orchestrator-cli` (token routing, fork mode, the authorization gate, comment-since) remain accurate and unmodified; Phase 2 generalizes their wording to forge-neutral when `GitlabForge` lands.
- **Dependencies:** independent. Foundation for Phase 2 (`GitlabForge` — the daily MR/comment/review/authorize loop) and Phase 3 (fork mode + the issues-lane forge side). Complements the archived `a000` authorization gate, which becomes `GithubForge::authorize`.
- **Acceptance:** `cargo test` passes; `cargo clippy --all-targets -- -D warnings` is clean; `openspec validate a007-forge-provider-abstraction --strict` passes. Tests: the existing GitHub PR/comment/authorization tests pass unchanged through the trait; a single-source-of-truth scan finds no GitHub REST call outside the forge module; a `github.com` URL resolves to `GithubForge`; an unsupported host returns a clear error; git clone/fetch/push are unaffected.
