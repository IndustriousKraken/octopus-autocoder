# Design: audit-failure-backoff

## Where the tracking lives

In the existing per-workspace audit state (`.audit-state.json`), inside each audit type's entry, as two new fields: `consecutive_failures: u32` and `last_failed_attempt_at: <rfc3339>`. Both serde-default (absent = no failures) so existing state files load unchanged. The cadence fields (`last_run_at`, `last_run_sha`) are never written by a failed attempt — the distinction "cadence fields vs failure-tracking fields" is what reconciles this change with the long-standing "a failure does not advance cadence" posture.

## Backoff formula

`backoff = min(poll_interval * 2^(consecutive_failures - 1), min(cadence_interval, 24h))`

- First failure → next attempt eligible one poll interval later (one skipped iteration at most, cheap recovery for transient blips).
- Repeats double: 2×, 4×, 8× the poll interval…, so a deterministic failure converges to at most one billed session per cadence interval (or per 24h for long-cadence audits) instead of one per iteration.
- The cap keeps the audit from backing off past its own cadence — a weekly audit never waits longer than a week (nor longer than 24h, whichever is smaller), preserving eventual retry.

## What counts as a failed attempt

Exactly the three outcomes that today leave cadence untouched and re-fire next iteration: `run()` returning `Err`, a write-policy violation, and `DidNotComplete`. `WorkspaceUnavailable` stays fully outside (its no-state-update semantics are load-bearing for the workspace-self-heal flow). `ValidationExhausted` already advances cadence today and is unaffected.

## Operator override

Queued on-demand runs (`audit` verb, CLI `audit run`) bypass the backoff check — an explicit operator retry is always allowed and is the recovery path after fixing the underlying cause. The queued run's terminal outcome updates the tracking normally (success clears it; failure re-arms it).

## Observability

- The existing audit-failure alert text gains `failure N in a row; next automatic re-attempt after <time>`.
- Each backoff-suppressed iteration logs one INFO line naming the audit, the count, and the remaining wait — so `journalctl` shows exactly why an audit is quiet.
