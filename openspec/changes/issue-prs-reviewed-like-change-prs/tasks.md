# Tasks

## 1. Surface the worked issue(s) to the pass

- [ ] 1.1 Have the issues lane report the slug(s) it worked this pass so the reviewer step can see them. `run_issues_lane` (`polling_loop/commits.rs`) currently returns nothing; `walk_issues` returns the archived slugs. Thread those slug(s) out of `run_pass_through_commits` alongside `processed` (e.g. a `processed_issues: Vec<String>` companion to the changes `processed` list), without conflating them with changes.

## 2. Run the reviewer on issue passes

- [ ] 2.1 In `run_reviewer_step` (`polling_loop/pass.rs`), change the skip guard so the reviewer runs when EITHER a change OR an issue was processed. The reviewer is skipped only when both are empty (genuinely audit-only / nothing) OR the existing `skip_spec_only_prs` condition holds. An issue-only pass (`processed` empty, `processed_issues` non-empty) MUST run the reviewer.
- [ ] 2.2 Confirm the reviewer's diff/changed-files come from the whole-branch context (they already do — `build_review_context` uses the three-dot branch diff), so the issue's commit is in the reviewed diff.

## 3. Load the issue brief into the review context

- [ ] 3.1 In `build_review_context` / `build_per_change_contexts` (`polling_loop/review_context.rs`), build a brief for each worked issue from its archive entry: read `issue.md` and `tasks.md` under `lanes::issues::archive_root(workspace)` (NOT `changes/archive`), and add them to the `ReviewContext`'s briefs as the issue's intent/acceptance context. Reading via the `archive_root` helper means this follows the canonical `issues/` location automatically (independent of the directory-relocation issue).
- [ ] 3.2 A mixed pass (change + issue) populates both the change brief(s) (from `changes/archive`) AND the issue brief(s) (from `issues/archive`). An issue-only pass populates only the issue brief(s).
- [ ] 3.3 If an issue's archive entry cannot be located (degraded), log a WARN and proceed with the diff + changed files (the reviewer still reviews the code) — never skip the review for a missing brief.

## 4. Tests

- [ ] 4.1 An issue-only pass invokes the reviewer exactly once (a completed invocation), and the verdict comes from that review — assert the reviewer ran, not that the step returned `None`/skipped.
- [ ] 4.2 The review context for an issue pass contains the issue's `issue.md` + `tasks.md` (sourced from `issues/archive`) as the brief. (Assert on the context's briefs, not on prompt prose.)
- [ ] 4.3 A pass with neither a change nor an issue still skips the reviewer (regression: audit-only passes are unaffected).
- [ ] 4.4 A mixed change+issue pass runs the reviewer with both briefs present.
