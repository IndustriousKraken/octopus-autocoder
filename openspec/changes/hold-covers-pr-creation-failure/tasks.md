# Tasks: hold-covers-pr-creation-failure

## 1. Marker fields

- [ ] 1.1 In `autocoder/src/push_block.rs`, add `failed_step` (`push` | `pr_creation`, serde-default `push`) and `issue_slugs: Vec<String>` (serde-default empty) to the marker struct, so existing on-disk markers deserialize unchanged.

## 2. Write the hold on PR-creation failure

- [ ] 2.1 In `autocoder/src/polling_loop/pass.rs`, when `open_pull_request` returns `Err` after a successful push and the pass carried at least one change or issue unit, write the push-block marker (tip, change slugs, issue slugs, reason, `failed_step: pr_creation`) before propagating the error; keep the existing throttled `PrCreationFailure` alert, extending its text to say the work is preserved and PR creation will be retried.
- [ ] 2.2 Include issue-bearing passes in the existing push-failure hold: the write guard currently checks processed changes only; it must also trigger when only issue units were processed, recording their slugs in `issue_slugs`.

## 3. Resume path

- [ ] 3.1 Verify the existing push-block resume flow handles a `pr_creation` hold end-to-end: tip match → no branch recreation, no executor → push retry (no-op on the already-pushed tip) → PR creation retried → marker removed on success. Add branching only if the no-op-push assumption fails in practice.
- [ ] 3.2 Make PR-body derivation on the resume path account for `issue_slugs` alongside change slugs.

## 4. Tests

- [ ] 4.1 Test: a PR-creation failure after a successful push writes the marker with `failed_step: pr_creation`; the next pass does not recreate the branch, does not run the executor, retries PR creation only, and clears the marker on success.
- [ ] 4.2 Test: a push failure on a pass that carried only issue units writes the marker with the issue slugs; the next pass resumes at the push step without re-running the executor.
- [ ] 4.3 Test: legacy markers without the new fields still deserialize and resume as push holds.
- [ ] 4.4 Test: stale-marker handling (tip mismatch) is unchanged for both `failed_step` values.

## 5. Validation

- [ ] 5.1 Run `openspec validate hold-covers-pr-creation-failure --strict` and fix any findings.
