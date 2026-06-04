## Why

Per-repo workspaces live under `<cache>/workspaces/` and are described in the code as "re-creatable but kept" — but nothing ever reclaims them. Each active repo accumulates a build-artifact tree (for a Rust repo, a multi-gigabyte `target/debug`), and the daemon has no size cap, LRU eviction, or pruning anywhere. Two active repos wedged a host at ~34 GB of workspaces on ~39 GB free. The only existing removal paths are the manual `wipe_workspace` verb and unrelated non-spec-path cleanup; neither bounds total cache size. This is an unbounded-growth defect: any daemon running several repos long enough will fill its disk.

This change adds a configurable size cap on the workspaces cache with least-recently-used eviction of whole workspaces. Whole-workspace eviction is deliberately language-agnostic — it makes no assumption about which subdirectories are build artifacts (`target/`, `node_modules/`, …); it simply removes the least-recently-used clones when the cache is over budget. An evicted repo re-clones on its next iteration via the existing workspace-init path, and loses nothing: per-PR revision state and audit state live in the state directory, not the workspace. Only cold repos pay a re-clone + rebuild; active repos keep their incremental builds.

## What Changes

**`cache.workspaces_max_gb` size cap (orchestrator-cli).** A new optional config field caps the total size of `<cache>/workspaces/`. When unset (the default), behavior is unchanged — the cache is unbounded — BUT the daemon logs a one-time startup notice that the workspace cache is unbounded and names the field to bound it (so the failure mode is discoverable before it wedges a disk).

**LRU eviction (orchestrator-cli).** When the cap is set, at each repo's iteration start the daemon measures the total workspaces-dir size; if it exceeds the cap, it evicts least-recently-used IDLE workspaces (`remove_dir_all` of the whole `<cache>/workspaces/<key>`) until the total is under the cap OR only non-evictable workspaces remain. The repo currently iterating AND any workspace holding a per-repo busy marker are NEVER evicted. If the non-evictable set alone exceeds the cap, the daemon logs a WARN that it cannot reclaim to target and proceeds — eviction never blocks work. Each eviction logs the workspace key, its reclaimed size, and the new total. "Least-recently-used" is by the last iteration that used the workspace (a daemon-maintained per-workspace last-used timestamp).

## Impact

- **Affected specs:**
  - `orchestrator-cli` — ADDED `Workspace cache LRU eviction under a size cap`.
- **Affected code:**
  - `autocoder/src/config.rs` — a `cache` block with `workspaces_max_gb: Option<u64>` (None = unbounded). Eligible for the hot-reload subset.
  - `autocoder/src/paths.rs` / a new maintenance helper — measure per-workspace size, record/read a per-workspace last-used timestamp, evict a workspace (`remove_dir_all`).
  - `autocoder/src/polling_loop.rs` — at iteration start, run the cap check + LRU eviction (respecting busy markers and the current repo) before doing work; emit the one-time unbounded-cache startup notice.
- **Operator-visible behavior:** none by default (unbounded, as today) except a one-time startup log nudging operators to set the cap. With the cap set, cold workspaces are evicted (and transparently re-cloned later) to keep the cache under budget.
- **Acceptance:** `cargo test` passes; `openspec validate a65-workspace-cache-eviction --strict` passes. Tests: unset cap → no eviction + the startup notice; an over-cap cache evicts oldest idle workspaces until under cap; a busy / current-repo workspace is never evicted; eviction stops (with a WARN) when only non-evictable workspaces remain; an evicted repo re-clones cleanly on next iteration with its per-PR state intact.
- **Dependencies:** none — independent of the fleet stream (a55–a64). Builds on the a25 data-paths split (per-PR/audit state already lives in the state dir, so evicting a workspace is lossless).
