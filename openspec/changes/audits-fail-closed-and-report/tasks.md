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

- [ ] 3.1 Extend the `queue_audit` control-socket action (`control_socket.rs`) to accept optional `{ channel, thread_ts, request_id }`; change the queue element from `String` to `QueuedAudit { audit_type, origin: Option<ChatOrigin> }` (`polling_loop/mod.rs`, the task handle).
- [ ] 3.2 Thread the originating channel/thread from the chatops `audit` dispatcher (`chatops/operator_commands.rs::dispatch_audit_now`) into the `queue_audit` submission.
- [ ] 3.3 After a queued audit resolves, have the scheduler post the terminal completion notification to its `origin` via the chatops backend; a cadence-driven run (no origin) posts none.
- [ ] 3.4 Implement the chatops completion-notification surface (`chatops/` + the notification family): findings / no-findings-with-summary / did-not-complete-with-cause, delivered on the threaded-notification path with non-threaded fallback.
- [ ] 3.5 CLI `audit run` (`cli/audit.rs`): submit `origin: None` and print the terminal result to stdout; no chatops notification.

## 4. On-demand queue durability

- [ ] 4.1 Replace the top-of-pass `std::mem::take` drain (`polling_loop/mod.rs`) so a queued entry is removed only after its audit has actually run; a pass that skips (busy marker), returns early (`ensure_initialized` failure), or is bounded out (`max_audits_per_iteration: 0`) leaves the entry for a later iteration.
- [ ] 4.2 Persist the on-demand queue to a state-dir JSON file (atomic tempfile+rename); load it into `pending_audit_runs` at task spawn and reconcile orphaned entries against live tasks at startup, so a restart between ack and run does not lose the queued audit.

## 5. Tests

- [ ] 5.1 Outcome unit tests: initial state is non-passing; uncaptured `exit_status` → `DidNotComplete`; declared-but-unpersisted → `DidNotComplete`; no submission → `DidNotComplete`; genuine declared no-findings → `SpecsWritten(vec![])` carrying the summary.
- [ ] 5.2 Scheduler tests: `DidNotComplete` does NOT advance cadence state AND posts the failure alert.
- [ ] 5.3 On-demand tests: `queue_audit` carries origin; a resolved queued audit posts the completion notification to that origin; a cadence run posts none.
- [ ] 5.4 Durability tests: busy-skip / init-failure / bound-zero retain the queued entry; a persisted queue is restored on startup and runs.
- [ ] 5.5 Chatops tests: the three terminal notifications render with the examined-summary / cause and degrade gracefully without thread support.

## 6. Documentation + acceptance

- [ ] 6.1 Update docs in the same change (per `project-documentation`): the README audits table and `docs/OPERATIONS.md` (periodic audits / on-demand triggers) describe the fail-closed outcomes; `docs/CHATOPS.md` documents the on-demand `audit` completion notification.
- [ ] 6.2 `openspec validate audits-fail-closed-and-report --strict` passes AND the full `cargo test` suite is green.
