## MODIFIED Requirements

### Requirement: `send it` verb in an audit thread schedules a triage executor run
The chatops listener SHALL recognize `@<bot> send it` (case-insensitive on `send it`) as the `SendItOnAudit` command ONLY when the message arrives with a non-empty `thread_ts` AND the `thread_ts` matches a tracked audit-thread state with `status: Open` OR `status: TriageFailed`. Same text outside a thread SHALL parse as the unknown-verb fallback (existing `?` reaction). When recognized, the dispatcher SHALL submit a `trigger_audit_action` control-socket action AND flip the audit-thread state's `status` to `TriagePending`. The next polling iteration drains the triage queue and runs the executor in triage mode.

#### Scenario: Send-it in tracked, open audit thread schedules triage
- **WHEN** an operator posts `@<bot> send it` as a thread reply where `thread_ts` matches an `AuditThreadState` with `status: Open` AND `posted_at` within the last 7 days
- **THEN** the dispatcher submits `trigger_audit_action` with the `thread_ts`
- **AND** the state file's `status` is updated to `TriagePending`
- **AND** the bot replies in the thread `✓ Triage scheduled for <audit_type> on <repo_url>. The next polling iteration will run it (~Nm).`

#### Scenario: Send-it in untracked thread is politely refused
- **WHEN** an operator posts `@<bot> send it` in a thread that matches no audit-thread, brownfield-survey, issue-candidate, OR spec-revision state
- **THEN** the bot replies `✗ This reply is in a thread autocoder is not tracking. The \`send it\` verb only acts in an audit-notification, brownfield-survey, issue-candidate, or spec-revision thread.`
- **AND** no control-socket action is submitted

#### Scenario: Send-it on stale audit thread is politely refused
- **WHEN** an operator posts `@<bot> send it` in a tracked thread whose `posted_at` is older than 7 days
- **THEN** the bot replies `✗ This audit's findings are too old to act on (>7d). Re-run the audit via @<bot> audit <type> <repo>.`
- **AND** the state file remains unchanged (the prune-stale-entries pass will eventually remove it)

#### Scenario: Send-it on already-acted thread is politely refused
- **WHEN** an operator posts `@<bot> send it` in a thread with `status: Acted` OR `status: TriagePending`
- **THEN** the bot replies `✗ This audit thread is already <status>. No new action taken.`
- **AND** no new triage is scheduled

#### Scenario: Send-it on TriageFailed thread re-attempts triage
- **WHEN** an operator posts `@<bot> send it` in a thread with `status: TriageFailed`
- **THEN** the dispatcher treats the request like the Open case (triage re-scheduled)
- **AND** the state's `status` is reset to `TriagePending` for the new attempt

## ADDED Requirements

### Requirement: Spec-revision contradiction alert is a tracked, discussable thread
When autocoder posts a `SpecNeedsRevision` chatops alert for a CONTRADICTION marker — a `.needs-spec-revision.json` whose `unimplementable_tasks` array is empty AND whose `gate_error` is empty (a `[in]` / `[canon]` semantic finding, NOT the executor's unimplementable-tasks flag NOR a gate-error hold) — autocoder SHALL capture the posted message's `channel` AND `thread_ts` in a `RevisionThreadState` keyed to the repository AND change slug, so a later reply can be matched to the change. The alert body SHALL advertise that the operator may reply in the thread to discuss the revision OR post `@<bot> send it` to have the change revised and a PR opened. A degraded post that returns no `thread_ts` SHALL still write the marker AND alert but SHALL NOT record a `RevisionThreadState` (the alert is simply not reply-matchable — graceful degradation, never an error). The `clear-revision` verb remains the unchanged manual escape.

#### Scenario: A contradiction alert is tracked and advertises the thread
- **WHEN** autocoder posts the `SpecNeedsRevision` alert for a marker with empty `unimplementable_tasks` AND empty `gate_error`
- **THEN** it records a `RevisionThreadState` carrying the alert's `channel`, `thread_ts`, repository, AND change slug
- **AND** the alert body states that a reply discusses the revision AND that `@<bot> send it` revises the change and opens a PR

#### Scenario: An unimplementable-tasks alert is not tracked as a revision thread
- **WHEN** the marker's `unimplementable_tasks` is non-empty (the executor's flag-and-halt case)
- **THEN** no `RevisionThreadState` is recorded for it AND the alert does not advertise the revision thread
- **AND** that marker keeps its existing operator-authored flow (the agent flags; the operator edits `tasks.md`)

