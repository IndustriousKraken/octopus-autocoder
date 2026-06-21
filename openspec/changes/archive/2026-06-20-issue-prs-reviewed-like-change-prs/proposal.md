# Issue-lane PRs are code-reviewed like change PRs

## Why

The reviewer-over-issue-passes behavior is already implemented in the daemon:
the issues lane surfaces its worked slug(s) to the pass, the reviewer-step skip
guard runs the reviewer whenever EITHER a change OR an issue was processed, and
the review context loads each worked issue's `issue.md` + `tasks.md` brief from
`issues/archive`. This change folds that shipped behavior into canon and locks it
with tests; canon does not yet carry the requirement, so the behavior is governed
only by the code.

The behavior matters because, without it, a pass that works only an issue would
ship a PR carrying a real code change with no automatic review: the reviewer step
would skip whenever the pass processed no *change*, treating an issue-only pass as
"audit-only." That contradicts the principle the reviewer already holds elsewhere
— a PR whose context carries a non-empty diff reaches the reviewer "rather than
skipping the call," and no verdict is produced without a completed review (the
canonical *Per-change review falls back to bundled when the change set is empty*
requirement) — and the gatekeepers-fail-closed posture (code does not ship
unreviewed by default). An issue carries real code changes, so its PR is reviewed
like any change PR.

For the same reason the reviewer needs the issue's intent: on a pass that
processes an issue, the review context must include the issue's `issue.md` and
`tasks.md` (its report + acceptance criteria and its fix steps) as the brief the
reviewer would otherwise build from a change's archive entry — not only the diff.
When a pass processes both a change AND an issue, both briefs are present.

## What Changes

- A pass that produces issue-lane commits SHALL run the code reviewer over them,
  exactly as a pass that processed a change does. An issue pass SHALL NOT be
  treated as an audit-only pass — it carries real code, so it gets a real review.
- The reviewer is still skipped on a pass that processed neither a change nor an
  issue (genuinely audit-only, or nothing) — that behavior is preserved.
- For an issue pass, the reviewer's brief (the role the archived-change brief
  plays for a change) SHALL be the worked issue's `issue.md` and `tasks.md` from
  its archive entry, so the reviewer has the issue's intent and acceptance
  criteria as context.
- All other reviewer behavior — transport (oneshot/agentic), verdict, concerns,
  `submit_review`, `reviewer.mode` dispatch, the caps, and the fail-closed
  no-submission handling — is unchanged. This adds the issue case; it does not
  redefine the change case.

## Impact

- Affected specs: `code-reviewer` (ADD **Issue-lane passes are code-reviewed**).
- Affected code: `polling_loop/pass.rs` (the reviewer-step skip decision must
  account for a processed issue, not just a processed change — thread the worked
  issue slug(s) into the decision), `polling_loop/commits.rs`/`lanes/walker.rs`
  (surface the worked-issue slug(s) to the pass so the reviewer step sees them),
  and `polling_loop/review_context.rs` (build the issue's brief from the issue's
  archive entry via `lanes::issues::archive_root`, so it follows the canonical
  `issues/` location automatically).
- Independent of the issues-directory relocation (it reads through the lane's
  archive helper) and of the agentic diff-on-demand change (it touches the
  reviewer-step skip decision and context assembly, not the agentic prompt
  rendering).
