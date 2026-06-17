# Tasks

## 1. Track the revision thread

- [x] 1.1 When posting the `SpecNeedsRevision` alert for a CONTRADICTION marker (empty `unimplementable_tasks` AND empty `gate_error`), post via the thread-returning notification path and capture the returned `channel`/`thread_ts`. Persist a `RevisionThreadState { repo_url, change_slug, channel, thread_ts, status }` keyed to repo + change (mirror `AuditThreadState`'s storage). A degraded post with no `thread_ts` writes the marker/alert but no `RevisionThreadState`.
- [x] 1.2 Distinguish the marker kind: only the contradiction marker (empty `unimplementable_tasks`, empty `gate_error`) is tracked + advertised. The executor's unimplementable-tasks marker and the gate-error hold marker are NOT tracked as revision threads.
- [x] 1.3 The contradiction alert body advertises: reply to discuss, or `@<bot> send it` to revise and open a PR.
- [x] 1.4 Add a lookup returning the `RevisionThreadState` whose `thread_ts` matches a reply's `thread_ts` (mirror the audit/survey/issue-candidate scans), for the dispatcher's fourth-context check.

## 2. Dispatcher: fourth `send it` context + advisor routing

- [x] 2.1 Extend the `send it` dispatcher to look up a reply's `thread_ts` against the revision-thread set after the audit, survey, and issue-candidate sets. A `send it` match runs the revision executor; a non-`send it` `@<bot>` reply in a revision thread routes to the advisor.
- [x] 2.2 Update the untracked-thread refusal text to name all four contexts (audit-notification, brownfield-survey, issue-candidate, spec-revision).
- [x] 2.3 Update `send it`'s `help` text to name all four valid thread contexts.

## 3. Revision advisor (read-only)

- [ ] 3.1 Add a `revision_advise` control-socket action that runs a read-only agentic session reconstructed from the change's spec deltas, the relevant canon, the marker's contradiction narrative, AND the thread transcript so far, and replies in the thread. It writes nothing. Stateless — no session is persisted; each reply rebuilds context.
- [ ] 3.2 Pass the thread transcript into the advisor's prompt as the conversation history so multi-round discussion works without a held session.

## 4. Revision executor (write + re-gate + PR)

- [ ] 4.1 Add a `revision_execute` control-socket action: a write-scoped agentic session that edits the flagged change's spec deltas along the discussed direction, scoped to that change's `openspec/changes/<slug>/` directory.
- [ ] 4.2 Re-run the `[in]` and `[canon]` checks (a02's invocation) against the revised change. On clean, open a PR carrying the spec-delta revision via the existing PR-open helpers and report the PR link in the thread. On a remaining contradiction, open no PR and report the contradiction in the thread.
- [ ] 4.3 Do NOT commit the revision to the base branch outside the PR, and do NOT auto-edit `tasks.md` to dodge the executor's unimplementable-tasks flag. Flip the `RevisionThreadState.status` to an acted state on a successful PR so a repeat `send it` is handled gracefully.

## 5. Tests

- [x] 5.1 A contradiction marker's alert records a `RevisionThreadState` (channel/thread_ts/repo/slug) and advertises the thread; an unimplementable-tasks marker records none and keeps its operator-authored flow (assert behavior/state, not message wording).
- [x] 5.2 Dispatcher: a `send it` matching a `RevisionThreadState` runs `revision_execute`; a non-`send it` reply runs `revision_advise`; a reply matching no record returns the four-context refusal; audit/survey/issue-candidate matches still take precedence (regression).
- [ ] 5.3 The advisor writes nothing to the workspace and is reconstructed per reply (no persisted session); a second reply includes the first exchange via the transcript.
- [ ] 5.4 The executor revises the change's spec deltas, re-gates, and opens a PR on a clean re-gate; on a still-failing re-gate it opens no PR and reports back; it never merges or commits outside the PR; it does not edit `tasks.md` to clear an unimplementable-tasks flag.

## 6. Docs

- [x] 6.1 Update `docs/CHATOPS.md` (and `docs/OPERATIONS.md` revision-marker section) to document the spec-revision thread: a contradiction marker can be discussed in its alert thread and revised via `send it`, which opens a PR; the unimplementable-tasks marker keeps its operator-authored flow; `clear-revision` remains the manual escape.
