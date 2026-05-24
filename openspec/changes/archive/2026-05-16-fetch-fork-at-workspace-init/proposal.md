## Why

When a fork-PR-mode workspace is freshly cloned (because the directory was missing — typically after an operator nuked `/tmp` or a fresh deployment), `ensure_initialized` clones upstream and adds the `fork` remote, but does NOT fetch from `fork`. The local tracking ref `refs/remotes/fork/agent-q` therefore doesn't exist, while the remote fork has whatever commits prior autocoder runs pushed there.

On the first iteration's push attempt, `git push --force-with-lease fork agent-q` compares the (empty) local tracking ref against the remote's actual state and rejects with `! [rejected] agent-q -> agent-q (stale info)`. The push then fails forever — every subsequent iteration sees the same mismatch. Operators get a "branch push keeps failing" chatops alert and the bot is stuck.

Observed in production after `/tmp` directory cleanup. The fix is to fetch the fork remote at clone time so the local tracking ref reflects reality immediately, and `--force-with-lease`'s safety check operates on accurate data.

## What Changes

- **MODIFIED capability:** `workspace-manager`'s "Idempotent workspace initialization" requirement. After cloning AND registering the fork remote, the manager SHALL ALSO run `git fetch fork` so the local tracking ref aligns with the fork's actual state.
- **Code:** `workspace::ensure_initialized` tracks whether it performed a clone (`did_clone: bool`); after `ensure_remote(workspace, "fork", fork_url)`, if `did_clone && fork_url.is_some()`, run `git fetch fork`. A fetch failure is logged at WARN but not propagated — `--force-with-lease` will fail with a clearer error later if the divergence is real.
- **No new behavior on existing workspaces:** the re-initialize path (workspace already exists) is unchanged. The existing `git fetch origin` is sufficient there; fork tracking refs persist across iterations unless the workspace itself is deleted.

## Impact

- Affected specs: `workspace-manager` (one MODIFIED requirement).
- Affected code: `autocoder/src/workspace.rs::ensure_initialized` (one additional `git::fetch_remote(workspace, "fork")` call, gated on the clone-just-happened flag).
- Operator-visible behavior: after a fresh-clone iteration, the first push works correctly even when the fork has prior autocoder commits. No effect on existing workspaces (re-init path unchanged).
- Cost: one extra `git fetch fork` per workspace-initialization (only on fresh clones; subsequent iterations don't re-fetch fork).
- Breaking: no.
