## Why

`fetch-fork-at-workspace-init` (the companion change) fixes the `--force-with-lease` "stale info" rejection by syncing local tracking with the fork's actual state. That's the right default for operators with open PRs against the fork. But for operators recovering from a deeper snafu — accidentally pushed wrong-shape work, fork has stale branches nobody cares about, the fork's state is genuinely worthless — fetching it is just preserving cruft. They'd prefer a clean slate.

This change adds an opt-in per-repo flag: when the workspace is being freshly cloned AND `github.recreate_fork_on_reinit: true` AND fork-PR mode is active, autocoder deletes the existing fork on GitHub via the API and re-creates it. The result: fork is a pristine mirror of upstream, local tracking refs are empty (no stale `agent-q` anywhere), the next push starts a fresh branch.

Off by default because deleting the fork closes every open PR autocoder ever opened from that fork (PR head ref disappears → GitHub auto-closes the PR). For operators with a long-lived stack of PRs in flight, that's catastrophic. For operators in the "snafu, starting over" mode, it's exactly what they want.

## What Changes

- **ADDED capability:** `workspace-manager` gains a "Optional fork recreation on workspace reinitialization" requirement.
- **MODIFIED capability:** `orchestrator-cli` gains a new config field `github.recreate_fork_on_reinit: bool` (default `false`).
- **Code:**
  - `GithubConfig` gets a new `recreate_fork_on_reinit: Option<bool>` field (defaults to `false` when unset). It is a global flag on the `github:` block, NOT per-repo, because all configured repos in a single autocoder process share the same fork owner.
  - `workspace::ensure_initialized` takes a new `recreate_fork_on_reinit: bool` parameter; when `did_clone && fork_url.is_some() && recreate_fork_on_reinit`, the manager:
    1. Resolves the upstream owner + repo name + the operator's GitHub token.
    2. Calls `github::delete_repo(fork_owner, repo_name, token)` (best-effort: a 404 means "fork was already gone" and is silently accepted).
    3. Waits up to 5 seconds for the deletion to propagate.
    4. Calls `github::create_fork(upstream_owner, upstream_repo, token)`.
    5. Waits up to the existing 30-second fork-availability check (already implemented in startup-fork-verification).
    6. Then proceeds with `ensure_remote` + `fetch fork` (the conservative default). The fetch returns an empty tracking ref because the fork is now empty.
  - New GitHub helper: `github::delete_repo(owner, repo, token) -> Result<()>` — `DELETE /repos/{owner}/{repo}`. 204 success; 404 silent-success; other non-2xx error.
- **Operator-visible chatops:** when re-fork triggers, post a one-line ChatOps notification `:warning: <repo>: re-forked at workspace reinitialization (previous fork deleted; any open PRs from this fork are now closed)`. This is a destructive operation; the operator should know about it loudly.

## Impact

- Affected specs: `workspace-manager` (one ADDED requirement), `orchestrator-cli` (one ADDED scenario under existing config validation requirements).
- Affected code: `autocoder/src/github.rs` (new `delete_repo` helper), `autocoder/src/config.rs` (new field), `autocoder/src/workspace.rs::ensure_initialized` (new parameter, conditional re-fork path), `autocoder/src/cli/run.rs` (plumb the flag through), `autocoder/src/polling_loop.rs` (chatops notification when re-fork triggers).
- New GitHub API permission required: the operator's PAT must include `delete_repo` scope when this flag is enabled. Without it, the delete call returns 403 and the re-fork operation fails; autocoder logs ERROR, falls back to the conservative fetch-fork-at-init behavior, and posts a chatops alert naming the missing scope.
- Operator-visible behavior: with default config (`recreate_fork_on_reinit: false` or unset), no behavior change vs. the conservative spec. With it enabled, fresh-clone iterations destroy and recreate the fork.
- Breaking: no. Default off; opt-in by setting the field.
- Foundation dependency: requires `fetch-fork-at-workspace-init` to be applied first (this change layers on top of the conservative default, providing an alternative path within the same `ensure_initialized` flow).
