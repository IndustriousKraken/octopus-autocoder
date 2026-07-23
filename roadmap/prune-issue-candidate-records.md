---
title: Prune issue-candidate records after an age threshold
status: proposed
added: 2026-07-23
---

`<state_dir>/issue-candidates/<id>.json` records are never deleted. Growth is
slow (~2KB per triaged report), so this is a slow-day item, not urgent.

Proposed policy: prune records older than ~30 days. Rationale: an issue that
has not resolved in 30 days is most likely forgotten and would need to be
restarted anyway.

Two constraints a naive age-prune must respect (from the 2026-07 issues-lane
work):

1. **`promoted` records are the durable queue** (per
   `promoted-issues-survive-workspace-cleaning`): deleting one is the tombstone
   that retires the issue, and the reconciler stops re-materializing its unit.
   Age-pruning a promoted-but-unfinished record silently drops queued work —
   acceptable under the "forgotten anyway" rationale, but it should log/alert
   what it retired rather than vanish it.
2. **`posted` records are the dedup memory**: pruning one means the same
   GitHub report can be re-triaged and re-posted to chatops as a fresh
   candidate. After 30 days that re-post is arguably a feature (a reminder),
   but it costs a triage LLM call per resurfaced report.

Suggested shape when picked up: prune on daemon startup or a slow cadence;
30-day threshold measured from `posted_at` (or a `promoted_at` if added);
skip records whose slug still exists in the repo's `issues/` (actively
queued/working); one log line per pruned record.
