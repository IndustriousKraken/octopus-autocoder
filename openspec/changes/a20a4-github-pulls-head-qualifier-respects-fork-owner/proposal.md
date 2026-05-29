## Why

Production-observed bug: `@<bot> revise <text>` PR comments are never processed on fork-PR-mode deployments. The operator posts the comment, the daemon's iteration runs, and nothing happens — no log entry, no revision attempt, no chatops acknowledgement. Same root cause: the daemon's status reply shows `latest PR: (none)` for every fork-PR-mode repo even when an open PR exists.

**Trace:**

The bug is in three GitHub API helpers in `autocoder/src/github.rs`. Two of them — `list_open_prs_for_head` (line 819) AND `latest_pr_for_head` (line 135) — hardcode the head qualifier as `<owner>:<branch>` using the upstream owner that the caller passed in:

```rust
let head_qualified = format!("{owner}:{head_branch}");
```

In direct-push mode (no `github.fork_owner` configured) this is correct — the PR's head is on the upstream repo, so `<upstream-owner>:<branch>` matches. In fork-PR mode (`github.fork_owner` set), the PR's head is on the FORK, so the correct qualifier is `<fork-owner>:<branch>`. GitHub's `head` filter is exact-match; `<upstream-owner>:<branch>` never matches a fork-headed PR, AND the API returns an empty list.

**Operator-visible consequences in fork-PR mode:**

1. **`@<bot> revise <text>`-on-PR is non-functional.** The revise dispatcher at `revisions.rs:359-367` calls `list_open_prs_for_head(api_base, &token, &owner /* upstream */, &repo_name, &repo.agent_branch)`. The internal query asks GitHub `head=<upstream-owner>:<branch>`. Returns empty. The dispatcher iterates over zero PRs. Operator's revise comment is never fetched. No log entry, no acknowledgement, no work. **Has been broken since fork-PR mode shipped.**

2. **`@<bot> status <repo>` shows `latest PR: (none)`.** The status path's `latest_pr_for_head` has the identical defect. Operators see `(none)` despite having an open PR.

The third helper, `list_open_prs` (line 39), is correct — it takes a pre-qualified `head` string from the caller, AND the caller (`polling_loop.rs:4179-4180`) builds it correctly with `fork_owner` first:

```rust
let head_owner = github_cfg.fork_owner.as_deref().unwrap_or(&upstream_owner);
let head = format!("{}:{}", head_owner, repo.agent_branch);
```

This is why the polling iteration's "skip when an open PR exists" check works correctly in fork-PR mode but the revise dispatcher AND status reply do not — same data, two helpers, one of them got the construction right and two got it wrong.

The bug class is the same shape as `a20a3`'s: the wrong identifier is used for a routing/filtering decision. In `a20a3` it was iteration termination gated on a stale signal; here it is GitHub API filtering gated on the upstream owner where fork-PR mode requires the fork owner. Both bugs share the structural defect of using an incorrect identifier for an external-facing query AND silently returning empty/falling-through.

## What Changes

**Refactor `list_open_prs_for_head` AND `latest_pr_for_head` to require an explicit `head_owner` parameter.** The helpers SHALL NOT silently reuse the `owner` parameter (which always names the UPSTREAM repo for path-routing) as the head qualifier owner. The new signatures:

```rust
pub async fn list_open_prs_for_head(
    api_base: &str,
    token: &str,
    owner: &str,           // upstream — used in /repos/{owner}/{repo}/pulls path
    repo: &str,
    head_owner: &str,      // NEW: fork owner in fork-PR mode, upstream in direct mode
    head_branch: &str,
) -> Result<Vec<PrSummary>>;

pub async fn latest_pr_for_head(
    api_base: &str,
    token: &str,
    owner: &str,
    repo: &str,
    head_owner: &str,      // NEW
    head_branch: &str,
) -> Result<Option<PrSummary>>;
```

Internal construction becomes `format!("{head_owner}:{head_branch}")`. Callers SHALL compute `head_owner = github_cfg.fork_owner.as_deref().unwrap_or(&upstream_owner)` AND pass it explicitly — matching the established pattern from `polling_loop.rs::open_pr_exists_for_agent_branch_at`.

**Update both call sites:**

1. `autocoder/src/revisions.rs::process_revision_requests_at` — currently passes `&owner` (upstream) as the head qualifier owner. Update to compute `head_owner` from `github_cfg.fork_owner` first AND pass it explicitly.
2. `autocoder/src/control_socket.rs::fetch_latest_pr` — same fix.

