# The spec-revision executor retains its marker instead of clearing it at PR-open

## Why

When `@<bot> send it` runs the spec-revision executor (a03), a clean re-gate
opens a revision PR AND the daemon then clears the change's
`.needs-spec-revision.json` marker
(`autocoder/src/polling/revision_session.rs:1499`), on the reasoning that "the
open PR parks the queue; the marker's blocking role is redundant." That
reasoning is imported verbatim from the `@<bot> revise` path
(`autocoder/src/revisions/process_pr.rs`), where it is TRUE — that revision is
applied to an open PR on the **agent branch**, which the polling loop's open-PR
short-circuit queries (canon: "Skip iteration when an open PR exists for the
agent branch", implemented by `open_pr_exists_for_agent_branch`), so the open PR
genuinely parks the whole repo and the marker's hold is redundant.

But the spec-revision executor opens its PR on a **dedicated per-change branch**,
`revision_branch_name(&repo.agent_branch, change_slug)` →
`<agent-branch>-spec-revision-<change-slug>`
(`revision_session.rs:415`, `:1174`, `:1466`). The open-PR short-circuit only
queries `repo.agent_branch` — it never sees the revision branch. So that PR does
**not** park the loop. With the marker cleared, the change immediately re-enters
`list_pending`, and the very next polling iteration's queue walk re-runs the
`[in]` / `[canon]` contradiction gate against the change's still-unmerged
(base-branch) contradictory deltas — burning gate tokens AND re-writing the
exact marker it just cleared. The marker is cleared only to be immediately
re-added, before the revision PR can be reviewed, exactly as reported.

**The canon is correct; the code drifted.** Three canonical requirements bear on
this and each is right as written:

- "Skip iteration when an open PR exists for the agent branch" deliberately
  scopes to the agent branch — broadening it to park on any per-change branch
  would over-block (it would freeze every other pending change and audit
  whenever one change has an open revision PR).
- "A successfully applied revision clears the change's needs-spec-revision
  marker" is correct AND correctly scoped to the `@<bot> revise` / agent-branch
  path, where the park premise holds.
- "Send it in a revision thread runs the spec-revision executor" is **silent**
  on the marker — it never authorized an auto-clear.

So no canonical requirement designed this behavior: the auto-clear in the
spec-revision executor is undocumented code that borrowed a justification which
does not hold in its context. The marker is already the correct-granularity
hold (it holds only the one flagged change, letting other work proceed); the
defect is clearing it too early, at PR-open, instead of leaving it until the
revision actually lands on base. The fix is to stop clearing it there and make
the retention an explicit, documented invariant so it cannot regress.

## What Changes

- The spec-revision executor SHALL NOT clear the `.needs-spec-revision.json`
  marker when a clean re-gate opens the revision PR. The marker remains, holding
  the change out of `list_pending`, until the revision lands on base AND an
  operator clears it (via `@<bot> clear-revision`) or the standard flow
  re-evaluates the now-revised change.
- This makes the change's held state stable while its revision PR awaits review:
  no wasted `[in]` / `[canon]` gate runs, no marker thrash, no spurious revision
  re-alert.
- The distinction from the `@<bot> revise` auto-clear is captured in canon so a
  future change does not re-introduce the premature clear by analogy.

## Impact

- Affected capability: `orchestrator-cli` (the spec-revision executor flow).
- Affected code: `autocoder/src/polling/revision_session.rs` — the clean-re-gate
  success path in `run_revision_execute` (remove the
  `remove_revision_marker_idempotent` block at ~`:1499–:1515`); the success
  thread reply (~`:1546`) gains a one-line note that the operator clears the
  marker after merging; and the marker-clear test assertion
  (`clean_regate_resets_consecutive_failure_count`, `:2887–:2888`) is inverted to
  assert retention.
- Operator-facing behavior after this change: once a revision PR is open, the
  operator reviews AND merges it, then runs `@<bot> clear-revision <repo>
  <change>` to release the change; it then re-gates cleanly against the merged
  base deltas AND proceeds to the implementer. This is the same operator-clear
  path the marker already documents; the only difference from today is that the
  marker is no longer (wrongly) auto-cleared and re-added while the PR is open.
- Deliberately **not** in scope (YAGNI): auto-clearing the marker when the daemon
  detects the revision PR was *merged* (deltas now on base) would remove the
  residual `clear-revision` step, but it requires merge-detection the daemon does
  not have here. If that toil proves worth removing, it is a separate,
  additive change; the minimal correct fix is to stop clearing prematurely.
