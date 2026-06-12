## Context

Audits run a wrapped agent CLI in a sandbox and resolve to an `AuditOutcome`. The specs-writing harness (`audits/specs_writing.rs`, used by `security_bug_audit`, `missing_tests_audit`, `canon_consolidation_audit`) resolves its verdict by snapshotting `openspec/changes/` before and after the session and counting new directories. Its only failure detection is `outcome_to_terminal_err`, which flags a timed-out or non-zero-exit session; everything else falls through to `AuditOutcome::SpecsWritten { changes: <new dirs> }`. The terminal verdict is therefore **inferred from the absence of an artifact**, and the outcome is **initialized to the success shape**.

Three consequences, all observed or reachable: (a) a clean exit-0 session that did nothing → `SpecsWritten { changes: [] }`; (b) a session whose `exit_status` was never captured (`None`, e.g. signal kill) → the non-zero-exit branch is skipped → same; (c) a session that surveyed and concluded a real finding but could not persist it → no new dir → same. All three log a green "wrote 0 new spec(s)."

Separately, on-demand audits are triggered over the control socket (`queue_audit`) into an in-memory `pending_audit_runs: Arc<Mutex<Vec<String>>>` on the live polling-task handle. The handle carries no chat origin, and `polling_loop/mod.rs` drains the queue with `std::mem::take` at the top of each pass — before the busy-marker and workspace-init gates — with no re-queue. The operator's `audit` request and its result are fully disconnected.

The `gatekeepers-fail-closed` standard (in flight) governs both shapes: a control's "could not run" must be a distinct, surfaced, non-passing state.

## Goals / Non-Goals

**Goals:**
- An audit outcome that *cannot* be a silent pass: it initializes to "did not complete" and only an evidenced terminal verdict overwrites it.
- A positive, structured terminal verdict from the audit session (disposition + a summary of what was examined), replacing inference-from-absence.
- Every operator-triggered audit posts a terminal result to the thread it came from — clean, with-findings, or failed-to-run.
- A queued on-demand audit is not lost to a pass-skip, early-return, bound-zero, or restart.

**Non-Goals:**
- No change to which audits exist, their cadences, or the post-hoc `WritePolicy` enforcement.
- No change to the sandbox mount fix (already landed).
- Periodic audits do NOT gain a chatops post on a clean run (noise); they DO surface a failed-to-run state (see D5).

## Decisions

### D1 — `AuditOutcome` initializes to a non-passing "did not complete" state
Add `AuditOutcome::DidNotComplete { audit_type, cause: AuditFailureCause, examined_summary: Option<String> }`. The specs-writing harness binds its result to this variant first; it is overwritten only by an evidenced terminal verdict (D2). `cause` enumerates the surfaced reasons: `SessionErrored` (timeout / non-zero exit / **uncaptured exit status** — closing the `exit_status == None` hole), `NoTerminalVerdict` (session ended without declaring an outcome), `FoundButCouldNotPersist` (declared findings but wrote no valid change dir). The scheduler treats `DidNotComplete` like the existing failure path: WARN + chatops alert, cadence state NOT advanced — distinct from `NoFindings`/`SpecsWritten`.

*Alternative considered:* reuse `Result::Err` from the harness. Rejected — an `Err` is logged and dropped per-audit, carries no structured cause or summary, and cannot drive a "failed to run" chatops notification distinct from a transient internal error.

### D2 — Evidence comes from the session itself (transcript), not a new submission tool
The harness keeps the disk diff as the source of *what* was written, and adds a fail-closed guard for the empty-result case driven by the session's own captured output:
- Terminal session error (timeout / non-zero exit / **uncaptured exit status**) → `DidNotComplete{ SessionErrored }`. (Group 1, landed.)
- Clean exit, ≥1 validated change dir → `SpecsWritten(names)` carrying `examined_summary`.
- Clean exit, zero dirs, AND the session produced **no real output** (empty/whitespace transcript — a degenerate "did nothing" session) → `DidNotComplete{ NoTerminalVerdict }`.
- Clean exit, zero dirs, WITH substantive output → `SpecsWritten(vec![])` carrying `examined_summary` (an evidenced genuine no-findings run).

