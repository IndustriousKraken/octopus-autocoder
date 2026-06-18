# Tasks

## 1. Issue marker primitive + ready-list skip

- [x] 1.1 In `lanes/issues.rs`, add the per-issue perma-stuck marker helpers, reusing the `.perma-stuck.json` filename (already excluded via `.git/info/exclude`): a path resolver, an `is_perma_stuck(workspace, slug)` presence check, AND an atomic `write_perma_stuck(workspace, slug, count, reason)`. No daemon-side remove helper is added — the operator removes the marker to unpark, and completion removes it by archiving the whole directory (a remove helper would have no production caller).
- [x] 1.2 In `issues::list_ready`, skip a `issues/<slug>/` whose `.perma-stuck.json` marker is present, mirroring the existing lock-file skip.

## 2. Consume the failure counter

- [x] 2.1 Remove the `#[allow(dead_code)]` on `lanes::state::failure_count` and consult it from the walker so the recorded counter actually gates re-selection.

## 3. Park on non-progress in the walker

- [x] 3.1 Thread `executor.perma_stuck_after_failures` from `polling_loop/commits.rs` (`run_pass_through_commits` already holds it) into `run_issues_lane` → `walker::walk_issues` → `process_one_issue` → `map_issue_outcome`.
- [x] 3.2 Add a `park_issue` helper in the walker: write the marker, clear the counter (so a later unpark gets a fresh attempt budget), post the operator-visible chatops alert (name the issue, attempt count, last reason), AND log. Best-effort on marker-write failure: still alert.
- [x] 3.3 Wire outcomes: retryable failures (`Failed`, `IterationRequested`, `PreconditionUnmet`, executor `Err`) record a failure and park when the counter reaches the threshold; `Escalated` AND `KickedBackToChanges` park immediately; `Aborted` does neither.

## 4. Clear on completion

- [x] 4.1 On `Archived`, clear the failure counter (so a slug reused later starts clean); the marker, if any, is removed implicitly by the archive move that relocates the whole `issues/<slug>/` directory.

## 5. Tests

- [x] 5.1 `list_ready` excludes an issue carrying a `.perma-stuck.json` marker; removing it makes the issue selectable again.
- [x] 5.2 A retryable failure parks the issue exactly when the counter reaches the threshold (not before).
- [x] 5.3 An escalate outcome AND a kick-back outcome each park the issue on a single attempt.
- [x] 5.4 An abort outcome neither increments the counter nor parks the issue.
- [x] 5.5 Parking posts a chatops alert (assert the alert is emitted and names the slug, not its exact wording).
