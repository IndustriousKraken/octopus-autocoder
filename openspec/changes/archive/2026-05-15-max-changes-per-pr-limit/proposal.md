## Why

Currently a single polling iteration bundles every pending change that processes successfully into one PR. When a repository has a long queue (e.g. ten stacked refactors authored at once), the resulting PR is dozens of commits across thousands of lines — far too large for a human reviewer to do justice. The author of this project has hit exactly this case: ten queued changes in another repo would land as one ten-commit PR that nobody could review carefully.

A configurable per-repo cap keeps autocoder's "one pass → one PR" rhythm but bounds the size of that PR. Default `3` keeps PRs reviewable while still letting closely-related changes ship together. Operators with tighter or looser review tolerances can override per repo.

## What Changes

- **MODIFIED capability:** `orchestrator-cli`. The "Per-repository asynchronous polling loop" requirement gains a per-pass bound on how many changes it commits before stopping the queue walk and producing a PR. Subsequent pending changes wait for the next iteration.
- **Config:** new optional per-repo field `max_changes_per_pr: u32` on `RepositoryConfig`. When unset, falls back to the executor-level default `executor.max_changes_per_pr` if set, else the global default `3`. A configured value of `0` is a misconfiguration and is clamped to `1` with a WARN log at startup (matching the existing `perma_stuck_after_failures` clamp pattern).
- **Code:** `walk_queue` accepts a `max_changes` parameter. After each change is successfully `Archived` (or `ArchivedSelfHeal`), check `archived.len() >= max_changes`; if so, break out of the loop. `Escalated` and `Failed` outcomes do NOT count toward the cap because they did not produce a commit.
- **Resumed AskUser changes still count.** A pass that begins by resuming a previously-waiting change counts that resume against the cap once it archives.

## Impact

- Affected specs: `orchestrator-cli` (one MODIFIED requirement).
- Affected code: `autocoder/src/polling_loop.rs` (walk_queue signature + break condition; `execute_one_pass` plumbs the value through), `autocoder/src/config.rs` (new field on `RepositoryConfig` + executor-level fallback + clamp helper + tests).
- Behavior change: PRs are now bounded by `max_changes_per_pr`. Operators with queues longer than the cap will see their changes ship over multiple iterations instead of one giant PR.
- Default `3` is a reviewability heuristic, not a load-bearing number. Operators can set per-repo overrides.
- Breaking: no. Existing configs without the field get the default of 3, which only differs from current behavior if there were genuinely four-or-more changes piling up per iteration — a case that already produces unreviewable PRs.
