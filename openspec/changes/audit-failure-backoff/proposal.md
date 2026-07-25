# audit-failure-backoff

## Why

A deterministically failing audit currently re-bills a full agentic session on every polling iteration, essentially forever. Three failure outcomes deliberately leave the audit's cadence state untouched — a `run()` error, a write-policy violation, and `DidNotComplete` — so the next iteration's cadence check sees the audit as still-due and spawns another billed session. At a 300-second poll interval that is up to ~288 billed sessions per day per broken audit per repository, while the corresponding chatops alert is throttled to one per 24 hours. The burn is nearly silent: the operator sees one alert a day and a rising bill. (The OpenCode permission-prompt incident produced exactly this failure shape: every session ended with no verdict written.)

The retry-every-iteration behavior is currently mandated by the specs ("state is NOT updated", "the next iteration retries"), so this is a spec change, not a bug fix: failed attempts should be re-attempted on a backoff, not on every iteration, while keeping the fail-closed reporting posture intact.

## What Changes

- Failed audit attempts (run error, write-policy violation, `DidNotComplete`) are recorded in failure-tracking fields distinct from the cadence fields — a failure still never advances cadence.
- The scheduler does not re-attempt a failing audit until a backoff elapses: starting at the poll interval, doubling per consecutive failure, capped at the smaller of the audit's cadence interval and 24 hours.
- A successful attempt clears the tracking; `WorkspaceUnavailable` skips remain outside it entirely; operator-queued on-demand runs bypass the backoff.
- The audit-failure alert names the consecutive-failure count and when the next re-attempt is eligible.

## Impact

- Affected specs: `orchestrator-cli` (Periodic audit framework; Audit runs fail closed to a non-passing did-not-complete outcome; new backoff requirement)
- Affected code: `autocoder/src/audits/scheduler.rs`, `autocoder/src/audits/state.rs`