`examined_summary` is derived from the session's captured final answer (`AgenticRunOutcome::stdout`), trimmed and length-bounded. The on-demand completion notification (D3) surfaces it.

*Why this over a positive `submit_audit_outcome` MCP tool:* the submission-tool design is a stronger guarantee but disproportionate now — it adds an MCP-tool subsystem AND forces reworking ~15 existing harness tests (which assert `SpecsWritten` from the disk diff with no MCP submission) to stand up fake submissions, in exchange for closing a hole that is narrow post-sandbox-fix (timeout/non-zero/signal-kill are already caught; the only residual is "clean exit, did literally nothing"). The transcript guard closes that residual cheaply and yields the same examined-summary and notification UX. If false-negative tolerance later proves too low, the submission tool remains the obvious upgrade.

*Threshold choice:* only a fully empty/whitespace transcript trips `NoTerminalVerdict`, so a terse-but-real "No issues found" is never mis-flagged as a failure.

### D3 — On-demand origin threading + completion notification
`queue_audit` accepts optional `{ channel, thread_ts, request_id }`. The queue element becomes `QueuedAudit { audit_type, origin: Option<ChatOrigin> }`. After a queued audit resolves, the scheduler posts a terminal notification to `origin` via the chatops backend: success-with-`examined_summary` (and findings/PR pointer), or the D1 failed-to-run state with its `cause`. The CLI `audit run` path supplies `origin: None` and prints the terminal result to stdout instead.

### D4 — Queue durability: re-queue-on-skip + persist across restart
Two mechanisms:
1. **No drain-and-discard.** Replace the top-of-pass `mem::take` with: snapshot the queued set for this pass, and remove an entry from the shared queue only once its audit has actually run. A pass that skips (busy), returns early (init failure), or is bound out (`max_audits_per_iteration: 0`) leaves entries in place for the next iteration.
2. **Persist across restart.** Mirror the queued set to a small JSON file under the daemon state dir (atomic tempfile+rename), loaded into `pending_audit_runs` at task spawn. An entry is removed from both memory and disk only on actual run. This honors the "✓ Queued, will run" promise across a restart.

*Alternative considered:* re-queue only (no persistence). Rejected — a restart between ack and run is exactly an "initialize-to-failed-then-lose-it" hole the operator called out; persistence is small and closes it.

### D5 — Surfacing policy differs by trigger
- **On-demand** (operator asked): always post a terminal result — including a clean "0 findings, examined X."
- **Periodic** (cadence): unchanged for clean runs (stay quiet to avoid noise) and for findings (existing notification). NEW: a `DidNotComplete` periodic run posts the failed-to-run alert (a scheduled security audit that could not run must not be silent).

## Risks / Trade-offs

- **The transcript guard is coarse.** Only a fully empty/whitespace transcript trips `NoTerminalVerdict`; a session that emits output but did no real analysis still passes as no-findings. → Accepted: this is a narrow residual after the sandbox fix + Group 1 (which already catch timeout / non-zero / signal-kill). The prompt drives a real survey; the positive-submission tool remains the obvious upgrade if false-negatives prove material.
- **More chatops traffic** from on-demand completion posts. → Acceptable: the operator explicitly asked; one terminal message per request.
- **Persisted-queue staleness** (an entry for a repo removed from config). → Mitigation: load-time reconciliation against live tasks drops orphans, same as other startup marker sweeps.
- **`AuditOutcome` variant churn** touches every `match` on it. → The compiler enumerates the sites; each maps cleanly to the existing failure or no-findings arm.

## Open Questions

- Should `examined_summary` be length-bounded before it reaches chatops (it is agent free-text)? Lean yes — cap and truncate with an ellipsis, consistent with other agent-text surfaces.
- (Resolved) The verdict mechanism is the session's own captured transcript, not a new MCP submission tool — see D2.
