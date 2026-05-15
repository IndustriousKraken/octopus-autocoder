## Decisions

### Default of 3 (not 1, not unlimited)
- **1** would be the most reviewable but doubles the wall-clock to drain a queue of N changes (N iterations × poll_interval). For a queue of ten changes at five-minute intervals, that's 50 minutes vs. one big PR right now.
- **Unlimited** matches today's behavior but is precisely what we're trying to fix — ten-commit PRs from a single iteration are unreviewable.
- **3** is the smallest cap that still lets closely-related changes ship together (e.g. a small refactor with a fix that depends on it). It also matches a common informal "three commits and I'm getting suspicious" reviewer threshold.
- Operators with strong opinions can override per repo. The default is a starting point, not a constraint.

### Per-repo override, with optional executor-level fallback
A pure executor-level setting would force operators with mixed repos (some narrow-focus, some monorepo-style) to pick one number for everything. A pure per-repo setting would make it tedious to set "two" for every one of ten repos. The two-tier model with per-repo precedence covers both ergonomic shapes for cheap.

Lookup order at iteration time: per-repo override → executor-level default → hardcoded `3`. No additional layers.

### Clamp `0` to `1` (do not error)
Matches the existing `perma_stuck_after_failures` precedent. `0` is obviously a misconfiguration (it would mean "produce a PR with zero commits" which is the same as "do nothing this iteration"). Erroring at startup is hostile to operators editing YAML; the WARN log + clamp gets the daemon running with sensible behavior while making the misconfiguration visible.

### Count only `Archived` / `ArchivedSelfHeal` outcomes
`Failed` and `Escalated` outcomes produce no commit and would never appear in the resulting PR. Counting them would mean a queue with two failures-in-a-row could ship a PR with zero commits despite the cap being `3` — surprising and useless.

### Resumed AskUser changes do count
A pass that begins by resuming a previously-waiting change produces a commit if it archives. That commit is in the same PR as any further pending-change commits, so it must count against the cap to keep the cap meaningful. Edge case: a resume that itself produces a new AskUser → the resume did NOT archive, no commit, no count.

### Where the cap is enforced
`walk_queue` is the natural site: it iterates over pending changes, classifies outcomes, and is the only function that decides "produce another commit" or "stop." Adding `max_changes: u32` as a parameter and `break;` after the post-archive bookkeeping is a 5-line change. Plumbing it from `execute_one_pass` keeps the resume-counting on the same counter the post-resume walk shares.

## Open Questions

None. The design is small, the defaults follow precedent, and there are no architectural choices that ripple beyond `polling_loop.rs` and `config.rs`.
