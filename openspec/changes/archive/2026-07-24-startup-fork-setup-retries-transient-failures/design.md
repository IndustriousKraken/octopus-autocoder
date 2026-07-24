## Context

`ensure_forks_exist_with` (`autocoder/src/cli/run.rs`) returns undifferentiated `ForkSetupFailure`s; every one becomes a process-lifetime skip plus one alert. The mid-iteration path already distinguishes transient from permanent (`classify_recovery_failure`: transient patterns like `Could not resolve host` and 5xx retry next iteration; permanent ones skip-for-lifetime; unknown defaults to transient), and the startup path carries `TODO(a14)` (`run.rs:2043`) pointing at exactly this asymmetry. Consequence today: a DNS blip or a slow fork population at boot costs an operator intervention (restart/reload) that the daemon grants itself for the same failure five minutes later mid-iteration. The 60-second reachability timeout is the sharpest case — GitHub populates large forks asynchronously, so the timeout is *expected* to be beatable one iteration later.

## Goals / Non-Goals

**Goals:**
- Transient startup fork failures self-heal on the polling cadence with zero operator action.
- Deterministic failures keep the loud, operator-owned skip they have today.

**Non-Goals:**
- New classification rules — the mid-iteration pattern set (and its default-to-transient stance) is reused as-is; one classification concept in the codebase.
- Retrying inside startup (blocking boot on backoff loops) — retries happen on the polling cadence after startup completes, keeping startup fast and non-blocking.
- Extending the same treatment to the dirty-workspace startup check (the other half of `TODO(a14)`) — separable, and its failure modes differ; can be its own change.

## Decisions

- **Retry on the polling cadence via a fork-pending task state, not a startup loop.** Startup stays single-pass and fast; the per-repo task owns its own recovery, mirroring how mid-iteration recovery already works. The fork-pending iteration body is "re-attempt setup; on failure WARN and sleep; on success continue as normal" — no other work, because nothing downstream (branch init, lanes, push) can function without the fork remote.
- **Reuse `classify_recovery_failure`.** Fork-setup causes are threaded through the same classifier (HTTP status and error-chain text patterns). The identity mismatch, underivable URL, and unroutable PAT are pre-classified permanent at their creation sites — they are semantic outcomes, not error-shaped strings.
- **Reachability timeout = transient.** The strongest motivation for the change: fork creation *succeeded*; only population lagged the 60s budget. The fork-pending re-attempt's initial probe (`fork_reachable`) succeeds as soon as GitHub finishes, typically on the first retry.
- **Throttled alert while pending, INFO on recovery.** One alert per throttle window tells the operator a repo is degraded without per-iteration spam; the WARN-per-attempt keeps the journal precise. Recovery logs INFO rather than alerting — un-degrading is the expected outcome, and the alert stream stays reserved for conditions needing eyes.
- **`reload` semantics unchanged.** A fork-pending task is in the live task map, so reload's reconciliation treats it as present (hot-swaps config normally); permanent-skipped repos remain absent and reload re-adds them, exactly as today.

## Risks / Trade-offs

- [A genuinely permanent failure mis-classified transient retries forever] → Bounded noise: one WARN per iteration plus a throttled alert; the same trade the mid-iteration classifier already accepted, and the alert makes the stuck-pending state visible. The operator can still fix and reload at any time.
- [Fork-pending retries hammer GitHub during an outage] → One `ls-remote` probe + at most one POST per polling interval per affected repo — far below any rate-limit concern.
- [A permanent failure at the POST that the status-pattern classifier calls transient (e.g. 403 rate-limit text vs 403 missing-scope)] → Inherited from the shared classifier; refining its patterns benefits both call sites and belongs there, not here.
