# Audit triage carries each finding's full body, not just its one-line title

## Why

Every audit finding (`Finding { severity, subject, body, anchor }`) carries a rich
per-finding `body`. For `drift_audit` that body is a "divergence" paragraph that
states what the spec requires, what the code does, and why the gap matters — the
drift-audit prompt (`prompts/drift-audit.md`) asks the agent for exactly this. That
body is the most valuable thing the audit computed, and it is DROPPED on the live
path at two layers:

1. `format_audit_thread_body` renders only `severity glyph + subject + anchor` per
   finding — the `body` field is never emitted into the Slack thread. The operator
   reading the audit notification sees only `[capability] title (file:line)`.
2. More consequentially, the audit-thread state is stamped with that SAME truncated
   thread body, which becomes the state's `findings_excerpt`, which is EXACTLY what
   the triage executor receives as `TriageContext.findings` when an operator runs
   `send it` on the audit thread. So the downstream agent that acts on a finding
   sees only the one-line title — not the divergence reasoning the audit already
   produced — and must re-derive what the spec requires and what the code does,
   risking hallucination of the very analysis that was already in hand.

The full `body` IS preserved in the on-disk audit log, but nothing on the live
notification/triage path reads that log. The fix is to stop discarding the body
before it is shown and before it is stamped.

## What Changes

- The rendered audit thread body SHALL include each finding's full `body` (the
  divergence paragraph), not merely `severity glyph + subject + anchor`. The
  existing 35,000-character thread-body cap with the pointer-to-daemon-log tail is
  retained unchanged and applies to the now-richer body.
- The audit-thread state's stamped excerpt (`findings_excerpt`) SHALL carry the
  same rich, body-bearing rendering, so the value handed to the triage executor as
  `TriageContext.findings` on `send it` is the rich form, not the thin one. The
  same 35,000-character cap and truncation pointer apply to the stamped excerpt.

## Impact

- Affected specs: `chatops-manager` (the rendered audit thread body), `orchestrator-cli`
  (the stamped excerpt handed to the triage executor as `TriageContext.findings`).
- Affected code: `format_audit_thread_body` (`autocoder/src/audits/mod.rs`), the
  excerpt stamped by `stamp_audit_thread_state` (`autocoder/src/audits/scheduler.rs`),
  the `findings_excerpt` capping in `autocoder/src/audits/threads.rs`, and the triage
  context assembly in `autocoder/src/polling_loop/triage.rs`.
- Coexists with — does NOT restate or alter — the existing 35,000-character
  thread-body truncation requirement and the existing threaded-notification path.
- Independent change; it touches no requirement another in-flight change modifies.
