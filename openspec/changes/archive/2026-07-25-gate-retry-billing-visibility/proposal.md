# gate-retry-billing-visibility

## Why

A verifier-gate session that ends without calling its submission tool is re-attempted up to `executor.verifier_gate_retries` times (default 2), and every re-attempt is a fresh, fully billed agentic session. The behavior is sound — a flaky model deserves a second chance before the gate fails closed — but it is completely invisible: nothing in chatops, the PR, or the logs tells the operator that one gate invocation just billed three sessions. A model that habitually skips its submission tool multiplies gate spend silently (during the OpenCode permission-prompt incident, every gate session ended with no submission — tripling gate cost across the fleet with no signal). The retry budget itself is also unspecced: no requirement mentions `executor.verifier_gate_retries` today.

## What Changes

- A new requirement blesses the existing retry behavior — no-submission sessions only, bounded by `executor.verifier_gate_retries` (default 2 additional attempts; errors and timeouts still fail closed with no re-attempt) — and makes retries operator-visible.
- When a daemon-run gate invocation consumed at least one re-attempt, autocoder posts one chatops notification naming the gate identifier, the change slug, attempts used vs. allowed, the final disposition, and the model attribution — so the operator can attribute rising spend to the model and act (e.g. switch models).
- Daemon-absent runs (`verify`, standalone) emit the same information as a WARN log line.

## Impact

- Affected specs: `orchestrator-cli` (new requirement)
- Affected code: `autocoder/src/verifier_gate.rs` (surface attempts-used), gate call sites in `autocoder/src/polling_loop/` and `autocoder/src/code_implements_spec.rs` (post the notification), `autocoder/src/cli/verify.rs` (WARN path), `docs/CONFIG.md` (document the field)
