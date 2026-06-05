# Implementation tasks

## 1. Aggregate reviewer-initiated revisions (`revisions.rs`)

- [x] 1.1 Group a single review's `<!-- reviewer-revision -->` comments instead of looping one executor run per comment. Build one revision instruction carrying all the concerns (e.g. a numbered list of the per-concern requests).
- [x] 1.2 Dispatch ONE executor revision run for the group; increment the auto-revision cap (`max_auto_revisions_per_pr`) by exactly one for the whole batch.
- [x] 1.3 Post one operator-visible summary of the concerns being addressed (not one terse comment per concern).
- [x] 1.4 Leave human `@<bot> revise <text>` handling as-is — per-request, bounded by the human-revise cap.

## 2. `auto_revise` tri-state (`code_reviewer.rs` / config)

- [x] 2.1 Parse `auto_revise` as `block | actionable | off`, default `block`. Map the legacy boolean: `true` → `actionable`, `false` → `off`.
- [x] 2.2 Gate the aggregated dispatch: `block` fires only when the effective verdict is `Block` (post-a004 escalation included); `actionable` fires on any actionable concern regardless of verdict; `off` never fires.
- [x] 2.3 Document the field + default in `docs/CODE-REVIEW.md` / `docs/CONFIG.md` (tri-state, legacy mapping, default `block`).

## 3. Tests

- [x] 3.1 A review with N≥2 actionable concerns dispatches exactly one revision run and increments the auto-revision cap by one (assert the single run + single increment, not N).
- [x] 3.2 Duplicate-targeting concerns in one review do not produce a second no-op run (one aggregated run).
- [x] 3.3 `auto_revise` default (`block`): a `Concerns` verdict does not dispatch; a `Block` verdict does.
- [x] 3.4 `auto_revise: actionable` dispatches on a `Concerns` verdict; `off` never dispatches.
- [x] 3.5 Legacy `true`/`false` map to `actionable`/`off`.

## 4. Acceptance gate

- [x] 4.1 `cargo test` passes for the autocoder crate.
- [x] 4.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [x] 4.3 `openspec validate a005-aggregate-reviewer-revisions --strict` passes.
