## ADDED Requirements

### Requirement: `ExecutorOutcome::Aborted` distinguishes operator-shutdown-initiated subprocess kills from real failures

The `ExecutorOutcome` enum SHALL gain a new variant `Aborted { reason: String }`. This variant represents an executor subprocess that exited because the daemon itself was being shut down (the SIGTERM the daemon received cascaded to the executor's process group, killing the wrapped CLI child by signal — the reaped `ExitStatus` reports `signal() == Some(15)`, i.e. killed by SIGTERM/signal 15). The variant is structurally distinct from `Failed` so the polling-loop AND the failure-counter mechanism can treat it differently — specifically, `Aborted` SHALL NOT count against `executor.perma_stuck_threshold`.

The polling-loop's outcome dispatcher SHALL handle `Aborted` by:

1. Logging INFO `executor aborted: {reason}` naming the change.
2. Dropping `.in-progress` per the canonical openspec-queue-engine "Unlocking after any executor outcome" requirement.
3. NOT incrementing the per-change failure counter.
4. NOT writing `.perma-stuck.json`.
5. NOT posting a chatops failure alert (operator initiated the shutdown; they don't need notification).
6. Leaving `.iteration-pending.json` untouched (mirrors the `Failed` arm's preservation of continuation context).
7. Returning `Ok(())` from the per-change processing function. The polling loop continues its shutdown sequence normally; the change remains pending AND retries on the next polling cycle after restart.

#### Scenario: `Aborted` does NOT increment failure counter
- **WHEN** the polling-loop's outcome dispatcher receives `Ok(ExecutorOutcome::Aborted { reason: "daemon shutdown (SIGTERM cascade)" })` for change `a35-foo`
- **THEN** the `consecutive_failures` counter for `a35-foo` is NOT incremented
- **AND** `.perma-stuck.json` is NOT written for `a35-foo`
- **AND** no `❌ Failed` OR `:no_entry: perma-stuck` chatops alert fires
- **AND** the `.in-progress` lock is dropped
- **AND** `.iteration-pending.json` (if present) is preserved

#### Scenario: Two consecutive `Aborted` outcomes do NOT perma-stuck
- **WHEN** the same change receives `Aborted` outcomes in two consecutive polling iterations (e.g., operator restarts the daemon twice in a row while the change is mid-iteration)
- **THEN** the change is NOT perma-stuck
- **AND** the failure counter remains at 0 throughout
- **AND** the third polling iteration picks up the change fresh AND attempts implementation normally

### Requirement: Classifier returns `Aborted` for a SIGTERM-killed subprocess during daemon shutdown; preserves `Failed` for external-source SIGTERMs

The `classify_outcome` path in `claude_cli.rs` SHALL inspect a process-wide shutdown flag (`crate::daemon::SHUTDOWN_REQUESTED: AtomicBool`) when the wrapped CLI was killed by SIGTERM. Because the daemon spawns the wrapped CLI directly in its own process group, a SIGTERM cascade reaps the child *by signal*, so the reaped `ExitStatus` reports `signal() == Some(15)` (`code()` returns `None` for any signal-killed process). The classifier SHALL detect the SIGTERM kill as `status.signal() == Some(15) || status.code() == Some(143)` — the former is the production shape; the latter is accepted defensively for the shell "128 + 15" convention that surfaces only if a wrapper OR the CLI itself catches SIGTERM and `exit(143)`s. The flag SHALL be set to `true` by the daemon's SIGTERM handler BEFORE the daemon initiates shutdown of child tasks (so classifier checks happening during the shutdown cascade observe the flag as true). The flag is one-way per process lifetime (false → true; never reset).

Classification rules for a SIGTERM-killed subprocess (`signal() == Some(15)` OR `code() == Some(143)`):

- `SHUTDOWN_REQUESTED == true` → return `ExecutorOutcome::Aborted { reason: "daemon shutdown (SIGTERM cascade)" }`. The subprocess was killed by the cascade from the daemon's own SIGTERM; not the change's fault.
- `SHUTDOWN_REQUESTED == false` → return `ExecutorOutcome::Failed { reason: <stderr excerpt, OR the Display of the status — e.g. "executor exited with signal: 15 (SIGTERM)"> }` (today's behavior, preserved). An external source (OOM killer, manual `kill -TERM <pid>`, container orchestrator) sent the executor a SIGTERM; this is treated as a real failure AND counts against the failure budget.

Classification rules for subprocesses NOT killed by SIGTERM are UNCHANGED by this requirement. The shutdown flag SHALL NOT affect:

- Exit status `0` paths (still classified as `Completed` per the diff-presence heuristic OR existing happy-path rules).
- Other non-zero exit codes / other signals (still classified as `Failed` with the stderr-derived reason).
- Timeout cases (still classified per the canonical timeout-precedence requirement).
- Tool-recorded outcomes (still classified per the canonical "Tool-recorded outcomes take precedence" requirement from `a27a0`).

The flag's purpose is narrow: distinguish the one specific case where the daemon's own shutdown caused the executor's death. Every other classification path is preserved.

#### Scenario: SIGTERM-killed subprocess during daemon shutdown classifies as Aborted
- **WHEN** `classify_outcome` is called with `outcome.exit_status` reporting `signal() == Some(15)` (a SIGTERM-killed child) AND `SHUTDOWN_REQUESTED.load(SeqCst) == true`
- **THEN** the classifier returns `Ok(ExecutorOutcome::Aborted { reason: "daemon shutdown (SIGTERM cascade)" })`
- **AND** the same result holds for the defensive `code() == Some(143)` form (a wrapper/CLI catching SIGTERM and exiting 143)
- **AND** no failure-counter increment OR alert fires downstream

#### Scenario: SIGTERM-killed subprocess without daemon shutdown classifies as Failed (today's behavior)
- **WHEN** `classify_outcome` is called with `outcome.exit_status` reporting `signal() == Some(15)` AND `SHUTDOWN_REQUESTED.load(SeqCst) == false`
- **THEN** the classifier returns `Ok(ExecutorOutcome::Failed { reason })` where `reason` is the stderr excerpt (or the Display of the signal-killed status when stderr is empty, naming `signal: 15`)
- **AND** the existing failure-counter + perma-stuck protections fire normally (external SIGTERMs from OOM killer, manual kill, etc., remain protected against loop)

#### Scenario: Exit-1 with shutdown flag set still classifies as Failed
- **WHEN** `classify_outcome` is called with `outcome.exit_status: ExitStatus(code: 1)` AND `SHUTDOWN_REQUESTED.load(SeqCst) == true` (e.g., the executor genuinely failed with a non-SIGTERM exit code during the shutdown window)
- **THEN** the classifier returns `Ok(ExecutorOutcome::Failed { reason: <stderr excerpt> })` per the existing behavior
- **AND** the shutdown flag does NOT override non-SIGTERM exit codes (the flag's gate is specifically on signal-15 / exit-143 deaths, not all exit codes during shutdown)

#### Scenario: Exit-0 with shutdown flag set still classifies via existing happy-path rules
- **WHEN** `classify_outcome` is called with `outcome.exit_status: ExitStatus(code: 0)` AND `SHUTDOWN_REQUESTED.load(SeqCst) == true` (e.g., the executor completed cleanly just before the daemon's shutdown timing)
- **THEN** the classifier proceeds through the existing exit-0 path (Completed-via-tool-outcome, OR Completed-via-diff, OR the canonical Layer-2 heuristic)
- **AND** the shutdown flag does NOT mask a legitimate Completed outcome

### Requirement: Daemon's SIGTERM handler sets the shutdown flag as its first action

The daemon's SIGTERM signal handler SHALL set `SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst)` as its FIRST action, BEFORE initiating the shutdown of child tasks (polling-loop futures, chatops listener, control-socket listener, etc.). This ordering is load-bearing: the SIGTERM cascade to executor subprocesses happens AFTER the daemon's children begin shutting down, AND those subprocesses' classifier checks must observe `SHUTDOWN_REQUESTED == true`.

The flag SHALL NOT be reset during the process's lifetime (one-way false → true). A subsequent daemon restart starts a new process with the flag at its `AtomicBool::new(false)` default.

#### Scenario: SIGTERM handler sets flag before cascading
- **WHEN** the daemon receives SIGTERM
- **THEN** the SIGTERM handler's first action is `SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst)`
- **AND** subsequent shutdown actions (graceful child cancellation, socket close, etc.) happen AFTER the flag store

#### Scenario: Flag persists for the rest of the process lifetime
- **WHEN** the flag has been set to `true` AND the daemon's shutdown sequence proceeds
- **THEN** the flag remains `true` until the process exits
- **AND** any classifier call happening during the shutdown cascade observes the flag as `true`

#### Scenario: Fresh daemon process starts with flag false
- **WHEN** a new daemon process spawns (e.g., post-restart)
- **THEN** `SHUTDOWN_REQUESTED.load(Ordering::SeqCst)` returns `false`
- **AND** the next iteration's classifier calls classify exit codes per the non-shutdown path
