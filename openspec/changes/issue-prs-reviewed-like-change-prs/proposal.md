# Issue-lane PRs are code-reviewed like change PRs

## Why

When the issues lane works an issue, its fix-plus-archive commit rides the pass's
push and PR — but the code reviewer never runs over it. The reviewer step skips
entirely whenever the pass processed no *change*: the worked issue is not tracked
as a processed unit, so an issue-only pass looks "audit-only" (no implementer
work) and the reviewer is skipped. The result is a PR carrying a real code change
that ships with no automatic review. The operator can trigger `@<bot> code-review`
by hand, but unsupervised review — the default for every change PR — does not
happen for issues.

This contradicts the principle the reviewer already holds elsewhere: a PR whose
context carries a non-empty diff must reach the reviewer "rather than skipping
the call," and no verdict may be produced without a completed review (the
canonical *Per-change review falls back to bundled when the change set is empty*
requirement). An issue carries real code changes, so its PR should be reviewed
like any change PR. The issues lane (a009) was added without wiring this, leaving
a gap in canon and a skip in the loop.

Separately, even when the reviewer does run on a pass that also processed a
change, it has no brief for the issue: the review context is built only from
`changes/archive` entries, so the reviewer sees the issue's diff but not its
intent or acceptance criteria.

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
