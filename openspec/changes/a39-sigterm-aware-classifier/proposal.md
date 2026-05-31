## Why

When an operator runs `systemctl restart autocoder` (OR `systemctl stop autocoder` followed by a manual start) while a polling iteration's executor subprocess is mid-run, systemd sends `SIGTERM` to the daemon. The daemon's process-group setup (per the canonical executor sandbox config) ensures the SIGTERM cascades to the executor's wrapped CLI subprocess too. The subprocess exits with status code `143` (= 128 + 15, the standard Unix convention for "killed by signal 15 / SIGTERM").

Today's `classify_outcome` path treats exit status 143 as any other non-zero exit: it returns `ExecutorOutcome::Failed { reason: "executor exited with exit status: 143" }`. The polling loop's failure-counter increments `consecutive_failures` for the change. After `executor.perma_stuck_threshold` consecutive failures (default `2`), the change is marked perma-stuck AND blocks further iterations until an operator clears the marker.

**Observed in production**: an operator restarted the daemon twice in a row (deploying patches mid-iteration). Each restart killed an in-flight `a35-thread-daemon-paths-globals-removal` iteration with SIGTERM → exit 143 → `Failed`. Two consecutive failures triggered perma-stuck. The operator had to manually `clear-perma-stuck` to recover. **Neither failure was a real agent failure — both were operator-initiated daemon restarts.** Counting them against the change's failure budget conflates two distinct events:

- **Real failure**: the agent did the work AND it didn't produce a valid outcome (timeout, exit error, parse failure, etc.). The change's failure budget SHOULD count this.
- **Aborted-by-daemon-shutdown**: the subprocess was killed by signal because the daemon is shutting down. The change made no choice; it's not the change's fault. The failure budget SHOULD NOT count this.

The fix is narrow: when `exit_status == 143` AND the daemon-shutdown path is active (i.e., the daemon itself received SIGTERM AND is propagating to children), the classifier SHALL return a new outcome variant `Aborted { reason: "daemon shutdown" }` that the failure-counter does NOT count as a `consecutive_failures` increment. On daemon restart, the change is picked up fresh AND retries from the agent-q tip — same as today's recovery path, except the failure counter doesn't tick.

Distinguishing "SIGTERM from daemon shutdown" from "SIGTERM from external source" requires the daemon to track its own shutdown state. The simplest mechanism: a process-wide `AtomicBool` set by the daemon's SIGTERM handler that the classifier consults. When the flag is set AND exit_status is 143, the outcome is `Aborted`. When the flag is clear AND exit_status is 143 (e.g., an unrelated process sent the executor a SIGTERM, OR the OOM killer reaped it with SIGTERM), the outcome is the existing `Failed` (preserving today's perma-stuck protection against external-kill loops).

## What Changes

**New `ExecutorOutcome::Aborted { reason: String }` variant.** Sibling to `Failed`, `Completed`, `AskUser`, etc. The polling-loop's outcome dispatcher routes this variant to a new arm that:

