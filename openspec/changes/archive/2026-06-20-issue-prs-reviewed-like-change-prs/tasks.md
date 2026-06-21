# Tasks

The reviewer-over-issue-passes behavior is already shipped in production code.
Tasks 1–3 are a verification checklist against that shipped code (cite the
file:line refs and confirm — do NOT re-implement; churning these no-ops the code
and trips the `[out]`/stub gates). Task 4 (tests) is the remaining work that this
change adds.

## 1. Surface the worked issue(s) to the pass

- [x] 1.1 The issues lane reports the slug(s) it worked this pass alongside the
  changes `processed` list, without conflating them. `run_issues_lane` returns
  the archived issue slugs (`autocoder/src/polling_loop/commits.rs:216-225`,
  `run_issues_lane` signature at `commits.rs:216`, returning `walk_issues`'s
  archived slugs at `commits.rs:287-306`/`299-309`). `run_pass_through_commits`
  threads them out as a `processed_issues: Vec<String>` companion to `processed`
  (`commits.rs:9-22` signature returns `(Vec<String>, Vec<String>, bool)`;
  `processed_issues` captured at `commits.rs:32` and returned at `commits.rs:203`).

## 2. Run the reviewer on issue passes

- [x] 2.1 The reviewer-step skip guard runs the reviewer when EITHER a change OR
  an issue was processed; it skips only when both are empty (or the existing
  `skip_spec_only_prs` condition holds). The guard is
  `let no_reviewable_work = processed.is_empty() && processed_issues.is_empty();`
  at `autocoder/src/polling_loop/pass.rs:426`. `run_reviewer_step` takes
  `processed_issues: &[String]` (`pass.rs:376-385`) and is wired from
  `execute_one_pass` with the lane's slugs (`pass.rs:108-118`).
- [x] 2.2 The reviewer's diff/changed-files come from the whole-branch
  three-dot context, so the issue's commit is in the reviewed diff:
  `build_review_context` derives the diff + file list via
  `git::diff_three_dot` / `git::diff_files_changed`
  (`autocoder/src/polling_loop/review_context.rs:15-16`).

## 3. Load the issue brief into the review context

- [x] 3.1 `build_review_context` builds a brief for each worked issue from its
  archive entry — reading `issue.md` and `tasks.md` under
  `crate::lanes::issues::archive_root(workspace)` (NOT `changes/archive`) and
  pushing them onto the context's briefs
  (`autocoder/src/polling_loop/review_context.rs:85-111`). Reading via
  `archive_root` follows the canonical `issues/` location automatically.
- [x] 3.2 A mixed pass populates both the change brief(s) (from `changes/archive`,
  `review_context.rs:61-83`) AND the issue brief(s) (from `issues/archive`,
  `review_context.rs:90-111`); an issue-only pass populates only the issue
  brief(s) (the change loop iterates an empty `processed`).
- [x] 3.3 A missing issue archive entry logs a WARN and proceeds with the diff +
  changed files (never skips the review for a missing brief):
  `review_context.rs:91-102` (`locate_archive_dir` → `None` → `tracing::warn!` +
  `continue`).

## 4. Tests

- [x] 4.1 An issue-only pass invokes the reviewer exactly once (a completed
  invocation), asserted via a counting reviewer client driven through the real
  `run_reviewer_step` skip-guard — asserting the reviewer RAN (call count == 1,
  report present), not that the step returned `None`/skipped.
  (`issue_only_pass_invokes_reviewer_once`, `polling_loop/tests/t04.rs`.)
- [x] 4.2 The review context for an issue pass contains the issue's `issue.md`
  (brief `proposal`) + `tasks.md` (brief `tasks`), sourced from `issues/archive`.
  Asserts on the returned `ReviewContext.archived_changes` data, not on prompt
  prose. (`issue_pass_review_context_carries_issue_brief`,
  `polling_loop/tests/t04.rs`.)
- [x] 4.3 A pass with neither a change nor an issue still skips the reviewer
  (regression: counting client never invoked, no report).
  (`no_change_no_issue_pass_skips_reviewer`, `polling_loop/tests/t04.rs`.)
- [x] 4.4 A mixed change+issue pass populates BOTH briefs in
  `ReviewContext.archived_changes` (change brief from `changes/archive`, issue
  brief from `issues/archive`). (`mixed_change_and_issue_pass_carries_both_briefs`,
  `polling_loop/tests/t04.rs`.)
</content>
</invoke>
