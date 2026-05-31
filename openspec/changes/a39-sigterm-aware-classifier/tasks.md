# Tasks

## 1. New `ExecutorOutcome::Aborted` variant

- [ ] 1.1 Add `Aborted { reason: String }` variant to `ExecutorOutcome` in `autocoder/src/executor/mod.rs`. Document inline that this variant is for "subprocess killed by signal during operator-initiated daemon shutdown; should NOT count against `consecutive_failures`."
- [ ] 1.2 Unit-test: `ExecutorOutcome::Aborted` round-trips through any Debug/match arms the codebase has. Add matches AS NEEDED for compiler exhaustiveness.

## 2. Shutdown-flag tracking

- [ ] 2.1 In `autocoder/src/daemon.rs` (OR the equivalent module hosting the daemon's lifecycle/SIGTERM handler), add `pub static SHUTDOWN_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);`.
- [ ] 2.2 The existing SIGTERM handler SHALL set `SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst)` as its first action, BEFORE initiating shutdown of child tasks. This ordering ensures classifier checks happening DURING the shutdown cascade see the flag as true.
- [ ] 2.3 Unit-test the flag default state (false at startup).
- [ ] 2.4 Integration test (sigterm-handler-shape): a fixture that invokes the daemon's SIGTERM handler observes the flag flip to true.

## 3. Classifier exit-143 + shutdown-flag check

- [ ] 3.1 In `autocoder/src/executor/claude_cli.rs::classify_outcome`, add a check AFTER the existing exit-status path (line ~1140) AND BEFORE the diff-presence Completed fallback:
  - If `exit_status.code() == Some(143)` AND `crate::daemon::SHUTDOWN_REQUESTED.load(Ordering::SeqCst)` is `true`: return `Ok(ExecutorOutcome::Aborted { reason: "daemon shutdown (SIGTERM cascade)".to_string() })`.
  - Otherwise fall through to existing behavior.
- [ ] 3.2 Unit-test: classifier with exit_status=143 AND shutdown flag set → returns `Aborted` with the documented reason.
- [ ] 3.3 Unit-test: classifier with exit_status=143 AND shutdown flag clear → returns `Failed { reason: "executor exited with exit status: 143" }` (existing behavior preserved for external SIGTERMs).
- [ ] 3.4 Unit-test: classifier with exit_status=1 AND shutdown flag set → returns existing `Failed { reason: <stderr excerpt> }` (the flag does NOT override non-143 exit codes).
- [ ] 3.5 Unit-test: classifier with exit_status=0 AND shutdown flag set → returns `Completed` per existing diff-presence path (the flag does NOT override clean exits).

## 4. Polling-loop `Aborted` arm

- [ ] 4.1 Add the `Ok(ExecutorOutcome::Aborted { reason })` arm to the polling-loop's outcome dispatcher (next to the existing `Failed`, `Completed`, etc. arms). Behavior:
  - Log INFO `executor aborted: {reason}` naming the change.
  - Call `queue::unlock(workspace, change)` to drop `.in-progress` (per the canonical unlock-on-any-outcome requirement).
  - Do NOT call `handle_failure_counter` OR equivalent counter-incrementing helper.
  - Do NOT write `.perma-stuck.json`.
  - Do NOT post any chatops failure alert.
  - Do NOT touch `.iteration-pending.json` (mirrors the `Failed` arm — preserves continuation context if any).
  - Return `Ok(QueueStep::Failed)` OR a new `Ok(QueueStep::Aborted)` variant (implementer's choice; the queue-walk's behavior on `Aborted` mirrors `Failed`'s halt-the-walk semantics).
- [ ] 4.2 Add the corresponding arm to every other outcome-dispatch site in `polling_loop.rs` (there are several; the test `Ok(crate::executor::ExecutorOutcome::IterationRequested { .. })` pattern indicates ~4-5 sites). Each site treats `Aborted` per the contract above.
- [ ] 4.3 Unit-test: polling-loop arm with a stub executor returning `Aborted { reason: "..." }` asserts `.in-progress` is dropped, failure counter is NOT incremented, `.perma-stuck.json` is NOT written.
- [ ] 4.4 Integration test: two consecutive `Aborted` outcomes for the same change do NOT trigger perma-stuck (counter stays at 0; marker absent).

## 5. Validation

- [ ] 5.1 `cargo test` passes.
- [ ] 5.2 `cargo clippy` produces no NEW warnings against the existing baseline.
- [ ] 5.3 `openspec validate a39-sigterm-aware-classifier --strict` passes.
