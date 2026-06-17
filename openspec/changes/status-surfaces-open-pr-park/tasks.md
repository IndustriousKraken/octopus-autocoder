# Tasks

## 1. Park detection in the status path

- [ ] 1.1 In the status reply composer (`chatops/operator_commands.rs`), query the agent-branch open PRs via the existing `list_open_prs_for_head` helper (the same call the skip-iteration gate uses). Reuse the result already fetched for the `latest PR:` line where practical to avoid a second round-trip.
- [ ] 1.2 On a GitHub error for that query, treat it as "park unknown": the `currently:` line falls back to the marker-based determination (`idle` when no marker) and a WARN is logged. Never fabricate a park.

## 2. `currently:` line park branch

- [ ] 2.1 In the `currently:` line computation, refine rule 1 (no marker present): when an open agent-branch PR exists, render `parked: open PR #<n> awaiting review — no new work until it is merged or closed` (lowest-numbered open PR, with a `(+N more)` suffix when several); otherwise render `idle`. Rules 2-7 (marker-present cases) are unchanged.
- [ ] 2.2 Preserve the existing invariant that the line never reads `idle` when a busy marker exists; extend it so it also never reads `idle` when an open agent-branch PR exists.

## 3. Docs

- [ ] 3.1 Update `docs/CHATOPS.md`'s `status` reply-shape examples to include the `parked: open PR #<n> ...` variant, and update the diagnostic-value paragraph to distinguish "parked on an open PR (merge or close to resume)" from "truly idle".

## 4. Tests

- [ ] 4.1 With no busy marker AND an open agent-branch PR present, the `currently:` line renders the park variant naming the PR, not `idle` (assert the branch taken / the PR number is present, not the exact phrasing).
- [ ] 4.2 With no busy marker AND no open agent-branch PR, the line renders `idle`.
- [ ] 4.3 With a busy marker present, the marker-based determination wins regardless of open-PR state (an open PR does not override `working on <change>` etc.).
- [ ] 4.4 A failed open-PR query degrades to the marker-based determination (`idle` when no marker) and does not break the rest of the reply.
