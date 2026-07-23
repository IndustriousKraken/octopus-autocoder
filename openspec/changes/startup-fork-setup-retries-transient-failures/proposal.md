## Why

A fork-setup failure at startup skips the repository for the process lifetime, whatever the cause. That posture is right for deterministic failures (a renamed fork, an unroutable PAT) but wrong for transient ones: a DNS blip, a GitHub 5xx, or a fork that takes longer than 60 seconds to populate each permanently sideline a repository until an operator notices the alert and restarts or reloads. The daemon already solves this exact problem mid-iteration — recovery failures classify transient vs. permanent, and transient retries on the next iteration — and the startup path carries a code TODO pointing at that precedent. Startup should stop being the one place where a network blip costs a human intervention.

## What Changes

- Startup fork-setup failures are classified with the same transient/permanent classification the mid-iteration recovery path uses (including its default-to-transient posture for unrecognized errors).
- A TRANSIENT failure no longer skips for the process lifetime: the repository's polling task spawns in a fork-pending state that re-attempts fork setup at the start of each iteration (doing no other work for that repository until setup succeeds), logs a WARN per failed attempt, and posts one throttled chatops alert while pending. Success flips the task to normal operation with an INFO recovery notice — no operator action, no restart.
- The reachability-timeout case ("fork created but not reachable within 60s") is transient — GitHub populates large forks asynchronously, and the next iteration's probe usually succeeds.
- A PERMANENT failure (fork-identity mismatch, underivable fork URL, unroutable PAT, permanent-classified HTTP statuses) keeps today's behavior exactly: skip for the process lifetime, alert with the restart-or-reload remedy.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `orchestrator-cli`: the "Startup verification of fork existence" requirement branches its failure handling on the existing transient/permanent classification instead of treating every failure as process-lifetime.

## Impact

- `autocoder/src/cli/run.rs`: `ensure_forks_exist_with` returns classified failures; transient ones spawn fork-pending polling tasks instead of joining the skip list.
- `autocoder/src/polling_loop/`: a fork-pending task state whose iteration body is "retry fork setup, then park until next tick" until success.
- Reuses `classify_recovery_failure` (or its pattern set) — no new classification rules; alert plumbing reuses the existing throttle machinery.
- Resolves the `TODO(a14)` comment on `repo_passes_startup_check` for the fork-setup half of startup.
