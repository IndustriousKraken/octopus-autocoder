# Tasks: audit-failure-backoff

## 1. Failure tracking

- [ ] 1.1 In `autocoder/src/audits/state.rs`, add `consecutive_failures: u32` and `last_failed_attempt_at: Option<DateTime<Utc>>` to the per-audit state entry, both serde-defaulted so existing state files load unchanged.
- [ ] 1.2 In `autocoder/src/audits/scheduler.rs`, record a failed attempt (increment count, stamp timestamp, persist) on each of the three failure outcomes: `run()` `Err`, write-policy violation, and `DidNotComplete`. Leave the cadence fields (`last_run_at`, `last_run_sha`) untouched on these paths, as today.
- [ ] 1.3 Clear the failure-tracking fields on any successful terminal outcome. Leave them untouched on `WorkspaceUnavailable`.

## 2. Backoff gate in the scheduler

- [ ] 2.1 In the cadence-driven scheduling check, before spawning a due audit whose `consecutive_failures > 0`, compute `backoff = min(poll_interval * 2^(consecutive_failures - 1), min(cadence_interval, 24h))` and skip the audit when `now - last_failed_attempt_at < backoff`, logging one INFO line with the audit type, the count, and the remaining wait.
- [ ] 2.2 Queued on-demand runs bypass the backoff check entirely; their terminal outcome still updates the tracking per 1.2/1.3.
- [ ] 2.3 Extend the audit-failure alert text with the consecutive-failure count and the next-eligible re-attempt time.

## 3. Tests

- [ ] 3.1 Test: an audit whose attempt failed is NOT re-attempted on the immediately following iteration; it IS re-attempted once the backoff elapses.
- [ ] 3.2 Test: the backoff doubles per consecutive failure and never exceeds the smaller of the cadence interval and 24 hours.
- [ ] 3.3 Test: a successful run clears the tracking; a queued on-demand run executes despite an open backoff.
- [ ] 3.4 Test: `WorkspaceUnavailable` neither records a failed attempt nor arms a backoff.

## 4. Validation

- [ ] 4.1 Run `openspec validate audit-failure-backoff --strict` and fix any findings.