Both call sites get the `github_cfg` reference they need (the revise dispatcher already takes it; the status path takes it via `fetch_latest_pr(repo, github_cfg)`).

**Leave `list_open_prs` as-is.** It already takes a pre-qualified `head` string AND the caller at `polling_loop.rs:4179-4180` builds it correctly. Renaming OR adding a `head_owner` parameter to that one too would be churn without benefit.

**Canonical invariant: GitHub `pulls` filter-by-head construction MUST consult `fork_owner`.** A new requirement in `orchestrator-cli` codifies: any code path that constructs a GitHub `head` query parameter (the `<owner>:<branch>` exact-match used by `GET /repos/{owner}/{repo}/pulls?head=...`) SHALL use `<fork_owner>:<branch>` when `github.fork_owner` is configured AND `<upstream_owner>:<branch>` otherwise. The invariant SHALL be enforceable by reading the construction site: the `head_owner` SHALL come from a named variable that explicitly consults `github.fork_owner.as_deref().unwrap_or(&upstream_owner)`, NOT from an implicit reuse of a same-named parameter.

**Regression-prevention tests.** Two unit tests in `github.rs`:

1. `list_open_prs_for_head_uses_head_owner_param_for_qualifier` — fixture: mockito intercepts the GET, expects `head=fork-owner-name:agent-q`. Caller passes `head_owner: "fork-owner-name"`, `owner: "upstream-owner-name"`. Assert the query string. Pre-fix construction would put `upstream-owner-name` in the qualifier; the test fails against pre-fix code.
2. `latest_pr_for_head_uses_head_owner_param_for_qualifier` — identical shape for the status helper.

Two integration tests at the call-site layer:

3. `revise_dispatcher_finds_pr_in_fork_pr_mode` — fixture config with `github.fork_owner` set + mockito GitHub stub responding with a PR for `head=<fork-owner>:<branch>` only. Drive `process_revision_requests_at`. Assert: the dispatcher fetched the PR's comments (i.e., proceeded past the empty-list early-return).
4. `status_shows_latest_pr_in_fork_pr_mode` — similar shape for `fetch_latest_pr`. Assert: the returned `Option<PrSummary>` is `Some`, NOT `None`.

## Impact

- **Affected specs:**
  - `orchestrator-cli` — ADDED requirement: `GitHub pulls filter-by-head queries use fork_owner as the head qualifier owner in fork-PR mode`. Covers the construction-site invariant AND the two known-affected call paths (revise dispatcher, status reply).
- **Affected code:**
  - `autocoder/src/github.rs` — extend `list_open_prs_for_head` AND `latest_pr_for_head` signatures with `head_owner: &str`. Update internal `head_qualified` construction. Update the existing tests that call these helpers to pass the new parameter (any existing test that uses the same string for `owner` AND `head_owner` continues to pass after the migration; tests that exercise fork-mode get new assertions).
  - `autocoder/src/revisions.rs` — `process_revision_requests_at` computes `head_owner` from `github_cfg.fork_owner` AND passes it to `list_open_prs_for_head`.
  - `autocoder/src/control_socket.rs` — `fetch_latest_pr` accepts the (already-available) `github_cfg` argument, computes `head_owner`, AND passes it to `latest_pr_for_head`.
  - New tests in `github.rs::tests`, `revisions.rs::tests`, AND `control_socket.rs::tests` per the four tests listed above.
- **Operator-visible behavior:**
  - `@<bot> revise <text>` on a PR comment in fork-PR mode begins to work for the first time. Operators see the revision attempt log AND the `✅ Revision applied: <description>` reply per the canonical revise mechanism.
  - `@<bot> status <repo>` correctly reports the latest PR in fork-PR mode. `(none)` only appears when there is genuinely no open PR.
  - No new config knobs. No behavior change in direct-push mode (the upstream owner IS the head owner; the existing query string matches what the new construction produces).
- **Breaking:** no for operators. Internal API change — two function signatures gain a parameter — but autocoder is not consumed as a library.
- **Acceptance:** `cargo test` passes (new tests + existing tests pass after signature migration); `openspec validate a20a4-github-pulls-head-qualifier-respects-fork-owner --strict` passes; `cargo clippy --bin autocoder` produces no new warnings in touched files. Manual verification: on a fork-PR-mode daemon AND an open PR, `@<bot> status <repo>` reports the PR; `@<bot> revise add a missing test for <X>` on the PR triggers a revision iteration within one poll cycle.
