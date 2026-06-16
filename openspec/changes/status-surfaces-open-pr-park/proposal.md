# Status reply surfaces the open-PR park as the idle reason

## Why

When an open PR exists for the agent branch, autocoder skips every iteration for
that repo entirely (per `Skip iteration when an open PR exists for the agent
branch`) until the PR is merged or closed. The skip is correct, but it is invisible
in chat: it logs an INFO line to the daemon log only, and the `status` reply shows
`currently: idle` with no reason. An operator who believes they already merged the
PR sees a repo sitting idle for hours with pending changes and no explanation — the
`latest PR:` line shows the PR but reads as informational (it appears whether the
PR is open or merged), so it does not signal "this is why nothing is happening."

The `status` reply is the operator's one-shot diagnostic; it should name the gate
that is holding the repo, not just report `idle`.

## What Changes

- The `currently:` line gains a park variant. When no busy marker is present AND an
  open PR exists for the agent branch (the skip-iteration gate is active), the line
  reads `parked: open PR #<n> awaiting review — no new work until it is merged or
  closed` instead of `idle`. With no marker and no open agent-branch PR, it still
  reads `idle`.
- The status path performs the same agent-branch open-PR query the skip gate uses
  (`list_open_prs_for_head`); on a GitHub failure it degrades to `idle` (it never
  fabricates a park) and the rest of the reply still renders.
- `docs/CHATOPS.md`'s status reply-shape documentation enumerates the new variant.

## Impact

- Affected specs: `chatops-manager` (`Status reply always shows live workspace
  snapshot` — the `currently:` branching) and `project-documentation` (`CHATOPS.md
  status reply documentation enumerates the new currently: line variants`).
- Affected code: the status reply composer (`chatops/operator_commands.rs`) — add
  the agent-branch open-PR check (reuse `list_open_prs_for_head`) and the park
  branch in the `currently:` line computation.
- Independent of the other in-flight changes; touches no requirement they modify.
