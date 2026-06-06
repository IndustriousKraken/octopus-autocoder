# Implementation tasks

## 1. Bundled fallback on empty split

- [ ] 1.1 In `review_pr_at_state_with`'s `ReviewerMode::PerChange` arm: compute `split_per_change_contexts(ctx)`; when it is empty, dispatch the bundled path (`reviewer.review(ctx)`) and return its report, instead of calling `review_per_change(&[])` → `synthesize_per_change_report(vec![])`.
- [ ] 1.2 Add an empty-input guard to `synthesize_per_change_report` so it can never be the source of a defaulted `Pass` (it is no longer reached with an empty vec from the per_change arm; the guard makes that invariant explicit rather than returning a `Pass` report).

## 2. Tests

- [ ] 2.1 `per_change` mode with an empty `archived_changes` context AND a non-empty diff/changed_files reviews bundled: exactly one reviewer invocation occurs, and the verdict is the one the (stubbed) bundled review returns — not a defaulted `Pass`.
- [ ] 2.2 The fallback bundled review receives the context's diff and changed files (assert on what the stub reviewer was handed, not on log/message wording).
- [ ] 2.3 Regression: `per_change` mode with a populated `archived_changes` (≥1 change) still dispatches one review per change and synthesizes them, with no bundled fallback.

## 3. Acceptance gate

- [ ] 3.1 `cargo test` passes for the autocoder crate.
- [ ] 3.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [ ] 3.3 `openspec validate a015-reviewer-per-change-bundled-fallback --strict` passes.
