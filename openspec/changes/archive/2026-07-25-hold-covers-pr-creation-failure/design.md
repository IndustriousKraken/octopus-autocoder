# Design: hold-covers-pr-creation-failure

## Reuse the existing marker, don't invent a second hold

The push-block marker already records everything the resume path needs (tip commit, carried slugs, reason, and the rendered PR-body sections). Two additive fields cover both gaps:

- `failed_step`: `push` | `pr_creation`, serde-default `push` so existing markers deserialize unchanged.
- `issue_slugs`: `Vec<String>`, serde-default empty.

## Resume path stays almost untouched

The existing resume flow is: tip match → skip branch recreation and executor → retry push → on success, remove marker and open the PR. For a `pr_creation` hold the tip is already on the remote, so the push retry is a cheap no-op (`--force-with-lease` of an identical tip), and the flow proceeds straight to PR creation — which is exactly the retry we want. So the resume logic needs no branching on `failed_step`; the field exists for the alert text and operator diagnostics, not for control flow. The one behavioral addition on the resume path: PR-body derivation must account for issue slugs.

## Alert categories unchanged

Push failure keeps `BranchPushFailure`; PR-creation failure keeps `PrCreationFailure` (both already exist and are 24h-throttled). The only text change: each alert states that the completed work is preserved and which step will be retried.

## Failure-to-resolve-tip edge

As with the existing push-failure arm, if the agent-branch tip cannot be resolved when writing the marker, the daemon logs a WARN and proceeds without a hold (the work remains on the branch; the next pass recreates it). This is the current degraded behavior, unchanged — the hold is best-effort protection, and manufacturing a tip would corrupt the stale-marker check.
