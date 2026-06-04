# Implementation tasks

## 1. `cache.workspaces_max_gb` config (orchestrator-cli)

- [ ] 1.1 `config.rs` — add an optional top-level `cache` block with `workspaces_max_gb: Option<u64>` (None = unbounded, the default). Validate `> 0` when set. Include it in the hot-reloadable subset so a reload applies the new cap at the next iteration.
- [ ] 1.2 At startup, when `cache.workspaces_max_gb` is unset, log a ONE-TIME notice that the workspace cache is unbounded AND names the field to bound it.

## 2. Eviction helper (paths / maintenance)

- [ ] 2.1 Measure total `<cache>/workspaces/` size AND per-workspace size (directory walk).
- [ ] 2.2 Maintain a per-workspace last-used timestamp (touch a marker — or record in state — at each iteration that uses the workspace). Read it to order eviction candidates oldest-first.
- [ ] 2.3 `evict_workspace(key)` = `remove_dir_all(<cache>/workspaces/<key>)`, logging the key, reclaimed bytes, and the new total. Symlink-safe (do not follow symlinks out of the cache root).

## 3. Enforce the cap at iteration start (polling_loop)

- [ ] 3.1 Before a repo does work, if `workspaces_max_gb` is set AND the total exceeds the cap, evict least-recently-used IDLE workspaces until under cap OR only non-evictable workspaces remain.
- [ ] 3.2 NEVER evict: the repo currently iterating, OR any workspace holding a per-repo busy marker / active lock. Eviction must respect the existing per-repo busy-marker mechanism so a concurrently-iterating repo is never removed out from under itself.
- [ ] 3.3 If the non-evictable set alone exceeds the cap, log a WARN (cannot reclaim to target) AND proceed — eviction NEVER blocks or fails an iteration.
- [ ] 3.4 An evicted repo re-clones via the existing workspace-init path on its next iteration; confirm per-PR revision state AND audit state (state dir, not workspace) survive eviction.

## 4. Tests

- [ ] 4.1 Unset cap → no eviction occurs AND the one-time unbounded-cache startup notice is emitted.
- [ ] 4.2 With a cap set and the cache over budget, eviction removes oldest-idle workspaces until the total is under the cap.
- [ ] 4.3 A workspace holding a busy marker AND the current repo's workspace are never evicted.
- [ ] 4.4 When only non-evictable workspaces remain and they exceed the cap, a WARN is logged and the iteration proceeds.
- [ ] 4.5 After eviction, the evicted repo's next iteration re-clones cleanly AND its per-PR state file (state dir) is intact.

## 5. Acceptance gate

- [ ] 5.1 `cargo test` passes for the autocoder crate.
- [ ] 5.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [ ] 5.3 `openspec validate a65-workspace-cache-eviction --strict` passes.
