# orchestrator-cli — delta for a65-workspace-cache-eviction

## ADDED Requirements

### Requirement: Workspace cache LRU eviction under a size cap
The daemon SHALL support an optional cap on the total size of the per-repo workspace cache (`<cache>/workspaces/`), configured via `cache.workspaces_max_gb` (`Option<u64>`; unset = unbounded, the default). When the cap is set, the daemon SHALL keep the workspace cache under it by evicting least-recently-used IDLE workspaces. Whole-workspace eviction is language-agnostic: it removes entire least-recently-used clones (`<cache>/workspaces/<key>`) rather than reasoning about which subdirectories are build artifacts. An evicted repo re-clones via the existing workspace-init path on its next iteration; eviction is lossless because per-PR revision state AND audit state live in the state directory, NOT the workspace.

Eviction SHALL run at a repo's iteration start, before the repo does work: if `cache.workspaces_max_gb` is set AND the measured total exceeds the cap, the daemon evicts least-recently-used idle workspaces (oldest last-used first) until the total is under the cap OR only non-evictable workspaces remain. The daemon SHALL NEVER evict the repo currently iterating NOR any workspace holding a per-repo busy marker / active lock. If the non-evictable set alone exceeds the cap, the daemon SHALL log a WARN that it cannot reclaim to target AND proceed — eviction SHALL NEVER block or fail an iteration. Each eviction SHALL log the workspace key, the reclaimed size, AND the new total. "Least-recently-used" is ordered by the last iteration that used each workspace (a daemon-maintained per-workspace last-used timestamp).

When `cache.workspaces_max_gb` is unset, the cache is unbounded (today's behavior) AND the daemon SHALL log a ONE-TIME startup notice that the workspace cache is unbounded AND name the field that bounds it, so the unbounded-growth failure mode is discoverable before it exhausts a disk.

#### Scenario: Unset cap is unbounded with a one-time startup nudge
- **WHEN** `cache.workspaces_max_gb` is unset
- **THEN** no eviction occurs (the cache grows unbounded, as today)
- **AND** the daemon logs exactly one startup notice that the workspace cache is unbounded AND names `cache.workspaces_max_gb` as the way to bound it

#### Scenario: Over-cap cache evicts oldest idle workspaces
- **WHEN** `cache.workspaces_max_gb` is set AND the total `<cache>/workspaces/` size exceeds it at a repo's iteration start
- **THEN** the daemon evicts least-recently-used IDLE workspaces (oldest last-used first), removing each whole `<cache>/workspaces/<key>`, until the total is under the cap OR only non-evictable workspaces remain
- **AND** each eviction logs the workspace key, the reclaimed size, AND the new total

#### Scenario: The current and busy workspaces are never evicted
- **WHEN** eviction runs
- **THEN** the repo currently iterating is NOT evicted
- **AND** any workspace holding a per-repo busy marker / active lock is NOT evicted (a concurrently-iterating repo is never removed out from under itself)

#### Scenario: Best-effort when the active set exceeds the cap
- **WHEN** the non-evictable workspaces (current + busy) alone exceed the cap
- **THEN** the daemon logs a WARN that it cannot reclaim to the target
- **AND** the iteration proceeds — eviction never blocks or fails an iteration

#### Scenario: An evicted repo re-clones losslessly
- **WHEN** a repo whose workspace was evicted reaches its next iteration
- **THEN** the daemon re-creates the workspace via the existing workspace-init (clone) path
- **AND** the repo's per-PR revision state AND audit state (resolved from the state directory, not the workspace) are intact
