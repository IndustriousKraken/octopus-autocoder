# Tasks

## 1. Rich findings rendering

- [x] 1.1 Build a single rich-findings renderer that emits, per `Finding`, the existing `severity glyph + subject + anchor` line FOLLOWED BY the finding's full `body` (the divergence paragraph) when `body` is non-empty. Keep findings visually separated (blank line between findings) so the operator and the triage agent can tell them apart.
- [x] 1.2 Replace `format_audit_thread_body` (`autocoder/src/audits/mod.rs`) so the rendered thread body uses the rich renderer from 1.1 instead of the title-only form. A finding whose `body` is empty still renders its one-line title (no regression for body-less findings).
- [x] 1.3 Retain the existing 35,000-character cap on the thread body with the existing pointer-to-daemon-log tail (`… [truncated; full findings at journalctl -u autocoder | grep audit_id=<audit_id>]`); apply it to the now-richer body without altering the cap value or the tail text.

## 2. Stamped excerpt carries the rich form

- [x] 2.1 Ensure the excerpt stamped into the audit-thread state by `stamp_audit_thread_state` (`autocoder/src/audits/scheduler.rs`) is the SAME rich, body-bearing rendering used for the thread body — so `findings_excerpt` carries the divergence reasoning, not the thin title-only string.
- [x] 2.2 Apply the existing 35,000-character cap + truncation pointer to the stamped `findings_excerpt` (`autocoder/src/audits/threads.rs`) exactly as it is applied to the thread body; the cap value and tail are unchanged.

## 3. Triage receives the rich excerpt

- [x] 3.1 Confirm the triage flow (`autocoder/src/polling_loop/triage.rs`) reads the stamped `findings_excerpt` into `TriageContext.findings` so that, on `send it`, the executor receives the rich body-bearing excerpt rather than the title-only string. No re-derivation of the audit's analysis is required by the downstream agent.

## 4. Tests

- [x] 4.1 A `drift_audit` finding with a non-empty `body` renders that body in BOTH the posted thread body AND the stamped `findings_excerpt` (assert the divergence text is present, not just the title).
- [x] 4.2 A rendered body exceeding 35,000 characters is truncated to the cap with the existing pointer-to-daemon-log tail — for both the thread body and the stamped excerpt.
- [x] 4.3 On `send it`, `TriageContext.findings` equals the rich body-bearing excerpt (assert the divergence text is present), NOT the title-only form.
- [x] 4.4 A finding whose `body` is empty renders its one-line title with no regression (no stray blank-body artifact) in both the thread body and the excerpt.
