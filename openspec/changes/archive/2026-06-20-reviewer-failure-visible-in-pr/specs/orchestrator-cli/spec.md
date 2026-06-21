## ADDED Requirements

### Requirement: A discarded or errored reviewer renders a visible FAILED TO RUN section in the PR
The code reviewer is a control-plane gatekeeper, so per the gatekeepers-fail-closed standard its inability to run SHALL be a visible, non-passing state — never silence. When the agentic reviewer produces no usable verdict — it DISCARDS the review (a session that records no schema-valid `submit_review` submission) OR it errors (spawn/transport failure) — the PR body SHALL carry an explicit `## Code Review: FAILED TO RUN` section naming the cause, parallel to the `[out]` gate's `## Spec Verification: FAILED TO RUN`. The daemon SHALL NOT omit the reviewer section in this case, AND SHALL NOT represent the failure as an approval.

This is the reviewer-side counterpart of the same fix the `[out]` gate already has: the prior behavior — returning no review report, which rendered NO `## Code Review` section at all — left a failed/discarded review invisible in the PR (only a chatops alert was posted), so an operator reading the PR could not distinguish "reviewed and approved" from "the reviewer never ran." The one-shot reviewer path already surfaces its failure as a visible report; this brings the agentic path (the default) into line.

The reviewer remains ADVISORY: a FAILED TO RUN reviewer state SHALL NOT block PR creation AND SHALL NOT be a `Block`/`Approve` verdict — it is a distinct could-not-run state. The existing operator-visible chatops alert SHALL continue. The PR's gate-verdict ledger SHALL record the reviewer as failed-to-run (NOT passed/approved AND not absent), so the reviewer's could-not-run state is legible there too.

#### Scenario: A discarded agentic review renders FAILED TO RUN, not silence
- **WHEN** the agentic reviewer session records no schema-valid `submit_review` submission (the review is discarded)
- **THEN** the PR body carries an explicit `## Code Review: FAILED TO RUN` section naming the cause
- **AND** the reviewer section is NOT omitted AND the failure is NOT rendered as an approval
- **AND** the existing reviewer-failure chatops alert is still posted
- **AND** PR creation still proceeds — the reviewer never blocks

#### Scenario: An errored agentic review renders FAILED TO RUN
- **WHEN** the agentic reviewer errors (spawn/transport failure) before producing a verdict
- **THEN** the PR body carries the `## Code Review: FAILED TO RUN` section naming the cause
- **AND** it is not represented as an approval AND PR creation proceeds

#### Scenario: The gate ledger records the reviewer as failed-to-run
- **WHEN** the reviewer is discarded OR errors
- **THEN** the PR's gate-verdict ledger records the reviewer verdict as failed-to-run
- **AND** it is NOT recorded as passed/approved NOR left absent (so "could not run" is distinguishable from "ran and approved")

#### Scenario: A successful review is unchanged
- **WHEN** the agentic reviewer returns a valid verdict (Approve OR Block)
- **THEN** the PR body renders the normal `## Code Review` section AND the ledger records that verdict, exactly as before
