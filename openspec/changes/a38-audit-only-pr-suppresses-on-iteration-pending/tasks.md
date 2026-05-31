# Tasks

## 1. Iteration-pending detection helper

- [ ] 1.1 Add `fn list_iteration_pending_changes(workspace: &Path) -> Vec<String>` (OR similar) that globs `<workspace>/openspec/changes/*/.iteration-pending.json` AND returns the change names of every directory carrying the marker.
- [ ] 1.2 Unit-test: workspace with no markers returns empty Vec; workspace with one marker returns one name; workspace with two markers returns both names in sorted order.

## 2. Audit-only-PR path suppression

- [ ] 2.1 In `autocoder/src/polling_loop.rs` (OR the equivalent module), after the iteration's commit-count check passes (`commit_count > 0`) AND before the push + PR-creation step, invoke `list_iteration_pending_changes`. When the returned Vec is non-empty:
  - Log INFO `audit-only PR path suppressed: iteration-pending markers present for <comma-separated change list>`.
  - Return `Ok(())` WITHOUT calling `git::push_force_with_lease` OR `github::create_pull_request`.
- [ ] 2.2 When the returned Vec IS empty, proceed to the push + PR-creation steps exactly as today (no behavioral change for the audit-only-PR happy path).
- [ ] 2.3 Unit-test the suppression branch with a fixture workspace containing one `.iteration-pending.json` marker: assert push is NOT called, PR creation is NOT called, AND the INFO log line fires.
- [ ] 2.4 Unit-test the non-suppression branch with a fixture workspace containing NO `.iteration-pending.json` markers: assert push IS called AND PR creation IS called (regression test for the existing happy path).

## 3. Iteration-pending marker write — implementation backfill

- [ ] 3.1 Audit the `IterationRequested` arm of the polling-loop's outcome dispatcher (in `autocoder/src/polling_loop.rs` OR equivalent) AND verify it calls the marker-write helper AFTER the commit + push step. If the call is missing, ADD it per the canonical `a27a1` "Iteration-pending marker file in the change directory carries state across iteration boundaries" requirement.
- [ ] 3.2 Marker write SHALL use atomic tempfile + rename (matching `mcp_askuser_server::write_marker`).
- [ ] 3.3 Marker payload SHALL be `{"completed_tasks": [...], "remaining_tasks": [...], "reason": "...", "iteration_number": N}` per a27a1's spec.
- [ ] 3.4 Audit the `IterationRequested` arm's `.in-progress` cleanup. Per the canonical openspec-queue-engine "Unlocking after any executor outcome" requirement, `.in-progress` SHALL be removed after ALL outcome arms (including IterationRequested). If the IterationRequested arm doesn't drop the lock, ADD the call.

## 4. Audit-only PR body content-aware rendering

- [ ] 4.1 In the audit-only PR body composition code (likely in `autocoder/src/git_workflow_manager.rs` OR `polling_loop.rs`), partition the agent-branch commits-ahead-of-master into categories by commit-message prefix:
  - `audit: <type>` → audit-produced.
  - `iteration N of <change>` → iteration WIP (per a27a1's commit-message format).
  - `archive: <change>` OR similar → implementer-archived (existing format).
  - Anything else → manual / unknown.
- [ ] 4.2 PR body template enumerates commit categories AND only includes a section for each non-empty category. "Audit-produced proposals" framing is included ONLY when audit-produced commits are non-empty.
- [ ] 4.3 PR title formatting: when ALL commits are audit-produced, use today's `audit-only: <N> proposal(s) from <comma-separated-audit-types>` title (existing behavior). When commits are mixed OR when audit commits are absent, the title takes a generic shape like `agent-q changes: <N> commits across <categories>`.
- [ ] 4.4 Unit-test each category combination:
  - Three audit commits, zero implementer, zero iteration WIP → title uses `audit-only:`, body has "Audit-produced proposals" section only.
  - Two audit commits, one iteration WIP → title uses generic shape, body has both sections.
  - Zero audit commits, one iteration WIP → with iteration-pending markers present (which by the suppression rule above means this PR shouldn't open at all). The renderer should still produce a sensible body if invoked directly via test (defensive).

## 5. Integration tests

- [ ] 5.1 Add an integration test that drives a polling iteration end-to-end against a temp workspace + temp bare git repo:
  - Setup: workspace with a change directory carrying an `.in-progress` marker AND a stub executor returning `IterationRequested { ... }`.
  - Action: run the polling iteration's outcome dispatcher.
  - Assert: `<workspace>/openspec/changes/<change>/.iteration-pending.json` exists AND parses to the expected payload.
  - Assert: `<workspace>/openspec/changes/<change>/.in-progress` does NOT exist.
  - Assert: `git rev-list --count <base>..<agent>` returns 1 (the iteration_request WIP commit was pushed).
  - Assert: NO call to `github::create_pull_request` happened (audit-only-PR path suppressed by the iteration-pending marker).
- [ ] 5.2 Add a sibling integration test for the audit-only-PR happy path (regression):
  - Setup: workspace with no iteration-pending markers AND a stub audit returning `SpecsWritten` AND a stub executor that produces no implementer commits.
  - Action: run the polling iteration.
  - Assert: agent branch has the audit's commits.
  - Assert: `github::create_pull_request` IS called with the canonical audit-only title format.
- [ ] 5.3 Add an integration test for the mixed case (audit AND iteration_request commits BOTH present, iteration-pending marker present):
  - Setup: workspace with one iteration-pending marker AND a stub audit producing one commit AND iteration_request WIP commit on agent-q.
  - Action: run the iteration.
  - Assert: NO PR is opened (suppression rule).
  - Assert: the audit's commit remains on agent-q (not lost; will ship in the next iteration's PR once iteration-pending resolves).

## 6. Validation

- [ ] 6.1 `cargo test` passes.
- [ ] 6.2 `cargo clippy` produces no NEW warnings against the existing baseline.
- [ ] 6.3 `openspec validate a38-audit-only-pr-suppresses-on-iteration-pending --strict` passes.
