## 1. Remove the implementer-queue-based early return

- [ ] 1.1 In `autocoder/src/polling_loop.rs`, delete the `if processed.is_empty()` block (currently lines 702-708):
  ```rust
  // DELETE THIS BLOCK:
  if processed.is_empty() {
      let _ = AlertState::clear(workspace);
      return Ok(());
  }
  ```
- [ ] 1.2 Verify the downstream `commit_count == 0` check (currently at line 712-719) becomes the sole gate for the "no work to push" path. That check already clears `AlertState` AND returns `Ok(())`, so behaviour is preserved for the implementer-empty + audit-empty case.
- [ ] 1.3 Audit the call site to confirm no other code between lines 702 AND 712 reads `processed` in a way that would panic OR mis-handle the empty case. The `commit_count = git::rev_list_count(...)` call IS independent of `processed`. Good.

## 2. Skip the reviewer for audit-only iterations

- [ ] 2.1 The reviewer step at lines 732-771 takes `&processed` AND builds a `ReviewContext` against the implementer-touched files. When `processed.is_empty()` AND `commit_count > 0` (the audit-only case), there are no implementer-touched files; the reviewer would either error OR produce a meaningless review. Wrap the reviewer step:
  ```rust
  let (review_report, draft, reviewer_revision_concerns) = if processed.is_empty() {
      // Audit-only iteration: nothing for the reviewer to evaluate.
      // The audit's own validation pass already gated each proposal.
      (None, false, Vec::new())
  } else {
      // existing match block: reviewer.is_some() vs None
  };
  ```
- [ ] 2.2 Tests: a unit test exercising the audit-only path confirms the reviewer's `review()` method is NOT called (mock reviewer's call counter remains zero).

## 3. PR-body construction for audit-only iterations

- [ ] 3.1 Locate `open_pull_request` (or its body-building helper) AND branch on `processed.is_empty()`:
  - When NON-empty: existing behaviour (PR body lists processed changes, includes reviewer section if present, etc.).
  - When empty AND `commit_count > 0`: build a body that names the audit-produced proposals. Source: `git log <base_branch>..<agent_branch> --format=%s` returns the agent-branch commit subjects, which carry the canonical `audit: <type> proposals (N change(s))` shape. Render as:
    ```markdown
    This PR ships audit-produced proposals only — no implementer changes this iteration.

    Commits on the agent branch:

    - <commit-subject-1>
    - <commit-subject-2>
    - ...

    Each `audit: <type>` commit creates new `openspec/changes/<prefix>-*` directories that the next polling iteration will pick up via `list_pending` and route to the implementer.
    ```
- [ ] 3.2 The PR title for an audit-only iteration SHALL be `audit-only: <N> proposal(s) from <comma-separated-audit-types>` so reviewers immediately recognize the PR's shape.
- [ ] 3.3 Tests: a unit test asserts the title shape AND body content for the audit-only path.

## 4. Regression-prevention integration test

- [ ] 4.1 Add a test (in `autocoder/src/polling_loop.rs::tests`) named `audit_only_iteration_pushes_and_opens_pr`. Setup:
  - Fixture workspace + git init + initial commit on `base_branch`.
  - Empty `openspec/changes/` directory (no pending changes).
  - A mock audit (`MockAudit`) registered in the audit registry that returns `AuditOutcome::SpecsWritten { changes: vec!["secure-test-1".into()], retries_used: 0 }` AND writes a real `openspec/changes/secure-test-1/proposal.md` to the workspace before returning (so the post-hoc WritePolicy check passes).
  - The audit fixture also performs `git add` + `git commit` of the new directory inside its `run()` method (mirroring `specs_writing.rs`'s real flow).
- [ ] 4.2 Stub the git push at the `git::push_force_with_lease` boundary. Use the existing testing-mode hook (`AUTOCODER_TEST_FAKE_PUSH` env-var) OR (if it doesn't exist) introduce one. Capture: which branch was pushed, which remote.
- [ ] 4.3 Stub `github::create_pull_request` at the test boundary. Capture: head, base, title, body. The existing tests at lines 5436+ already use `create_pull_request_at_for_test`; the new test follows the same pattern.
- [ ] 4.4 Assertions:
  - The push stub WAS called.
  - The PR-creation stub WAS called.
  - The captured head matches the configured `agent_branch`.
  - The captured base matches `base_branch`.
  - The captured title matches the audit-only shape from task 3.2.
  - The captured body contains `audit: <type>` AS substring.
- [ ] 4.5 Pre-fix verification: temporarily revert task 1.1's deletion AND re-run the test. It SHALL fail (push stub never called → assertion fails). This verifies the test actually guards the bug.

## 5. Canonical requirement codifying the termination invariant

- [ ] 5.1 `openspec/changes/a20a3-audit-only-iterations-push-and-pr/specs/orchestrator-cli/spec.md` ADDs:
  `Polling iteration termination is gated on agent-branch commit count, not on implementer-queue outcome`. The body requires that any "no work to ship" early-return SHALL consult `git rev-list --count <base>..<agent>` (OR equivalent) AND SHALL NOT use higher-level signals (implementer-queue length, audit-queue length, etc.) as the sole gate.

## 6. Spec deltas

- [ ] 6.1 `openspec/changes/a20a3-audit-only-iterations-push-and-pr/specs/orchestrator-cli/spec.md` ADDs the termination-invariant requirement (per task 5.1).

## 7. Verification

- [ ] 7.1 `cargo test --bin autocoder` passes — new regression test + existing tests.
- [ ] 7.2 `openspec validate a20a3-audit-only-iterations-push-and-pr --strict` passes.
- [ ] 7.3 `cargo clippy --bin autocoder` produces no new warnings in `polling_loop.rs` AT the lines I added/modified.
- [ ] 7.4 Manual verification (post-deploy): on a daemon with the fix applied, run `@<bot> audit security_bug <repo>` against a repo with no pending changes. Expect: `🔍 created proposal …` notification(s), THEN `✅ PR opened: <url>` notification, THEN the PR exists on GitHub with the audit's commits.
