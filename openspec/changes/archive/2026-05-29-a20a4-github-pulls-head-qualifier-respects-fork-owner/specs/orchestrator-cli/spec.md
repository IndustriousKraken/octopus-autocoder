## ADDED Requirements

### Requirement: GitHub `pulls` filter-by-head queries use `fork_owner` as the head qualifier owner in fork-PR mode
Any GitHub REST API request to `GET /repos/{owner}/{repo}/pulls` that filters by `head` SHALL construct the head qualifier as `<head_owner>:<head_branch>` where:

- `head_owner = github.fork_owner` when `github.fork_owner` is configured (fork-PR mode).
- `head_owner = <upstream_owner>` when `github.fork_owner` is absent (direct-push mode).

The `head_owner` SHALL be an explicit named variable in the call site, computed from `github.fork_owner.as_deref().unwrap_or(&upstream_owner)` (OR equivalent). Helper functions in the GitHub-API module SHALL accept the head qualifier owner as an explicit parameter; they SHALL NOT silently reuse the upstream-owner argument (used to construct the URL path) for the head qualifier. The construction-site discipline is what prevents the bug class — every caller is forced to think about which owner belongs in the head filter.

This requirement applies to every code path that issues a `pulls?head=...` query, including:

- The polling iteration's open-PR existence check (`open_pr_exists_for_agent_branch_at`).
- The PR-comment revision dispatcher's PR-list query (`process_revision_requests_at`).
- The operator-status reply's latest-PR query (`fetch_latest_pr`).
- Any future code that filters PRs by head.

The rationale: GitHub's `head` filter is an exact-string match on `<owner>:<branch>`. In fork-PR mode the PR's head is on the operator's fork, so `<fork-owner>:<branch>` matches AND `<upstream-owner>:<branch>` does not. Pre-spec code in two of the three head-filter queries used `<upstream-owner>:<branch>` (because the helper functions reused the URL-path owner parameter for the head qualifier construction), which never matched any PR in fork-PR mode. Operators in fork-PR mode lost `@<bot> revise` on PRs AND status's `latest PR` field with no log line — the helpers returned empty lists which the callers correctly treated as "no PR" without a way to distinguish that signal from a real "no PR exists" state.

The invariant is enforceable by code review: at every `head=...` query construction site, the `head_owner` variable's source MUST be visible AND MUST explicitly consult `github.fork_owner`.

#### Scenario: Fork-PR-mode revise dispatcher finds the PR
- **WHEN** `github.fork_owner` is configured AND an open PR exists with head `<fork_owner>:<agent_branch>` on the upstream repo
- **AND** the polling iteration's revise-dispatcher step runs
- **THEN** the GitHub `pulls?head=...` query is constructed with `head=<fork_owner>:<agent_branch>`
- **AND** the API returns the PR
- **AND** the dispatcher fetches the PR's comments AND proceeds with the revision flow per the canonical revise mechanism

#### Scenario: Fork-PR-mode status reply finds the PR
- **WHEN** `github.fork_owner` is configured AND an open PR exists with head `<fork_owner>:<agent_branch>`
- **AND** an operator runs `@<bot> status <repo>`
- **THEN** the status path's `fetch_latest_pr` call constructs `head=<fork_owner>:<agent_branch>`
- **AND** the reply's `latest PR` line names the PR number AND URL, NOT `(none)`

#### Scenario: Direct-push-mode behaviour unchanged
- **WHEN** `github.fork_owner` is absent (direct-push mode)
- **AND** any of the three head-filter code paths runs
- **THEN** the `head_owner` resolves to the upstream owner (via `unwrap_or(&upstream_owner)`)
- **AND** the constructed query exactly matches the pre-spec behaviour
- **AND** existing direct-push-mode operators see no behavioural change

#### Scenario: Helper functions require explicit head_owner parameter
- **WHEN** a maintainer inspects the GitHub-API helper signatures
- **THEN** `list_open_prs_for_head` AND `latest_pr_for_head` (AND any future helper that issues a head-filtered pulls query) take a separate `head_owner: &str` parameter alongside the URL-path `owner` parameter
- **AND** the helpers' internal `format!("{head_owner}:{head_branch}")` construction does NOT reuse the `owner` parameter
- **AND** every caller passes the explicitly-computed `head_owner`

#### Scenario: Regression test guards the construction
- **WHEN** the test suite runs
- **THEN** at least one unit test exercises `list_open_prs_for_head` with `owner != head_owner` AND asserts the query string contains `head=<head_owner>:<head_branch>` exactly
- **AND** at least one unit test exercises `latest_pr_for_head` with the same shape
- **AND** at least one integration test for the revise dispatcher exercises fork-PR mode end-to-end AND asserts the dispatcher proceeds past the open-PR-list step (i.e., the mock matched the fork-owner-qualified query)
- **AND** at least one integration test for the status reply exercises fork-PR mode end-to-end AND asserts `latest PR` is populated rather than `(none)`
- **AND** all four tests fail against any implementation that uses the upstream owner as the head qualifier
