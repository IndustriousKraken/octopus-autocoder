## Why

The `gatekeepers-fail-closed` standard establishes that a control-plane gatekeeper's inability to run is a distinct, surfaced, non-passing state — never a pass. The verifier gates currently VIOLATE that standard: the `[in]` and `[canon]` pre-flights are spec'd fail-OPEN (a session error / unavailable CLI / no submission → "no contradictions found" → proceed), and the `[out]` gate omits its section on failure (reads as "not configured / passed"). This is the conformance change that brings the gates in line with the standard — the implementation already lands here.

## What Changes

- **`[in]` / `[canon]` (blocking) → fail CLOSED.** A gate that cannot run (CLI unavailable / unregistered strategy, spawn/timeout error, no submission, or an uncorrected schema rejection) no longer proceeds as "no contradictions." It writes the `.needs-spec-revision.json` marker with a structured `gate_error` (distinct from a findings-based revision), posts a distinct "gate FAILED TO RUN — change held" chatops alert, and halts the queue walk. The change is held because it was NOT evaluated; an operator clears the marker (after fixing the gate) to retry. A successful run with an empty result is still `Clean` → proceed; findings still block as before.
- **`[out]` (advisory) → fail CLOSED to a VISIBLE state.** On failure the gate renders an explicit `## Spec Verification: FAILED TO RUN — <cause>` section (NOT a pass) instead of omitting the section. It remains advisory: it never blocks PR creation.
- **Verifier-gate framework** posture updated from "pre-executor gates are fail-open" to "fail-closed (held)"; the `[out]` advisory posture clarified to "fail-closed to a visible state, never silent."

## Impact

- **Affected specs:** `orchestrator-cli` — MODIFY `Verifier-gate framework`, `Change-internal contradiction pre-flight check (opt-in)`, `Change-vs-canonical contradiction pre-flight check (the [canon] gate)`, AND `Code-implements-spec verification (the [out] gate, advisory)`.
- **Affected code (already implemented):** the gate run functions return a three-way `Clean | Found | Errored` outcome (`preflight/change_contradiction.rs`, `preflight/canon_contradiction.rs`); `code_implements_spec.rs` returns `Verified | FailedToRun { cause }`. The `.needs-spec-revision.json` marker (`spec_revision.rs`) gains a structured `gate_error` population + a gate-error `operator_action`. The pre-flight callers (`polling_loop/preflight_checks.rs`) hold on `Errored` via a shared `handle_gate_error`; a distinct `maybe_post_gate_error_alert` (`polling_loop/alerts_throttle.rs`) surfaces the held state; `polling_loop/pass.rs` renders the `[out]` FAILED TO RUN section.
- **Operator-visible:** a misconfigured / non-functioning blocking gate now HOLDS the change with a clear "FAILED TO RUN" alert (was: silently waved through). An opencode gate whose `submit_*` could not route — previously a silent fail-open — now surfaces. The `[out]` PR section says FAILED TO RUN instead of vanishing.
- **Relationship:** realizes the conformance follow-on named in `gatekeepers-fail-closed`.
- **Acceptance:** `cargo test` (gate-, marker-, and caller-level tests assert held-on-error, not proceed) + `openspec validate verifier-gates-fail-closed --strict`.
