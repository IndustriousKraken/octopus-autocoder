## Why

The current queue-blocking policy has an asymmetry: `.in-progress` (AskUser waiting) AND `.needs-spec-revision.json` markers BLOCK subsequent pending-change processing for the same repo, while `.perma-stuck.json` only EXCLUDES the affected change from `list_pending` AND lets subsequent changes proceed. Both classes of marker represent the same kind of problem — "this change couldn't be completed AND requires operator action" — but the queue treats them differently.

This bit the operator yesterday: a07 went perma-stuck due to a bad MODIFIED header. Dependent changes a08 / a09 (named for stack ordering AND assuming a07's symbols would be available) were still in the queue. The daemon happily picked them up, ran them, observed they couldn't reference a07's not-yet-archived contributions, AND failed. By the time the operator caught it, multiple changes had churned.

The principled fix: any marker that says "agent could not complete this change; operator action required" should block the queue. The four current markers:

| Marker | Today's queue effect | Proposed queue effect |
|---|---|---|
| `.in-progress*` (AskUser waiting) | Blocks ✓ | Same (unchanged) |
| `.needs-spec-revision.json` | Blocks ✓ | Same (unchanged) |
| `.perma-stuck.json` | Drops change; queue continues ✗ | **Blocks queue** |
| `.needs-spec-revision.json` with `unarchivable_deltas` (from a17) | Blocks ✓ (it's a needs-spec-revision marker) | Same (a17 already handles this) |

The escape hatch addresses the legitimate case of "I know this change is broken; please skip it AND keep going with everything else." The operator opts in via a new chatops verb.

## What Changes

**`.perma-stuck.json` blocks the queue by default.** The polling loop's pre-pending-walk check that currently halts on `.in-progress` or `.needs-spec-revision.json` markers SHALL be extended to also halt on `.perma-stuck.json`. The change still gets excluded from `list_pending` (existing behavior) AND now additionally blocks subsequent changes per the same-repo blocking policy.

**New chatops verb `@<bot> ignore-and-continue <repo-substring> <change-slug>`.** Stamps `.ignore-for-queue.json` alongside the existing blocking marker. The queue-blocking check sees the ignore-marker AND treats the original as "don't block siblings" — the change stays excluded from `list_pending` (it's still broken; agent can't run it) but doesn't gate subsequent changes. Operator's saying "I know this one's broken; the rest are independent; proceed."

**`@<bot> clear-ignore <repo-substring> <change-slug>`.** Removes the ignore-marker. The original blocking marker (perma-stuck OR needs-spec-revision) is still in place; the queue resumes blocking on it. Used when the operator wants to revert the "skip and continue" decision — e.g. they decide they DO want the dependent changes to wait until the broken one is fixed.

**Status reply surfaces both markers.** The `@<bot> status` reply's "active markers" section names every marker present per change. With both `.perma-stuck.json` AND `.ignore-for-queue.json` present, the section reads:

```
active markers:
  a07-...: .perma-stuck.json (ignore-for-queue: yes — queue not blocked)
  a09-...: .needs-spec-revision.json (blocking queue)
```

The "(ignore-for-queue: yes — queue not blocked)" annotation makes the operator's decision visible.

**The ignore-marker is git-tracked.** Like `.perma-stuck.json` AND `.needs-spec-revision.json`, the file lives inside `openspec/changes/<change>/` AND is intentionally committed so it survives `wipe-workspace` (consistent with the existing per-change-directory marker pattern).

**Per-change-directory markers' git-tracked status.** Re-confirming the existing rule: `.perma-stuck.json`, `.needs-spec-revision.json`, AND the new `.ignore-for-queue.json` are intentionally NOT in `.git/info/exclude`. They're tracked AND committed so operators can see them on GitHub AND so `wipe-workspace` preserves them via the re-clone. This is distinct from the workspace-bookkeeping files (`.audit-state.json`, `.alert-state.json` post-`a16`) which are daemon-only AND live elsewhere.

## Impact

- **Affected specs:**
  - `orchestrator-cli` — one MODIFIED requirement: `Perma-stuck change detection` — adds the queue-blocking behavior AND describes how the ignore-marker downgrades it. All 6 existing scenarios preserved.
  - `orchestrator-cli` — one ADDED requirement: `Ignore-for-queue marker downgrades blocking-marker behavior without unblocking the change itself`.
  - `chatops-manager` — one MODIFIED requirement: the operator-verbs table gains `ignore-and-continue` AND `clear-ignore`. All existing verbs preserved.
  - `chatops-manager` — the status-reply requirement (from `a11`) gains coverage for the new "active markers" annotation. MODIFIED.
  - `project-documentation` — one ADDED requirement: `OPERATIONS.md AND CHATOPS.md document the queue-blocking change AND the ignore verbs`.
- **Affected code:**
  - `autocoder/src/polling_loop.rs` — the pre-pending-walk marker-check loop gains a branch for `.perma-stuck.json` (currently only `.in-progress*` AND `.needs-spec-revision.json` are checked). The check skips when an accompanying `.ignore-for-queue.json` is present in the same change directory.
  - `autocoder/src/chatops/operator_commands.rs` (or wherever the inbound verb dispatcher lives) — register `ignore-and-continue` AND `clear-ignore`. Each takes `<repo-substring> <change-slug>` args, resolves the workspace, AND stamps OR removes `<workspace>/openspec/changes/<change>/.ignore-for-queue.json`.
  - The status-reply composer gains a check for `.ignore-for-queue.json` alongside its existing marker scan, AND annotates the relevant line in the "active markers" section.
  - `docs/OPERATIONS.md` — update the perma-stuck section to describe the new queue-blocking behavior AND name the ignore-and-continue escape hatch.
  - `docs/CHATOPS.md` — extend the operator-verbs table with the two new verbs.
- **Operator-visible behavior:**
  - A perma-stuck change halts further processing of subsequent pending changes in the same repo until the operator decides what to do. The chatops alert for perma-stuck (existing) is unchanged; the queue-blocking is an additional consequence.
  - `@<bot> ignore-and-continue <repo> <change>` is the explicit "skip this one AND keep going" signal. Stamps a marker. Queue resumes.
  - `@<bot> clear-ignore <repo> <change>` reverts. Queue blocks again.
  - `@<bot> clear-perma-stuck <repo> <change>` (existing) removes both the perma-stuck marker AND the ignore-marker as part of full resolution.
- **Breaking:** behavior change for repos with multiple in-flight changes where one perma-stucks. Pre-spec: subsequent changes continued processing. Post-spec: queue halts until operator decides (`clear-perma-stuck` to retry, or `ignore-and-continue` to skip). The new default is safer for stacked changes (the common autocoder pattern) AND aligned with how needs-spec-revision already behaves.
- **Acceptance:** `cargo test` passes; `openspec validate a18-operator-action-markers-block-queue --strict` passes. New tests:
  - Workspace with one perma-stuck change AND one pending change → queue halts (no executor invocation on the pending change).
  - Same workspace with the addition of `.ignore-for-queue.json` on the perma-stuck change → queue resumes (executor invoked on the pending change).
  - `@<bot> ignore-and-continue` stamps the marker file via the control socket.
  - `@<bot> clear-ignore` removes it.
  - Status reply with both markers present annotates correctly.
