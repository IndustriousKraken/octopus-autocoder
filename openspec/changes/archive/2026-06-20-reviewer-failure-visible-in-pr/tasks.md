# Tasks

## 1. Render a visible FAILED TO RUN reviewer section

- [x] 1.1 In `polling_loop/pass.rs` `run_reviewer_step`, change the agentic reviewer's `Discarded { reason }` AND `Err` arms: instead of returning `(None, false, Vec::new())` (which omits the `## Code Review` section), produce a visible failed-to-run reviewer result that renders a `## Code Review: FAILED TO RUN — <cause>` section in the PR body. Reuse the rendering shape the `[out]` gate uses (`render_spec_verification_failed_section`) and/or the one-shot path's synthetic-report approach — pick the one that flows cleanly through the existing PR-body assembly. The verdict is a distinct could-not-run state, NOT `Approve` and NOT `Block`; it does not open a revision and does not block PR creation.
- [x] 1.2 Keep the existing reviewer-failure chatops alert (`post_reviewer_discarded_alert`) on both arms.

## 2. Ledger reflects failed-to-run

- [x] 2.1 Ensure the PR's gate-verdict ledger records the reviewer as failed-to-run on discard/error (not passed/approved, not absent), so it is distinguishable from a ran-and-approved reviewer. Mirror how the `[out]` gate records `FailedToRun` into the ledger.

## 3. Tests

- [x] 3.1 A discarded agentic review (runner records no valid submission) produces a PR body containing `## Code Review: FAILED TO RUN`, NOT an omitted section and NOT an approval; PR creation still proceeds.
- [x] 3.2 An errored agentic review renders the same FAILED TO RUN section.
- [x] 3.3 The gate ledger records the reviewer as failed-to-run on discard/error.
- [x] 3.4 A successful agentic review (valid Approve/Block verdict) renders the normal `## Code Review` section unchanged.
