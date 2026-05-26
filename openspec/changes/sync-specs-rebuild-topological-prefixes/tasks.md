## 1. Delta parser

- [ ] 1.1 Add `pub fn parse_capability_deltas(spec_md: &str) -> Vec<DeltaEntry>` in `autocoder/src/cli/sync_specs.rs` (or a new sibling module `autocoder/src/cli/sync_specs_deps.rs` if size warrants). Public struct:
  ```rust
  pub enum DeltaEntry {
      Added { header: String },
      Modified { header: String },
      Removed { header: String },
      Renamed { from: String, to: String },
  }
  ```
  Parses markdown looking for `## ADDED Requirements`, `## MODIFIED Requirements`, `## REMOVED Requirements`, `## RENAMED Requirements` block headers and extracts every `### Requirement: <header>` line that follows until the next `## ` block or EOF. For RENAMED blocks, paired FROM / TO lines are required; a block with a stray FROM lacking a TO (or vice versa) is skipped with a WARN-log (don't fail the scan over a malformed delta).
- [ ] 1.2 Tests:
  - ADDED block with one requirement returns one `Added` entry.
  - MODIFIED block with multiple `### Requirement: ...` entries returns one Modified per header.
  - RENAMED block with a FROM/TO pair returns one Renamed entry.
  - Malformed RENAMED (FROM without TO): skipped with WARN, no panic.
  - Spec file with no delta blocks at all returns an empty `Vec`.
  - Spec file with whitespace variants around `## ADDED Requirements` (trailing spaces, tabs) still parses.

## 2. Build the dependency graph

- [ ] 2.1 Add `pub fn build_dependency_graph(archive_root: &Path) -> Result<DependencyGraph, ScanError>`. Walks `archive_root` entries that match the date-prefix regex, opens each entry's `specs/*/spec.md` files, parses each via `parse_capability_deltas`, and assembles:
  ```rust
  pub struct DependencyGraph {
      // For each (capability, header), which archived change first ADDED it
      // (alphabetical sort: the change name that sorts first wins for ties).
      pub originating_add: HashMap<(String, String), String>, // (cap, header) -> archive_dir_name
      // For each archived change, the set of (capability, header) it depends on
      // via MODIFIED / REMOVED / RENAMED-FROM.
      pub dependencies: HashMap<String, Vec<(String, String)>>,
  }
  ```
  Renamed blocks contribute BOTH a dependency (on the FROM header) and a potential new originating_add (for the TO header) — a RENAME operationally replaces an existing requirement with a renamed one. The TO header's originating_add is the RENAMING change itself for purposes of any subsequent MODIFY targeting the new name.
- [ ] 2.2 Tests:
  - Build against a fixture with two changes: one ADDs `Foo`, one MODIFIES `Foo`. Assert `originating_add[("cap", "Foo")] == "add-foo"` and `dependencies["modify-foo"] == [("cap", "Foo")]`.
  - Build against a fixture with RENAMED `Foo` → `Bar`: assert `dependencies["rename"] == [("cap", "Foo")]` and `originating_add[("cap", "Bar")] == "rename"`.
  - Fixture with no deltas anywhere returns an empty graph.
  - Fixture where two changes both ADD the same header (operator error): the first by alphabetical sort wins; a WARN is logged identifying the duplicate.

## 3. Topological sort + rename plan generation

- [ ] 3.1 Add `pub fn compute_dependency_prefix_renames(archive_root: &Path) -> Result<Vec<RenamePlan>, RebuildAbortReason>` that:
  1. Calls `build_dependency_graph`.
  2. Groups archive entries by date prefix.
  3. For each day-group, performs a stable topological sort honoring dependencies (Kahn's algorithm or equivalent; preserve original alphabetical order between unrelated entries).
  4. Compares the sorted order to the current alphabetical order. For each entry whose position needs to change, generates a `RenamePlan { from: String, to: String, dependency_chain: Vec<String> }` where `to` includes a fresh `aNN-` prefix after the date that forces the new alphabetical position.
  5. Returns the minimum set of renames needed (only entries whose position must change; entries already correctly placed are not prefixed).
- [ ] 3.2 `RebuildAbortReason` enum:
  ```rust
  pub enum RebuildAbortReason {
      Cycle { changes: Vec<String>, requirements: Vec<(String, String)> },
      CrossDayBackwardDependency {
          dependent: String,                          // archived earlier
          dependency: String,                         // archived later (the ADD)
          capability: String,
          requirement_header: String,
      },
      ScanFailed { source_change: String, error: String },
  }
  ```
  All three carry enough detail for the operator's chatops / log message to be actionable.
- [ ] 3.3 Prefix-assignment rule for renames: when a day-group has K entries that need reordering, assign `a01-`, `a02-`, …, `a{K:02}-` in topological order. Width is fixed two digits to keep the lexicographic sort stable. If K > 99 (vanishingly unlikely — that means 100 same-day archives all depending on each other), return `RebuildAbortReason::ScanFailed { error: "more than 99 same-day reorderable entries; manual intervention required" }`.
- [ ] 3.4 Tests:
  - Happy case: two-entry day with one MODIFY → ADD inversion produces exactly one `RenamePlan` for the ADD entry, prefixed `a01-`.
  - Three-entry chain (A ADDs, B MODIFIES, C MODIFIES same as B) with alphabetical-current-order C-B-A: produces two `RenamePlan` entries to put A first and B second.
  - No conflicts in any day-group: returns `Ok(vec![])`.
  - Cycle: A MODIFIES requirement added by B, B MODIFIES requirement added by A → returns `Err(RebuildAbortReason::Cycle { ... })`.
  - Cross-day backward dependency: day-D change MODIFIES requirement added by day-D' > D change → returns `Err(RebuildAbortReason::CrossDayBackwardDependency { ... })`.
  - Stability: entries with no mutual dependency stay in original alphabetical order.
  - Idempotency: an archive with all entries already correctly prefixed (e.g. from a prior rebuild) produces zero renames on the second run.

## 4. Apply renames + integrate into rebuild flow

- [ ] 4.1 Add `pub fn apply_rename_plan(archive_root: &Path, plan: &[RenamePlan]) -> Result<(), std::io::Error>` that iterates the plan and performs `std::fs::rename` for each entry. Atomic per-rename; if any rename fails, log the error with the from/to and continue trying others (so a single permission glitch doesn't strand the entire plan in a half-applied state). After the call returns, log a summary line with the count of attempted vs successful renames.
- [ ] 4.2 In `rebuild_canonical`, immediately after validate_args and before the chronological-enumeration loop:
  1. Call `compute_dependency_prefix_renames`.
  2. On `Err(reason)`: log ERROR with the structured reason, populate `RebuildReport.abort_reason = Some(reason)`, return early with the report so the caller surfaces it the same way the per-change failure path does today.
  3. On `Ok(vec![])`: proceed to the existing enumeration loop with no renames recorded.
  4. On `Ok(plan)`: call `apply_rename_plan`, populate `RebuildReport.prefix_renames = plan_records`, then proceed to the existing enumeration loop. The subsequent `read_dir + sort_by_key(|e| e.file_name())` already in place naturally picks up the new names in the correct order.
- [ ] 4.3 Extend `RebuildReport`:
  ```rust
  pub struct RebuildReport {
      // existing fields...
      pub prefix_renames: Vec<RenameRecord>,           // empty when no renames applied
      pub abort_reason: Option<RebuildAbortReason>,    // Some when the pre-pass aborted
  }
  pub struct RenameRecord {
      pub from: String,
      pub to: String,
      pub day: String,                                 // "2026-05-14"
      pub dependency_summary: String,                  // human-readable explanation
  }
  ```
- [ ] 4.4 Tests:
  - Integration test: fixture archive with the `no-op-completion-is-failure` vs `self-healing-deployment` inversion. Run `rebuild_canonical`. Assert: report's `prefix_renames` has exactly one entry; the directory was actually renamed on disk; the subsequent chronological replay processes both changes successfully.
  - Cycle in fixture: `rebuild_canonical` returns a report with `abort_reason = Some(Cycle { ... })` and no canonical-spec files were modified.
  - Cross-day fixture: same shape, `abort_reason = Some(CrossDayBackwardDependency { ... })`, no canonical-spec files modified.

## 5. Chatops notification

- [ ] 5.1 In `autocoder/src/polling_loop.rs`, in the rebuild's chatops-notification path (currently posts the PR-opened notification), insert a NEW notification post BEFORE the PR notification when `report.prefix_renames` is non-empty. The new notification's text:
  ```
  🔀 <repo>: rebuild applied dependency-prefix renames in <N> day-group(s)
    <day-1>:
      <from> → <to>
        (<dependency_summary>)
      <from> → <to>
        ...
    <day-2>:
      ...
  ```
  Groups renames by their `day` field. The `dependency_summary` is the human-readable explanation generated in `compute_dependency_prefix_renames` (e.g. "dependency of `no-op-completion-is-failure`, which MODIFIES requirement \"...\" added here").
- [ ] 5.2 The notification fires regardless of whether the rebuild ultimately succeeded — operators want to know what was renamed even if a downstream replay failed. The notification is best-effort: if `post_notification` errors, log at ERROR and proceed (do NOT block PR creation).
- [ ] 5.3 If `report.abort_reason` is `Some(_)`, the rebuild did not produce a PR. Instead, post a separate notification describing the abort:
  ```
  ❌ <repo>: rebuild aborted — <abort-summary>. No archives were renamed; no canonical specs were modified. Operator action required.
  ```
  The abort-summary maps each `RebuildAbortReason` variant to a one-line description naming the offending change(s) and requirement(s).
- [ ] 5.4 Tests:
  - Snapshot test: with one rename, the notification text matches the documented `🔀` shape.
  - With multiple renames spanning two day-groups, the text is grouped by day.
  - With zero renames (Ok empty), no rename-notification is posted.
  - With a cycle abort, the `❌` notification is posted and contains both involved change names.
  - With `post_notification` returning Err, no panic; the rebuild's main path continues.

## 6. PR body addition

- [ ] 6.1 When `report.prefix_renames` is non-empty, the rebuild's generated PR body SHALL include a new section listing the renames in the same format as the chatops notification, BEFORE the existing "Canonical spec files" section. The operator reviewing the PR sees the renames first and can decide whether to keep, edit, or reject them.
- [ ] 6.2 When `report.abort_reason` is `Some(_)`, no PR is generated. The chatops `❌` notification is the only operator-facing output.

## 7. Spec delta

- [ ] 7.1 The ADDED requirement in `openspec/changes/sync-specs-rebuild-topological-prefixes/specs/orchestrator-cli/spec.md` codifies: the pre-scan obligation, the topological-sort + minimum-rename rule, the `aNN-` prefix convention (placement, width, sequence-within-day-group), the cycle / cross-day-backward-dep error conditions that abort the rebuild, the idempotency rule, and the chatops notification rules for both the success-with-renames and the abort cases.

## 8. Verification

- [ ] 8.1 `cargo test` passes (new + existing).
- [ ] 8.2 `openspec validate sync-specs-rebuild-topological-prefixes --strict` passes.
- [ ] 8.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
