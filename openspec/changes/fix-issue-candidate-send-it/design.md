## Context

Issue ingestion drafts a candidate, persists `CandidateState` keyed by
`candidate_id` (repo + reported issue number) under
`<state_root>/issue-candidates/<id>.json`, and posts a chatops notification.
`promote_candidate(workspace, state_root, state)` writes `issues/<slug>/`
(`issue.md`, `tasks.md`, plus `report-body.md` for public origin) and flips
the candidate's status to `Promoted`; writing the unit IS the queue, since the
issues-lane walker picks up any ready `issues/<slug>/`. Both `promote_candidate`
and `read_candidate` are implemented and unit-tested but unreachable.

The `send it` verb fires only on an `@<bot>` `app_mention` carrying a non-empty
`thread_ts`. The dispatcher (`dispatch_send_it_on_audit`) resolves the thread
against the audit-thread set first, then the brownfield-survey set
(`try_send_it_on_survey`), and refuses with `SEND_IT_REFUSE_UNTRACKED` when
neither matches. The survey fallback is the model: scan per-record state for a
`thread_ts` match, branch on the record's status, and on a fresh match submit a
control-socket action carrying `channel`/`thread_ts`.

The candidate store lives in the daemon state root. The dispatcher already
holds that root as `self.audit_thread_state_dir` (audit-thread reads append
`audit-threads/`; candidate reads append `issue-candidates/`), so the candidate
set is reachable from the same field without new plumbing.

## Goals / Non-Goals

**Goals:**
- A maintainer `@<bot> send it` in an issue-candidate thread promotes the
  candidate: writes `issues/<slug>/`, queues it, marks the candidate promoted,
  and confirms in-thread — the behavior the canon already requires.
- The candidate notification carries a matchable thread and a correct
  instruction.
- Reuse the established send-it dispatch shape (audit / survey), not a new one.

**Non-Goals:**
- No change to triage, classification, dedup, routing, or the quarantine of
  public report bodies.
- No change to the issues-lane walker, precedence, or archival.
- No new lifecycle states for a candidate beyond the existing `Posted` →
  `Promoted` transition.

## Decisions

### D1 — Capture `thread_ts`/`channel` on the candidate at post time
`post_candidate` posts via the thread-returning notification path
(`post_notification_with_thread`-equivalent) and records the returned
`thread_ts` and the `channel` on `CandidateState`. Both fields are optional and
`#[serde(default)]` so pre-existing candidate files deserialize unchanged; a
candidate with no captured thread (degraded post, older record) is simply not
matchable by a reply, which is acceptable graceful degradation. The store stays
keyed by `candidate_id`; the dispatcher matches a reply by scanning the
`issue-candidates/` directory for a record whose `thread_ts` equals the reply's
`thread_ts`, exactly as the survey fallback scans survey state.

### D2 — Promote synchronously via a control-socket action (not a deferred queue)
Promotion is a pure filesystem operation — write `issues/<slug>/`, flip the
state JSON — with no executor run. The dispatcher submits a
`promote_issue_candidate { url, candidate_id, channel, thread_ts }` action; the
handler resolves the workspace (`workspace::resolve_path(&paths, &repo)`),
loads the candidate, calls `promote_candidate`, and returns the written path on
success. This mirrors `queue_clear_survey`, which also does synchronous
filesystem work in the handler and returns a count. The operator gets immediate
confirmation rather than the survey/audit path's "runs next iteration"
deferral.

Alternative considered: a per-repo `pending_issue_promotions` queue drained by
the polling loop, mirroring `queue_brownfield_batch_request`. Rejected: it adds
a queue, a drain site, and an iteration of latency for an operation that needs
neither (no executor, no merge-conflict serialization — writing the unit is the
only side effect, and the lane walker already serializes downstream work).

### D3 — Dispatcher branch on candidate status
The issue-candidate lookup runs as the THIRD context, after audit and survey
lookups miss (preserving their precedence; a `thread_ts` is unique to one
record across the three sets). On a match:
- `Posted` → submit `promote_issue_candidate`; on `ok` reply
  `✓ Promoted issue candidate \`<slug>\` — wrote issues/<slug>/ AND queued it
  for the issues lane.`; on a handler error reply `✗ send it: could not
  promote candidate: <error>`.
- `Promoted` → reply `✗ send it: issue candidate \`<slug>\` is already
  promoted. No new action taken.` and submit nothing (idempotent).

A non-matching thread still falls through to the untracked refusal, whose text
is updated to name all three contexts.

### D4 — Corrected notification instruction
The candidate notification instructs `@<bot> send it` (the form that actually
fires), matching the audit and survey notifications. The "nothing is queued
until you do" clause is retained.

## Risks / Trade-offs

- **Optional fields on `CandidateState`.** Adding `thread_ts`/`channel` widens
  a serialized state struct. Mitigated by `#[serde(default)]` (back-compat
  reads) and by treating an absent thread as non-matchable rather than an
  error.
- **Directory scan per `send it`.** Matching a reply scans `issue-candidates/`.
  The set is small (one file per recently reported issue) and the scan runs
  only on the rare `send it` reply, after the audit and survey lookups miss —
  the same cost profile as the existing survey scan.
- **Synchronous promotion holds the control-socket handler for a filesystem
  write.** The write is bounded (three small files) and matches the existing
  `queue_clear_survey` precedent; no executor or network call is involved.
