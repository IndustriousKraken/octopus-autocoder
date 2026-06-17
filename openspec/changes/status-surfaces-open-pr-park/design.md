# Design

## D1 — Park is orthogonal to the busy marker

The `currently:` line branches on the busy marker (rules 1-7). The open-PR park is
a state the daemon enters BEFORE acquiring a marker — the skip gate returns from the
iteration without `recreate_branch` or any executor, so no marker is ever stamped.
The park is therefore invisible to the current marker-only branching, which collapses
it to rule 1's `idle`. The fix refines rule 1: when no marker is present, the line
distinguishes "parked on an open PR" from "truly idle."

## D2 — Reuse the gate's own query

The park determination uses the SAME signal the gate uses — `list_open_prs_for_head`
(open PRs whose head is the agent branch). This guarantees the status reply's park
claim matches the gate's actual behavior: if the gate would skip, the status says
parked; if not, it says idle. The status path already queries the agent-branch PR for
the `latest PR:` line, so the additional information is cheap.

## D3 — Fail to `idle`, never fabricate a park

Consistent with the existing "GitHub failure does not break the reply" scenario, a
failed open-PR query degrades: the `currently:` line falls back to `idle` (the
marker-only determination) and the rest of the reply renders normally. The status
never reports a park it could not confirm. (This mirrors the gate's own fail-open
posture is NOT copied — the gate fails closed and skips on a query error; the STATUS
reply, being read-only diagnostic, simply omits the park annotation rather than
guess. The two are independent: the gate decides whether to work; status only
describes.)

## D4 — Wording is behavior, not a pinned string

The park line names the blocking PR number and states that no work runs until it is
merged or closed; the exact phrasing is not asserted by a test (per `Tests assert
behavior or derivation, never message wording`). Tests assert the branch is taken
(park vs idle) given the open-PR state and the absence of a marker.

## D5 — Multiple open PRs

`list_open_prs_for_head` can return more than one PR. The line names the gating
PR(s); naming the lowest-numbered open PR (with a `(+N more)` suffix when several
exist) is sufficient and matches how the gate logs them. This is a rendering detail,
not a contract.
