# A discarded or errored reviewer is visible in the PR, not silent

## Why

When the agentic reviewer (the default) produces no usable verdict — it
discards the review (no valid `submit_review` submission) or errors — the daemon
returns no review report, which renders NO `## Code Review` section in the PR at
all. The only signal is a chatops alert. So a reader of the PR cannot tell
"reviewed and approved" from "the reviewer never ran" — the failure is invisible
where it matters most. The one-shot reviewer path already surfaces its failure
as a visible report; when `agentic` became the default, that visible-failure
behavior silently regressed.

The code reviewer is a control-plane gatekeeper, and the gatekeepers-fail-closed
standard already requires an advisory gatekeeper to render an explicit
"failed to run" result rather than omit its output. The `[out]` gate already
does this (`## Spec Verification: FAILED TO RUN`); the reviewer does not. This
brings the reviewer into line.

## What Changes

- A new `orchestrator-cli` requirement: a discarded or errored agentic review
  renders an explicit `## Code Review: FAILED TO RUN` section in the PR body
  (parallel to the `[out]` gate), naming the cause — not an omitted section and
  not an approval. The reviewer stays advisory (never blocks); the chatops alert
  continues; the gate-verdict ledger records the reviewer as failed-to-run (not
  passed, not absent).
- This is an ADD (not a MODIFY of "Agentic reviewer mode") deliberately: the PR
  surfacing of a reviewer failure has no dedicated canon home today, and an ADD
  avoids colliding with the in-flight `on-demand-code-review` change that
  modifies "Agentic reviewer mode".

## Impact

- Affected specs: `orchestrator-cli` (ADD the reviewer-failed-to-run-visibility
  requirement).
- Affected code: `polling_loop/pass.rs` (`run_reviewer_step`) — the agentic
  reviewer's `Discarded` AND `Err` arms render a visible failed-to-run reviewer
  result instead of returning `None` (which omitted the section); the PR-body
  assembly + gate ledger reflect it. Reuse the rendering shape the `[out]` gate
  uses for its FAILED TO RUN section and the one-shot reviewer uses for its
  synthetic failure report.
- No change to a successful review, to the advisory posture, or to the chatops
  alert. Closes the reviewer-side fail-open that paired with the `[out]`-gate
  one.
