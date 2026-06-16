# Interactive spec-revision thread

## Why

When the `[in]` or `[canon]` gate finds a contradiction at implement time, it
writes a `.needs-spec-revision.json` marker and posts a `SpecNeedsRevision` alert.
Today the operator's only path is to read the marker, hand-rewrite the spec,
push, and clear the marker — starting from scratch with no agent that has the
change, the canon, and the contradiction in context. For a change that is not
trivially wrong, the right move is often a judgment call ("is canon wrong here, or
should the change align to canon's existing term?") that the operator wants to
talk through before anything is rewritten.

This change makes the `SpecNeedsRevision` alert (for a contradiction marker) an
interactive thread: the operator discusses the revision with an agent that has the
full context, and then triggers the revision with `send it`. The agent drafts the
revision, re-runs the gates to confirm it is now consistent, and opens a PR. The
operator directs the approach and reviews the PR — so the agent drafts under human
direction and human review, never autonomously. This is the human-assisted
companion to `a02`, which self-heals the audit-authored cases before they ever
reach a marker; `a03` serves the residue: hand-written changes, and audit changes
where the authoring-time self-heal was disabled or did not apply.

## What Changes

- A contradiction `.needs-spec-revision.json` marker (one with empty
  `unimplementable_tasks` — a `[in]`/`[canon]` semantic finding, NOT the executor's
  unimplementable-tasks marker) is tracked as a revision thread: the alert's
  `channel`/`thread_ts` are captured in a `RevisionThreadState` keyed to the change,
  AND the alert advertises that the operator can reply to discuss OR `send it` to
  have the change revised and a PR opened.
- A non-`send it` `@<bot>` reply in a revision thread routes to a read-only
  revision advisor: an agentic session reconstructed from the change's spec
  deltas, the relevant canon, the marker's contradiction, AND the thread transcript
  so far. It answers the operator's questions (align-to-canon vs MODIFY-canon, and
  how) without writing anything.
- `@<bot> send it` in a revision thread becomes the fourth recognized `send it`
  context (after audit, brownfield-survey, and issue-candidate). It runs a revision
  executor that edits the change's spec deltas along the discussed direction,
  re-runs the `[in]` and `[canon]` gates to confirm the revision is consistent, AND
  opens a PR with the spec revision — reporting the PR back in the thread. If the
  re-gate still finds a contradiction, no PR is opened and the thread is told.
- The executor's unimplementable-tasks marker flow is unchanged: that invariant
  ("the agent flags; the operator authors the tasks.md edit; no AI process modifies
  its marching orders without human review") is preserved. `a03` only adds an
  operator-directed, PR-reviewed path for the distinct contradiction markers, which
  keeps human review in the loop (discussion + `send it` + PR merge).

## Stacked context

This is `a03`, stacking on `a02-audit-output-gate-checked` (it reuses a02's
`[in]`/`[canon]` check invocation for the revision executor's re-gate) AND on
`wire-issue-candidate-promotion` (it extends that change's three-context `send it`
dispatch to a fourth context). It does not change the gates themselves or the
unimplementable-tasks marker flow.

## Impact

- Affected specs: `chatops-manager` (the fourth `send it` context, the advisor
  routing, the help text); `orchestrator-cli` (the tracked revision thread +
  advertise, the revision advisor session, the revision executor + re-gate + PR,
  the audit-thread refusal text).
- Affected code: the `SpecNeedsRevision` alert post (capture channel/thread_ts for
  contradiction markers), a `RevisionThreadState` store, the `send it` dispatcher
  (fourth context), the revision advisor + revision-executor control-socket actions.
- Reused: the `[in]`/`[canon]` checks (via a02), the PR-open helpers, the
  `clear-revision` verb (unchanged manual escape).
