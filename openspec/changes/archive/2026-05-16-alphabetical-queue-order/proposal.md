## Why

`time-based-change-ordering` switched `queue::list_pending`'s sort key from entry name to `proposal.md` modification time, intending to capture authoring order. In practice this signal carries no information for the actual operator workflow:

- A fresh clone writes every file at clone time → identical mtimes → sort degenerates to the alphabetical tiebreaker.
- A `git pull` that brings in N new commits (whether one spec per commit or N specs in one) writes the touched files at pull time → identical mtimes → alphabetical tiebreaker.
- The only workflow where mtime captures authoring order is one-spec-per-push with a daemon poll between each, which is not how operators batch related stacked changes.

In addition, `git reset --hard` (used by the startup dirty-recovery path) only rewrites files that differ from the target — so a change that *survived* a failed pass (e.g. wasn't archived) keeps its older mtime while its dependencies (which got moved and restored) get fresh mtimes. The failed change then sorts *before* its dependencies, inverting the intended order. This was observed in production after a daemon restart.

The mtime approach is structurally fragile (git doesn't propagate mtimes through commits at all, only "when did my local git write this file") and provides no real signal for the workflows it was designed for. Reverting to alphabetical-by-name is predictable, deterministic, and survives every git operation unchanged. Operators with stacked dependencies use prefixes (`01-`, `02-`) to encode order explicitly when they need it; the few keystrokes are a fair price for behavior the operator can actually predict.

## What Changes

- **MODIFIED capability:** `openspec-queue-engine`'s "Enumerate ready changes" requirement. The sort key is again the entry name, ascending.
- **Code:** `queue::list_pending` reverts to `out.sort()` (Vec<String> alphabetical sort). The `(SystemTime, String)` pair construction is removed.
- **Tests:** the three mtime-specific tests (`list_pending_orders_by_proposal_mtime_ascending`, `list_pending_breaks_mtime_ties_alphabetically`, `list_pending_excludes_perma_stuck_after_ordering_change`) are removed. The last one's perma-stuck check is already covered by the older `list_pending_excludes_perma_stuck` test.
- **Dependency:** `filetime` dev-dependency removed from `autocoder/Cargo.toml` — it was added solely for the tied-mtime test.
- **README:** "Queue order" subsection is rewritten to describe alphabetical ordering and recommend prefixes for explicit sequencing.

## Impact

- Affected specs: `openspec-queue-engine` (one MODIFIED requirement).
- Affected code: `autocoder/src/queue.rs` (revert the sort step + drop the `SystemTime` import).
- Behavior change: operators with stacked changes will see them processed alphabetically. If alphabetical happens to match authoring order (common for descriptively-named refactor stacks), no operator action needed. Otherwise, rename with a numeric prefix.
- Breaking: yes, for operators who configured their queue intending mtime-based order. Mitigation: the mtime approach was unreliable anyway; if their queue was working by luck, alphabetical is at least predictable.
