# Tasks

## 1. Hold the busy marker across a revision

- [x] 1.1 In `process_pending_revision_execute` / `run_revision_execute` (`autocoder/src/polling/revision_session.rs`), acquire the per-repo busy marker before the converge loop begins, recording the change slug being revised (use the same marker mechanism `pass.rs` uses, so the existing stale-detection/recovery and the status `currently:` branching apply unchanged).
- [x] 1.2 Release the marker on EVERY exit path of the revision: clean re-gate → PR opened, budget exhausted with a contradiction remaining, scope/edit violation discarded, gate could-not-run, transcript-unreadable refusal, and any error return. Prefer a guard/drop so a panic or early return cannot leak the marker.
- [x] 1.3 Confirm no double-acquire deadlock with the same iteration's later `execute_one_pass` (the revision drains and completes before the pass runs in `loop_drive`); the revision must have released the marker before the pass attempts to acquire it.

## 2. Tests

- [x] 2.1 While a revision is in flight, the per-repo busy marker is stamped with the change slug, so `format_status_reply`'s `currently:` line reads `working on <slug> (started <age> ago)` and NOT `idle`. Assert on the marker state + rendered `currently:` value.
- [x] 2.2 After the revision completes (each terminal path: PR opened, budget exhausted, scope violation, gate could-not-run), the busy marker is released — a subsequent status does not show a lingering revision marker.
- [x] 2.3 A normal pass cannot acquire the marker for that repo while a revision holds it (per the per-repo concurrency rule).