1. Drops `.in-progress` per the canonical "Unlocking after any executor outcome" requirement.
2. Does NOT increment `consecutive_failures`.
3. Does NOT write `.perma-stuck.json`.
4. Does NOT post a chatops failure alert (operator initiated the shutdown; they don't need a notification).
5. Returns `Ok(())` from the per-change processing function. The polling loop continues its shutdown sequence normally.

The change directory's `.iteration-pending.json` marker (if present) SHALL be left untouched — same as the `Failed` arm — so a retry after restart preserves continuation context.

**Process-wide daemon-shutdown tracking.** A new `static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false)` in `autocoder/src/daemon.rs` (OR equivalent module). The existing SIGTERM handler at daemon startup SHALL set this flag to `true` before initiating shutdown. The flag is one-way (false → true; never reset within a process lifetime). A new daemon process starts with the flag false.

**Classifier consults the flag.** In `claude_cli.rs::classify_outcome`, when `exit_status.code() == Some(143)`:

- If `SHUTDOWN_REQUESTED.load(Ordering::SeqCst)` is `true`: return `Aborted { reason: "daemon shutdown (SIGTERM cascade)" }`.
- If `SHUTDOWN_REQUESTED` is `false`: return today's `Failed { reason: "executor exited with exit status: 143" }` behavior (preserves protection against external-kill loops like OOM, manual `kill -TERM <pid>`, container orchestration kills, etc.).

**No new config knobs.** The behavior is unconditional: operator-initiated daemon restart never counts against `consecutive_failures`. There's no operator-facing reason to opt out — counting daemon shutdown as a failure is purely operator-pain.

**No spec changes to perma-stuck mechanics.** The `Aborted` outcome simply bypasses the failure counter. Perma-stuck still fires for genuine `Failed` outcomes. Operators who manually issue `kill -TERM <executor-pid>` (rare; mostly debugging) still see the change fail per existing semantics.

## Impact

- **Affected specs:**
  - `executor` — ADDED a new `Aborted` variant on the `ExecutorOutcome` enum. ADDED a requirement defining the SIGTERM-during-daemon-shutdown classification rule. The existing `classify_outcome` ordering AND scenarios are unchanged for every non-143 exit code AND for non-shutdown exit-143 cases.
- **Affected code:**
  - `autocoder/src/executor/mod.rs` — `ExecutorOutcome` enum gains `Aborted { reason: String }` variant.
  - `autocoder/src/executor/claude_cli.rs::classify_outcome` — gains the exit-143 + shutdown-flag check. Returns `Aborted` when both hold; falls through to existing `Failed` otherwise.
  - `autocoder/src/daemon.rs` (OR equivalent) — adds `static SHUTDOWN_REQUESTED: AtomicBool`; SIGTERM handler sets it before initiating shutdown.
  - `autocoder/src/polling_loop.rs` — outcome dispatcher gains the `Aborted` arm. Implementation matches the design above (drop `.in-progress`, no counter, no marker, no alert, return Ok(())).
  - Tests covering: shutdown-flag-set + exit-143 → Aborted; shutdown-flag-clear + exit-143 → Failed; shutdown-flag-set + non-143 → existing classifier behavior (the shutdown flag doesn't override genuine completion paths).
- **Operator-visible behavior:**
  - `systemctl restart autocoder` while a change is mid-iteration NEVER causes the change to perma-stuck on restart.
  - The change retries from agent-q tip on the next polling iteration as today.
  - No new chatops notifications. The aborted run produces no `❌ Failed` alert (operator initiated the shutdown).
  - journalctl shows `executor aborted: daemon shutdown (SIGTERM cascade)` at INFO level naming the change.
- **Backward compatibility:** existing behavior preserved for every exit code except 143-during-shutdown. External SIGTERMs (OOM, manual kill, etc.) still classify as Failed.
- **Dependencies:** none. Independent of every queued change. Can land in any order.
- **Acceptance:** `cargo test` passes; `openspec validate a39-sigterm-aware-classifier --strict` passes. Tests:
  - Classifier called with exit_status 143 AND `SHUTDOWN_REQUESTED: true` returns `Aborted { reason: "daemon shutdown (SIGTERM cascade)" }`.
  - Classifier called with exit_status 143 AND `SHUTDOWN_REQUESTED: false` returns `Failed { reason: "executor exited with exit status: 143" }` (today's behavior preserved for external kills).
  - Classifier called with exit_status 1 AND `SHUTDOWN_REQUESTED: true` returns `Failed { reason: <stderr excerpt> }` (the shutdown flag doesn't override non-143 exit codes).
  - Polling-loop's `Aborted` arm drops `.in-progress`, leaves `.iteration-pending.json` untouched, does NOT increment the failure counter, does NOT write `.perma-stuck.json`, does NOT post a chatops failure alert.
  - Integration test: a fixture where two `Aborted` outcomes for the same change in succession do NOT trigger perma-stuck (the counter stays at 0 across both).