#### Scenario: A degraded post is not reply-matchable
- **WHEN** the alert post returns no `thread_ts`
- **THEN** the marker AND alert are still produced
- **AND** no `RevisionThreadState` is recorded (the thread is simply not reply-matchable)

### Requirement: Revision advisor discusses a flagged change read-only
A non-`send it` `@<bot>` reply whose `thread_ts` matches a `RevisionThreadState` SHALL run a read-only agentic session — the revision advisor — reconstructed from the flagged change's spec deltas, the relevant canonical specs, the marker's contradiction narrative, AND the thread transcript so far. The advisor SHALL answer the operator's question — typically whether to align the change to canon's existing vocabulary OR to MODIFY the contradicted canonical requirement, and how — AND SHALL write nothing to the workspace. The session SHALL be stateless: each reply reconstructs the advisor from the on-disk artifacts AND the thread transcript; no agent session is persisted between replies.

#### Scenario: The advisor answers from change, canon, and transcript, writing nothing
- **WHEN** an operator posts a discussion reply in a revision thread
- **THEN** a read-only session reads the change deltas, the relevant canon, the marker's contradiction, AND the thread so far, AND replies with its assessment
- **AND** no file in the workspace is modified by the reply

#### Scenario: Multiple rounds reconstruct from the growing transcript
- **WHEN** the operator posts a second discussion reply in the same thread
- **THEN** the advisor is reconstructed afresh, with the earlier exchange included via the thread transcript
- **AND** no agent session was held between the two replies

### Requirement: Send it in a revision thread runs the spec-revision executor
`@<bot> send it` in a revision thread SHALL run the spec-revision executor: a write-scoped agentic session that edits the flagged change's spec deltas along the direction the thread converged on, then re-runs the `[in]` AND `[canon]` checks against the revised change before producing any output. On a clean re-gate the executor SHALL open a PR carrying the change's spec-delta revision AND report the PR link in the thread; on a re-gate that still finds a contradiction the executor SHALL open NO PR AND report the remaining contradiction in the thread (the operator may discuss further AND `send it` again). The executor SHALL NOT commit a spec revision to the base branch outside the PR — human review of the PR is the merge gate — AND SHALL NOT auto-edit a `tasks.md` to dodge the executor's unimplementable-tasks flag (that separate marker keeps its operator-authored invariant). The revision is to the change's spec deltas to achieve canon-consistency, performed under operator direction (the thread) AND human review (the PR).

#### Scenario: Send-it revises, re-gates clean, and opens a PR
- **WHEN** an operator `send it`s a revision thread AND the executor's revision passes the re-run `[in]` and `[canon]` checks
- **THEN** the executor opens a PR carrying the change's spec-delta revision
- **AND** it reports the PR link in the thread
- **AND** it does not merge the PR or commit the revision to the base branch outside the PR

#### Scenario: A revision that still contradicts opens no PR and reports back
- **WHEN** the executor's revision still fails the re-run `[in]` or `[canon]` check
- **THEN** no PR is opened
- **AND** the remaining contradiction is reported in the thread so the operator can discuss further and `send it` again

#### Scenario: The unimplementable-tasks invariant is preserved
- **WHEN** the spec-revision executor runs
- **THEN** it revises the change's spec deltas to resolve the contradiction
- **AND** it does NOT auto-edit a `tasks.md` to make an unimplementable-tasks flag pass (that marker's operator-authored flow is untouched)
