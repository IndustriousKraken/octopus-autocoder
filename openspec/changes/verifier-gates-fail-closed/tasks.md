# Tasks

The implementation is complete; this change reconciles the canonical spec with it.

## 1. Spec reconciliation (this delta)

- [x] 1.1 MODIFY `Verifier-gate framework`, `Change-internal contradiction pre-flight check`, `Change-vs-canonical contradiction pre-flight check`, AND `Code-implements-spec verification` in `orchestrator-cli`: pre-executor gates fail CLOSED (held), the `[out]` gate renders FAILED TO RUN.

## 2. Implementation (done)

- [x] 2.1 `[in]`/`[canon]` gate run fns return `Clean | Found | Errored` (`preflight/change_contradiction.rs`, `preflight/canon_contradiction.rs`) — error/no-submission/unregistered-strategy → `Errored`, never empty-`Clean`.
- [x] 2.2 `spec_revision.rs`: `.needs-spec-revision.json` gains a structured `gate_error` population (gate + cause) + a gate-error `operator_action`.
- [x] 2.3 `polling_loop/preflight_checks.rs`: `Errored` → `handle_gate_error` writes the `gate_error` hold marker + halts; `Clean` → proceed; `Found` → existing findings marker.
- [x] 2.4 `polling_loop/alerts_throttle.rs`: `maybe_post_gate_error_alert` posts the distinct "gate FAILED TO RUN — change held" alert (shares the `SpecNeedsRevision` throttle).
- [x] 2.5 `[out]` gate: `code_implements_spec.rs` returns `Verified | FailedToRun { cause }`; `polling_loop/pass.rs` renders `render_spec_verification_failed_section` (a `## Spec Verification: FAILED TO RUN` section) instead of omitting.

## 3. Tests (done)

- [x] 3.1 Gate-level: error / no-submission / unregistered-strategy → `Errored` (not `Clean`); empty submission → `Clean`; findings → `Found` (`change_contradiction.rs`, `canon_contradiction.rs`).
- [x] 3.2 Marker-level: a `gate_error` marker serializes the structured field AND sets the gate-error `operator_action` (`spec_revision.rs`).
- [x] 3.3 Caller-level: a no-submission `[in]`/`[canon]` gate holds the change (executor NOT invoked, `gate_error` marker written) (`polling_loop/tests/t09.rs`).
- [x] 3.4 `[out]`: no submission renders a FAILED TO RUN section (not omitted) (`polling_loop/tests/t01.rs`).

## 4. Documentation

- [x] 4.1 `docs/OPERATIONS.md` (Pre-flight checks + SpecNeedsRevision section): a gate that cannot run HOLDS the change (failed-to-run alert + `gate_error` marker) rather than failing open; `[out]` renders FAILED TO RUN. (`docs/CHATOPS.md` clear-revision wording already covers the operator path.)

## 5. Acceptance

- [x] 5.1 `cargo test` passes (only the pre-existing parallel-load flakes intermittently fail; all pass isolated).
- [x] 5.2 `openspec validate verifier-gates-fail-closed --strict` passes.
