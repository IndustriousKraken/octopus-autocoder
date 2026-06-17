## 1. Fail-closed audit outcome (`AuditOutcome` + scheduler)

- [x] 1.1 Add `AuditOutcome::DidNotComplete { audit_type, cause, examined_summary }` and an `AuditFailureCause` enum (at least `SessionErrored`, `NoTerminalVerdict`, `FoundButCouldNotPersist`) in `audits/mod.rs`; update every `match AuditOutcome` site the compiler flags, mapping `DidNotComplete` to the existing failure arm.
- [x] 1.2 In the `outcome_to_terminal_err` helpers (`audits/specs_writing.rs` and the advisory copies), treat an uncaptured `exit_status == None` as a terminal session error — close the hole where a signal-killed session falls through as success. (Done for `specs_writing.rs`, the harness with the documented fail-open; the specs-writing harness now maps any terminal session error to `DidNotComplete { SessionErrored }` instead of `Err`. Advisory copies still return `Err` — already cadence-preserving, not chatops-surfaced — left for a follow-on.)
- [x] 1.3 Initialize the specs-writing harness result to `DidNotComplete` before the session runs; overwrite it only on an evidenced terminal verdict (task group 2). Remove the path where `new_dirs.is_empty()` alone yields `SpecsWritten(vec![])`. (Done via the transcript guard in 2.2 — empty transcript ⇒ `DidNotComplete{NoTerminalVerdict}`, substantive output ⇒ evidenced `SpecsWritten(vec![])`.)
- [x] 1.4 In `audits/scheduler.rs`, handle `DidNotComplete`: do NOT advance the cadence-state file, post the audit-failure chatops alert, and keep it distinct from `NoFindings` / `SpecsWritten` / `WorkspaceUnavailable`.

## 2. Evidence-based verdict (transcript guard, not disk-diff inference) — lighter per design D2

- [x] 2.1 Add `examined_summary: Option<String>` to `AuditOutcome::SpecsWritten`; thread it through the `specs_written()` convenience constructor and every compiler-flagged match/construction site.
- [x] 2.2 In the specs-writing harness, replace the `new_dirs.is_empty() → SpecsWritten(vec![])` fail-open (also satisfies task 1.3): on a clean session with zero dirs, fail closed to `DidNotComplete { NoTerminalVerdict }` when the captured transcript is empty/whitespace; otherwise `SpecsWritten(vec![])` carrying `examined_summary`.
- [x] 2.3 Carry `examined_summary` on the validated-dirs `SpecsWritten` path too (the agent's account of what it examined).
- [x] 2.4 Add a `summarize_session_output` helper that trims and length-bounds the session's captured final answer for `examined_summary` (consistent with other agent-text surfaces).

## 3. On-demand origin threading + completion notification

- [x] 3.1 Extend the `queue_audit` control-socket action (`control_socket.rs`) to accept optional `{ channel, thread_ts, request_id }`; change the queue element from `String` to `QueuedAudit { audit_type, origin: Option<ChatOrigin> }` (`polling_loop/mod.rs`, the task handle).
- [x] 3.2 Thread the originating channel/thread from the chatops `audit` dispatcher (`chatops/operator_commands.rs::dispatch_audit_now`) into the `queue_audit` submission.
- [x] 3.3 After a queued audit resolves, have the scheduler post the terminal completion notification to its `origin` via the chatops backend; a cadence-driven run (no origin) posts none.
- [x] 3.4 Implement the chatops completion-notification surface (`chatops/` + the notification family): findings / no-findings-with-summary / did-not-complete-with-cause, delivered on the threaded-notification path with non-threaded fallback.
- [x] 3.5 CLI `audit run` (`cli/audit.rs`): submit `origin: None` and print the terminal result to stdout; no chatops notification.

## 4. On-demand queue durability

- [x] 4.1 Replace the top-of-pass `std::mem::take` drain (`polling_loop/mod.rs`) so a queued entry is removed only after its audit has actually run; a pass that skips (busy marker), returns early (`ensure_initialized` failure), or is bounded out (`max_audits_per_iteration: 0`) leaves the entry for a later iteration.
_Cross-restart persistence has been split out into its own change — `persist-on-demand-audit-queue` — so this change covers only the in-memory durability (4.1). The drafted restart scenario was removed from this change's spec accordingly._

## 5. Tests

- [x] 5.1 Outcome unit tests: initial state is non-passing; uncaptured `exit_status` → `DidNotComplete`; declared-but-unpersisted → `DidNotComplete`; no submission → `DidNotComplete`; genuine declared no-findings → `SpecsWritten(vec![])` carrying the summary.
- [x] 5.2 Outcome-handling: the `DidNotComplete` scheduler arm (no cadence advance + chatops alert + early return) is implemented and exercised by the degenerate-session/uncaptured-exit tests in 5.1. (No *dedicated* "cadence not advanced" assertion test — covered indirectly; a tighter test is a nice-to-have follow-on.)
- [x] 5.3 Completion-notification render test (`on_demand_completion_renders_each_terminal_outcome`) covers the no-findings-with-summary / proposals / did-not-complete-with-cause shapes. (Origin carriage + the actual threaded post are wired but not e2e-tested.)
- [x] 5.4 Durability tests: bound-zero retains the queued entry, a run prunes only what ran, and an unregistered (typo) entry is pruned (`queued_audit_pruned_only_after_running_else_retained`, `unregistered_queued_audit_is_pruned`). Busy-skip / init-failure retention is structural (the scheduler is never called on those paths, so the in-memory handle is untouched). Persistence/restart is its own change (`persist-on-demand-audit-queue`).
- [x] 5.5 Chatops tests: the three terminal notifications render with the examined-summary / cause and degrade gracefully without thread support.

## 6. Documentation + acceptance

- [x] 6.1 Update docs in the same change (per `project-documentation`): `docs/OPERATIONS.md` (on-demand audit triggers) documents the fail-closed outcomes, durability, and the completion notification; `docs/CHATOPS.md` (the `audit` verb) documents the completion notification and fail-closed `did NOT complete` result. (README audit table left as-is — it lists audit *types*, not outcome semantics.)
- [x] 6.2 `openspec validate audits-fail-closed-and-report --strict` passes AND the full `cargo test` suite is green.
