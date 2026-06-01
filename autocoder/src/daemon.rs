//! Daemon-lifecycle globals. Houses process-wide state that the daemon
//! lifecycle (startup, signal handlers, shutdown) updates AND that other
//! modules consult.
//!
//! Currently this module only carries `SHUTDOWN_REQUESTED`, the flag the
//! classifier (`executor::claude_cli::classify_outcome`) reads to
//! distinguish operator-initiated daemon shutdowns (where the SIGTERM
//! cascades to the executor subprocess as exit 143) from external
//! SIGTERMs (OOM killer, manual `kill -TERM`, orchestrator kills) that
//! should remain protected by the perma-stuck failure counter.

use std::sync::atomic::{AtomicBool, Ordering};

/// Process-wide flag set to `true` by the daemon's SIGTERM handler
/// BEFORE the daemon initiates shutdown of child tasks. The classifier
/// reads this in `claude_cli::classify_outcome` to map exit status 143
/// (= SIGTERM-killed subprocess) to `ExecutorOutcome::Aborted` instead
/// of `ExecutorOutcome::Failed` — so an operator-initiated daemon
/// restart never counts a mid-iteration change against its
/// `consecutive_failures` budget.
///
/// The flag is one-way per process lifetime (`false` → `true`; never
/// reset). A fresh daemon process starts with the default `false`. See
/// `openspec/specs/executor/spec.md` for the full requirement.
pub static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Mark the daemon as shutting down. The SIGTERM (AND SIGINT) handler
/// calls this as its FIRST action, BEFORE cancelling child tasks, so
/// any classifier check happening DURING the shutdown cascade observes
/// the flag as `true`. Idempotent — the flag is one-way per process
/// lifetime.
pub fn request_shutdown() {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

/// Test-only mutex used to serialize tests that read OR mutate
/// `SHUTDOWN_REQUESTED`. The flag is a process-wide one-way bool in
/// production; tests need to reset it between cases AND must not race
/// each other or the classifier-shutdown tests in other modules.
///
/// Any test that flips the flag MUST take this guard for its entire
/// duration AND reset the flag to `false` (via `reset_for_test`) before
/// dropping the guard. The classifier tests that READ the flag must
/// take this guard too.
#[cfg(test)]
pub static TEST_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Test-only: reset the shutdown flag to `false`. Used by tests that
/// need to assert behavior at a known starting state. Production code
/// never resets this flag (it is one-way per process lifetime). Callers
/// MUST hold `TEST_GUARD` for the duration of the test that uses this.
#[cfg(test)]
pub fn reset_for_test() {
    SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    /// Task 2.3: the flag must be `false` by its `AtomicBool::new(false)`
    /// default. We take the test guard, reset the flag, AND assert.
    /// Resetting first defends against test ordering — another test in
    /// the binary may have flipped the flag earlier.
    #[test]
    fn shutdown_requested_default_is_false() {
        let _g = TEST_GUARD.lock().unwrap();
        reset_for_test();
        assert!(
            !SHUTDOWN_REQUESTED.load(Ordering::SeqCst),
            "SHUTDOWN_REQUESTED must default to false"
        );
        // Leave the flag at its reset state for the next test.
    }

    /// Task 2.4 (sigterm-handler-shape): a fixture that invokes the
    /// shutdown action observes the flag flip to `true`. The SIGTERM
    /// handler's FIRST action (per `cli/run.rs::spawn_signal_handler`)
    /// is exactly `request_shutdown()`, so calling it here exercises
    /// the handler's load-bearing step.
    #[test]
    fn sigterm_handler_shape_flips_flag() {
        let _g = TEST_GUARD.lock().unwrap();
        reset_for_test();
        assert!(
            !SHUTDOWN_REQUESTED.load(Ordering::SeqCst),
            "precondition: flag is false before handler fires"
        );
        request_shutdown();
        assert!(
            SHUTDOWN_REQUESTED.load(Ordering::SeqCst),
            "SHUTDOWN_REQUESTED must be true after the handler's first action"
        );
        reset_for_test();
    }
}
