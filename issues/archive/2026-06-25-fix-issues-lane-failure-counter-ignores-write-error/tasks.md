# Tasks

## 1. Stop discarding the failure-state write result in the issues walker
- [x] 1.1 In `autocoder/src/lanes/walker.rs::fail_and_maybe_park`, replace the
  pair
  ```rust
  let _ = state::record_failure(paths, workspace, slug, &reason);
  let count = state::failure_count(paths, workspace, slug);
  ```
  with a `match` on `state::record_failure(paths, workspace, slug, &reason)`:
  on `Ok(n)` bind `count = n`; on `Err(e)` emit
  `tracing::warn!(url = %repo.url, issue = %slug, "issues lane: failed to record consecutive-failure state: {e:#}");`
  and `return IssueStep::Failed { reason };` (do not park — the threshold cannot
  be confirmed). Mirror `handle_failure_counter` in
  `autocoder/src/polling_loop/preflight_checks.rs:592-602`.
- [x] 1.2 Keep the existing threshold check (`if count >= perma_stuck_threshold { park_issue(...).await; }`)
  driven by the bound `count`, and keep the final `IssueStep::Failed { reason }`
  return on the success path.
- [x] 1.3 Do not remove `state::failure_count` (it remains the lane's read
  accessor used elsewhere); only stop using it for the park decision inside
  `fail_and_maybe_park`.

## 2. Test the threshold park is driven by the returned count
- [x] 2.1 In the `autocoder/src/lanes/walker.rs` test module, add a test that
  drives `perma_stuck_threshold` consecutive retryable failures for one slug and
  asserts the issue is parked (the `.perma-stuck.json` marker exists for the
  slug) on the threshold pass — confirming the park decision tracks the
  incrementing counter returned by `record_failure`, not a stale re-read. Reuse
  the existing test scaffolding/`crate::testing::test_daemon_paths` helper.

## 3. Verify
- [x] 3.1 Run `cargo test -p autocoder lanes::walker` (or the crate's lane tests)
  and confirm the new and existing issues-lane tests pass.
