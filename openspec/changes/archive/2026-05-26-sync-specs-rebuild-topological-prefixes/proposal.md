## Why

`autocoder sync-specs --rebuild` walks archived changes in alphabetical order on the directory name (date prefix is the primary key; within-day order is alphabetical-on-slug because that is what `entries.sort_by_key(|e| e.file_name())` produces). Within a day, alphabetical-on-slug is arbitrary with respect to dependency direction.

A real-world rebuild surfaced the failure mode:

- `2026-05-14-no-op-completion-is-failure` MODIFIES `### Requirement: Reject archive-only iterations as Failed`.
- `2026-05-14-self-healing-deployment` is the change that ADDED that requirement.
- Both archived on 2026-05-14; `n` < `s` alphabetically.
- The MODIFY ran first against a canonical state that did not yet contain the requirement; openspec aborted; autocoder caught the silent skip and rolled back; the rebuild reported one failure that an operator had to chase by reading spec deltas.

The chronological-replay strategy is correct in principle (everything that was archived earlier should be applied earlier). The failure is in the within-day tiebreaker. Alphabetical sort gives a deterministic answer but the answer is unrelated to spec-delta dependency direction.

The `sync-specs-rebuild-atomicity` and `sync-specs-rebuild-aborted-output-detection` changes that preceded this one made the failure cleanly diagnosable (rollback runs, message says "openspec refused to apply: <requirement> - not found"). They did not prevent the failure. This change closes that loop by reordering same-day archives whose alphabetical sort disagrees with their dependency graph, and by persisting the reordering as an `aNN-` prefix on the archive directory name so subsequent rebuilds remain correctly ordered without any further analysis.

## What Changes

**Pre-scan pass.** Before the chronological enumeration loop runs, the rebuild SHALL scan every archived change's spec deltas to build a per-capability map from requirement header to the first change that ADDED it. The scan parses each `archive/<entry>/specs/<capability>/spec.md` for `## ADDED Requirements`, `## MODIFIED Requirements`, `## REMOVED Requirements`, and `## RENAMED Requirements` blocks; for each block, extract the requirement headers it operates on (or the FROM/TO pair for RENAMED). The result is a dependency graph: each MODIFIED / REMOVED / RENAMED-FROM entry creates an edge from the operating change to the change that originally ADDED the header.

**Topological sort within day-groups.** For each date prefix's set of archived changes, apply a topological sort using the dependency graph. The sort respects three rules:

1. Within a day-group, every ADDING change SHALL be sorted before every MODIFYING / REMOVING / RENAMING-FROM change that references the same `(capability, requirement_header)`.
2. Outside of dependency constraints, the original alphabetical order on slug is preserved (stable sort) — entries that have no dependencies on each other do not shuffle.
3. Days that have no within-day dependencies produce no renames.

**Apply `aNN-` prefix renames to force the sorted order.** After topological sort, for each entry whose new position differs from its current alphabetical position within the day-group, prefix the directory name with `aNN-` (two-digit zero-padded sequence: `a01-`, `a02-`, …, `a99-`) such that the new alphabetical order matches the topological order. Only entries whose position must change SHALL be renamed; entries already in the correct position SHALL NOT receive a prefix. The minimum number of renames possible should be applied — typically just the dependency-providers within a day, prefixed to sort before their dependents.

**Persist the renames as archive-directory moves.** Renames are filesystem operations (`std::fs::rename` from `archive/<original>/` to `archive/<original-with-prefix>/`). They become part of the rebuild's git-tracked changes alongside the canonical spec updates and are included in the same PR. The renames are reversible: an operator who disagrees with the proposed order can edit the PR to remove or alter them, or close the PR entirely.

**Chatops notification.** When at least one rename was applied during a rebuild, post a single notification to the repo's resolved chatops channel listing the renames in `FROM → TO` form, the day-group they belong to, and the dependency that triggered each. The notification fires before the PR is opened so an operator watching the channel sees the renames immediately and can intercept the rebuild PR for review. Format:

```
🔀 <repo>: rebuild applied dependency-prefix renames in 1 day-group
  2026-05-14:
    self-healing-deployment → a01-self-healing-deployment
      (dependency of no-op-completion-is-failure, which MODIFIES
       "Reject archive-only iterations as Failed" added here)
```

