## Why

The audit framework is a control whose job is to surface problems — missing tests, security bugs, drift. A control that reports "all clear" when it could not actually run is worse than no control: it removes the rail while reporting green. The specs-writing audit harness does exactly this today. It initializes its outcome to the success shape (`AuditOutcome::SpecsWritten`) and derives "no findings" purely from "no new `openspec/changes/` directories appeared on disk." Its terminal-error check only catches a timed-out or non-zero-exit session, so three distinct non-results all collapse into a green "wrote 0 new spec(s)": a clean exit-0 session that did no real work, a session whose exit status was never captured, and — observed in practice — a session that surveyed thoroughly and identified a real finding but could not persist it because writes were denied.

On-demand audits compound the opacity. An operator who triggers `audit <type> <repo>` receives the optimistic "✓ Queued, will run on the next polling iteration" acknowledgement and then nothing — whether the audit ran clean, found nothing, failed to run, or was silently dropped from an in-memory queue before it ever executed. The trigger and the result are disconnected.

This change brings the audit framework into conformance with the `gatekeepers-fail-closed` standard: an inability to run is a distinct, surfaced, non-passing state — never a silent pass — and every operator-triggered audit reports a terminal outcome with evidence of what it examined.

## What Changes

- **Fail-closed audit outcome initialization.** An audit run begins in an explicit "did not complete" state. Only a session that demonstrably ran to completion AND produced its expected artifact (or genuinely concluded no-findings with positive evidence of a survey) may resolve to a passing or no-findings outcome. A specs-writing session that produced output but persisted zero change directories, OR whose exit status was not captured, OR that shows no evidence of a real survey, resolves to a **surfaced failure** — chatops alert, cadence state NOT advanced — not "0 findings." Transient conditions get bounded retry, then errored; never fail-open.
- **Survey-evidence capture.** The audit session's final-answer summary (what it examined and its conclusion) is captured and carried on the outcome, so even a legitimate zero-findings run ships with evidence that the audit actually looked.
- **On-demand completion notification.** The chatops `audit` verb (and the CLI `audit run`) thread the originating channel and thread through the `queue_audit` action into the scheduler, which posts a terminal result back to that thread — "completed, N findings, here is what I examined" on success, and the explicit failed-to-run states above on failure.
- **On-demand queue durability.** A queued on-demand audit is no longer lost when the pass it would run in is skipped (busy marker), returns early (workspace-init failure), is bounded out (`max_audits_per_iteration: 0`), or the daemon restarts. A queued entry is removed only once its audit has actually run; otherwise it survives to a later iteration.

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `orchestrator-cli`: audit-framework outcome semantics (fail-closed initialization; "could not run" / "could not persist" / "no evidence of survey" become surfaced non-passing outcomes, distinct from "no findings"); survey-evidence capture on the outcome; the `queue_audit` control-socket action carries the originating chat channel/thread and an on-demand completion notification is emitted; the on-demand audit-run queue is durable across pass-skip, early-return, bound-zero, and restart.
- `chatops-manager`: an on-demand audit completion notification (success with examined-summary, and the failed-to-run states) delivered to the thread the `audit` request originated from.

## Impact

- **Code:** the specs-writing audit harness and advisory-audit terminal-error handling (`audits/specs_writing.rs`, `audits/mod.rs`, `audits/scheduler.rs`); the `AuditOutcome` enum (a non-passing "did not complete" variant carrying cause + captured summary); the `queue_audit` control-socket handler and `pending_audit_runs` drain (`control_socket.rs`, `polling_loop/mod.rs`); the chatops on-demand dispatch and notification surface (`chatops/operator_commands.rs`, `chatops/mod.rs`).
- **Governance:** conforms to the project-documentation `gatekeepers-fail-closed` standard; the periodic `drift_audit` and the `[canon]` gate can flag a future audit that regresses to a passing default.
- **Out of scope / already addressed:** the acute sandbox defect that mounted specs-writing audits read-only (contradicting the canon's OpenSpec-only sandbox that allows `Write`/`Edit`) is fixed separately as a code-vs-spec correction; this change is the systemic fail-closed and observability layer above that fix.
- **Non-goals:** no change to which audits exist or their cadences; no change to the post-hoc `WritePolicy` enforcement; no new audit types.
