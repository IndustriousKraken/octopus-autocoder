## Why

`daemon-control-socket-and-easy-reload` hot-applies tokens, reviewer setup, and chatops config but parks `repositories` changes as restart-required. Adding or removing a repo, or changing `base_branch` / `agent_branch` / `poll_interval_sec` / `chatops_channel_id` on an existing one, still forces a `systemctl restart`. This change closes that gap by extending the reload handler to spawn new polling tasks for added repos, cancel tasks for removed ones, and hot-swap per-task config holders for changed ones.

## What Changes

- **MODIFIED capability:** `orchestrator-cli`'s reload handler now hot-applies the `repositories` section in addition to the existing hot-applicable sections.
- **Per-task config holder:** each polling task's `RepositoryConfig` is held behind an `Arc<ArcSwap<RepositoryConfig>>` so the reload handler can replace it. The task reads from the holder at the top of each iteration.
- **Per-task cancellation token:** the daemon maintains a per-repo `CancellationToken` (keyed by repo URL) in addition to the global shutdown token. Removing a repo from config triggers `cancel()` on that repo's token; the task exits at its next iteration boundary. The global shutdown token still cancels all tasks together.
- **Task spawn for added repos:** the reload handler launches a new polling task with the same parameters as startup (workspace resolution, dirty-check, busy-marker setup) for any repo present in the new config but not in the current task set.
- **Identity key for repos:** two repos are "the same repo" iff their `url` field matches. Operators who want to switch a repo to a different upstream URL are deleting the old + adding the new from the daemon's perspective.
- **Diff semantics for "changed in place":**
  - `base_branch`, `agent_branch`, `chatops_channel_id`, `poll_interval_sec`, `local_path`: hot-swap via `ArcSwap`. Takes effect on the next iteration.
  - `url`: treated as a different repo. The old task is cancelled, a new one is spawned with the new URL (and new derived workspace path).
- **In-flight iteration safety:** a repo being cancelled mid-iteration finishes its current iteration normally (including push + PR if commits were produced). The cancellation check is in the inter-poll sleep, so the next poll never starts after the cancel.
- **Code:**
  - `polling_loop::run` signature changes to accept `Arc<ArcSwap<RepositoryConfig>>` instead of owned `RepositoryConfig`.
  - The polling task reads from the swap holder at the top of each iteration. Reading mid-iteration is forbidden — a single iteration uses a consistent snapshot.
  - The daemon owns a `HashMap<String, RepoTaskHandle>` keyed by URL; `RepoTaskHandle` bundles the per-repo cancellation token + the `Arc<ArcSwap<RepositoryConfig>>` + the `JoinHandle`.
  - The reload handler diffs `current_repos` vs `new_repos` by URL, then performs spawn / cancel / hot-swap as appropriate.
- **Response shape extension:** the reload response's `applied` array now includes `"repositories"` when any repository delta was applied. The response gains a `repositories_delta` field naming added / removed / changed URLs, so operators see exactly what happened.

## Impact

- Affected specs: `orchestrator-cli` (one MODIFIED requirement extending the reload handler, one MODIFIED requirement for the per-repo polling task to read from a swap holder).
- Affected code: `autocoder/src/polling_loop.rs` (per-task swap reads), `autocoder/src/cli/run.rs` (task map, per-repo cancel tokens), `autocoder/src/control_socket.rs` (reload handler extension).
- Behavior change visible to operators: `autocoder reload` after editing the `repositories` list now applies the change immediately. New repos start polling; removed repos exit at their next iteration boundary.
- Backward compatibility: a deployment still on the `daemon-control-socket-and-easy-reload` version will report `repositories` as `requires_restart` on reload — operators with the older daemon revert to `systemctl restart` for repo changes. No file-format break.
- Risk: task-lifecycle bugs are harder to test than pure-data hot-swap. The tests must cover all three deltas (add, remove, modify) plus the edge cases (cancel-mid-iteration, spawn-during-shutdown).