When no renames were applied, no notification fires (the rebuild's existing PR notification covers the normal case).

**Idempotency.** Running the rebuild a second time on already-prefixed archives produces no further renames. The prefixes are sticky: once the archive directory's name includes `a01-foo`, subsequent rebuilds see that name as the alphabetical position and find no conflict. This is the user's described benefit: the dependency order is encoded in the directory names from then on.

**Errors that abort the rebuild.** Two delta-graph conditions are unresolvable by within-day prefix renames; both SHALL abort the rebuild with a clear error before any renames or canonical-spec updates are applied:

1. **Cycle** — Change A MODIFIES a requirement added by B, and B MODIFIES a requirement added by A. No topological order exists. The rebuild logs the cycle (naming both changes and both `(capability, requirement)` pairs) and exits non-zero.
2. **Cross-day backward dependency** — A change archived on day D MODIFIES / REMOVES / RENAMES-FROM a requirement first added by a change archived on day D' > D. The prefix mechanism is within-day only; crossing day boundaries would require lying about archive dates, which is worse than within-day sort lying. The rebuild logs the offending pair and exits non-zero.

Both errors are operator-fix territory: the operator inspects the archives, decides whether to merge the conflicting changes, rewrite one to remove the dependency, or manually re-date.

## Impact

- **Affected specs:** `orchestrator-cli` — one ADDED requirement covering the pre-scan, topological sort, prefix renames, chatops notification, and the two abort-conditions.
- **Affected code:**
  - `autocoder/src/cli/sync_specs.rs` — add a new pre-pass function `compute_dependency_prefix_renames(archive_root) -> Result<Vec<RenamePlan>, RebuildAbortReason>` that performs the scan + topological sort + plan generation. Add an `apply_rename_plan` helper that performs the fs renames. Wire both into the rebuild entry point before the existing chronological-enumeration loop.
  - `autocoder/src/cli/sync_specs.rs` — extend `RebuildReport` with a `prefix_renames: Vec<RenameRecord>` field so the report-printer and chatops notification can both surface what was renamed.
  - `autocoder/src/polling_loop.rs` — extend the rebuild's chatops post path to emit the rename notification when `report.prefix_renames` is non-empty. Use the existing `post_notification` channel resolution.
  - New module-internal helpers for delta parsing: `parse_capability_deltas(spec_md: &str) -> Vec<DeltaEntry>` returning `(BlockType, requirement_header_or_pair)` tuples. Pure-data, easy to unit-test.
  - Tests:
    - Pure-data tests for `parse_capability_deltas`: ADDED block with one requirement, MODIFIED with two, RENAMED with FROM/TO pair, REMOVED, malformed blocks (return empty rather than error so a single bad delta doesn't abort the scan).
    - Topological sort tests: trivial happy path (one dependency in a day-group), no-dependency day produces no renames, multi-dependency chain (A → B → C all same day), cycle returns an error.
    - Cross-day backward dependency returns an error naming both changes.
    - Idempotency test: build a fixture with already-correctly-prefixed archives, run the pre-pass, assert no renames are produced.
    - Apply-rename test: assert directory was moved on disk and that subsequent chronological enumeration sees the new name.
    - Chatops notification test: with non-empty `prefix_renames`, assert one `post_notification` call with the documented `🔀` shape; with empty, assert no call.

- **Operator-visible behavior:** rebuilds against archives with intra-day dependency conflicts now succeed automatically and surface the renames as a chatops notification + PR diff. Operators who disagree with the proposed order edit the PR before merging. Subsequent rebuilds against the merged-PR archive are no-ops with respect to renames.

- **Breaking:** no. Rebuilds against archives with no intra-day dependency conflicts produce identical output (no renames, no notification beyond the existing PR-opened message). The `aNN-` prefix convention overlaps with the user's stacked-change `aNN-` naming convention but at a different position (after the date, not at the start of the slug), so no semantic confusion arises — operators viewing `archive/2026-05-14-a01-self-healing-deployment/` see clearly that it's a prefix applied at archive time, not a stacked-change name.

- **Acceptance:** `cargo test` passes (new + existing). A rebuild against a fixture archive containing the documented `no-op-completion-is-failure` vs `self-healing-deployment` conflict produces a PR with `self-healing-deployment` renamed to `a01-self-healing-deployment` and zero failures in the report. A rebuild against an archive with a cycle aborts with a clear error and applies no renames. A rebuild against an archive with no conflicts produces zero renames and the same PR shape the existing rebuild produces today.
