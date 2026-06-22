# Legible, bounded-retried executor outcomes

## Why

When an executor session fails, the operator-facing notification says only
`executor exited with exit status: N`. The actual cause — a transient upstream
API overload, a panic, a killed process — is buried in server-side logs, so a
30-second infrastructure blip is indistinguishable from a real failure without
SSHing in and reading the per-run log. Separately, the daemon re-runs the ENTIRE
session on subsequent passes with no backoff, repeatedly hitting the same
still-ongoing transient condition.

The executor is a swappable CLI strategy fronting any model API, so the fix MUST
be provider-agnostic: it cannot rely on parsing provider-specific error formats.
The principle is therefore: surface the captured evidence RAW (never parse it for
the decision), and retry any no-result failure a bounded number of times with
backoff (never classify it).

## What Changes

- **Assembled, legible failure reason.** On any non-success executor outcome, the
  `reason` is assembled from the evidence already captured — the agent's final
  message (if non-empty), captured STDERR (if non-empty), and the process exit
  status / terminating signal (always; and the sole content when the others are
  empty) — each truncated to a budget. Raw and uninterpreted; provider-agnostic.
- **Bounded retry with backoff.** An executor session that fails AND produced no
  committable result is retried up to `executor.session_retries` times with
  backoff before the failure is surfaced. No error classification: a transient
  clears on retry; a deterministic failure recurs through all attempts and is then
  surfaced. Sessions that DID produce committable work are never blindly re-run.
- **Optional, strategy-local retry hint.** `CliStrategy` gains an optional,
  defaulted `is_retryable(&outcome) -> Option<bool>` so an adapter MAY encode its
  own provider's signals; the default (`None`) means "retry per the bounded
  no-result rule". Provider knowledge stays in the strategy that owns it.
- **The operator-facing failure notification carries the assembled reason** rather
  than a bare exit code.

## Impact

- Affected capabilities: `executor` (the assembled reason in the failure contract;
  the optional `CliStrategy::is_retryable` hook), `orchestrator-cli` (the bounded
  no-result retry loop AND the `executor.session_retries` knob), `chatops-manager`
  (the failure notification surfaces the assembled reason).
- Affected code: `autocoder/src/executor/claude_cli.rs` (failure-reason assembly;
  reuse `agentic_run::failure_excerpt`), `autocoder/src/agentic_run.rs`
  (`AgenticRunOutcome`, the `CliStrategy` trait, `failure_excerpt`),
  `autocoder/src/polling_loop/outcome.rs` (the no-committable-result signal +
  retry loop), `autocoder/src/config.rs` (`executor.session_retries`),
  `autocoder/src/revisions.rs` + the pass/alert notification paths (render the
  assembled reason).
- Provider-agnostic by construction: no code parses error strings; classification,
  if any, is encapsulated per strategy.
