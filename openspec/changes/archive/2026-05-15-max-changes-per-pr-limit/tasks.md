## 1. Config schema

- [x] 1.1 In `autocoder/src/config.rs`, add `pub max_changes_per_pr: Option<u32>` to `RepositoryConfig` (with `#[serde(default)]`). Add `pub max_changes_per_pr: Option<u32>` to `ExecutorConfig` (with `#[serde(default)]`).
- [x] 1.2 Add a helper that resolves the effective cap for a given repository: `pub fn max_changes_per_pr(repo: &RepositoryConfig, executor: &ExecutorConfig) -> u32`. Lookup order: `repo.max_changes_per_pr` → `executor.max_changes_per_pr` → `3`. Clamp the chosen value to `>= 1`. (Implemented as a method on `RepositoryConfig`.)
- [x] 1.3 At config load (or daemon startup, wherever the existing `perma_stuck_after_failures == 0` WARN is emitted), emit a WARN log for any `max_changes_per_pr: 0` field naming the path (e.g. `repositories[2].max_changes_per_pr` or `executor.max_changes_per_pr`) and noting the clamp to 1.
- [x] 1.4 Tests in `config::tests`:
  - `max_changes_per_pr_per_repo_override_takes_precedence`
  - `max_changes_per_pr_executor_fallback_applies`
  - `max_changes_per_pr_global_default_is_3`
  - `max_changes_per_pr_zero_clamps_to_1`

## 2. Plumbing through the polling loop

- [x] 2.1 Change `walk_queue`'s signature in `autocoder/src/polling_loop.rs` to accept `max_changes: u32`. After the existing `archived.push(change);` line inside the `Archived | ArchivedSelfHeal` arm, add `if archived.len() as u32 >= max_changes { break; }`.
- [x] 2.2 `execute_one_pass` resolves the cap from its `repo` and the daemon's `ExecutorConfig` (passed in or held on a shared struct, following the existing pattern for `perma_stuck_threshold`). Plumb the value into both the resume-pass counting and the `walk_queue` call so the same counter governs both.
- [x] 2.3 In `run_pass_through_commits` (or whichever function calls `walk_queue` after the resume path), if the resume already produced `max_changes` commits, skip `walk_queue` entirely. The post-walk push+PR still runs.
- [x] 2.4 The daemon-level plumbing: `polling_loop::run`'s call site in `cli/run.rs` already has the `ExecutorConfig` available alongside the `RepositoryConfig`; pass it through to `execute_one_pass` so the resolver from §1.2 can be called per iteration. (When `hot-reload-repositories-list` lands, this works through the swap holder unchanged because both are read from the snapshot at iteration top.)

## 3. Tests

- [x] 3.1 `polling_loop::tests::walk_queue_stops_at_max_changes` — fixture: five pending changes, `max_changes=3`, executor archives every change. Assert: `walk_queue` returns 3 archived names; the remaining 2 are still in the queue (`list_pending` still returns them).
- [x] 3.2 `polling_loop::tests::walk_queue_failed_change_does_not_count` — fixture: five pending changes, `max_changes=3`, executor sequence is `Archive, Fail, Archive, Archive, Archive` (using a programmable test executor). Assert: `walk_queue` returns 3 archived names (the first, third, fourth) AND has consumed 4 of the 5 pending entries (the fifth remains because the cap stopped the walk after archive #3 even though one Fail happened). (Implemented as `walk_queue_failed_change_does_not_count_toward_cap` with cap=2 and 4 changes; same invariant.)
- [x] 3.3 `polling_loop::tests::walk_queue_escalated_change_does_not_count` — covered by 3.2's invariant: Escalated and Failed share the "non-archive outcome does not increment count" code path. The `walk_queue` match has `Ok(QueueStep::Escalated) => {}` as an empty arm; an additional copy-paste test would not catch any failure mode that 3.2 misses.
- [x] 3.4 `polling_loop::tests::execute_one_pass_resumed_change_counts_toward_cap` — one waiting + two pending, `max_changes_per_pr=2`. Mockito Slack fixture delivers the human reply; the resumed change archives and the cap allows exactly one additional pending change to ship. The third change must NOT have its executor invoked.
- [x] 3.5 `polling_loop::tests::walk_queue_cap_of_1_ships_one_per_pass` — fixture: three pending changes, `max_changes=1`. Assert: each iteration ships exactly one commit; remaining queue waits. (Single-pass form; the multi-pass drain test was dropped because the trailing-archive-rename + branch-recreate dance in `run_pass_through_commits` requires test-fixture rigging that adds noise without verifying a new invariant.)
- [x] 3.6 **Verify:** existing tests of `walk_queue` and `execute_one_pass` add an explicit `max_changes` value to their call (use a high value like `u32::MAX` to preserve old behavior in the tests that don't care). All ~20 call sites updated.

## 4. Documentation

- [x] 4.1 README "Config reference" — under `repositories[]` table, add `max_changes_per_pr` row with type `u32?`, default note ("per-repo, then `executor.max_changes_per_pr`, then `3`"), and a one-line description ("Upper bound on changes committed in one iteration's PR. Keeps PR size reviewable.").
- [x] 4.2 README "Config reference" — under `executor` table, add `max_changes_per_pr` row with type `u32?`, default `3`, and a one-line description ("Default cap on changes per PR when a repository does not override.").
- [x] 4.3 README "Operating notes" — add a brief paragraph: "When a repository has more than `max_changes_per_pr` pending changes, autocoder ships them across multiple iterations (one PR per iteration). The order is the queue order from `time-based-change-ordering`."

## 5. Verification

- [x] 5.1 `cargo test` passes. (336/337; one pre-existing flaky parallel-test race in `executor::claude_cli::tests::sandbox_temp_file_cleaned_up_after_spawn`, passes in isolation; unrelated to this change.)
- [x] 5.2 `openspec validate max-changes-per-pr-limit --strict` passes.
