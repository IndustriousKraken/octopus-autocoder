## MODIFIED Requirements

### Requirement: Inbound listener routes `send it` to `BrownfieldBatchAction` when posted in a brownfield-survey thread
The existing `send it` verb (per the canonical `audit-reply-acts` mechanism; the audit-thread handler itself is defined by `send it verb in an audit thread` and is NOT redefined here) SHALL gain a SECOND recognized context: when posted as a reply inside a brownfield-survey lifecycle thread, the listener SHALL submit a `BrownfieldBatchAction { survey_request_id, channel, thread_ts }` INSTEAD OF the canonical audit-triage action.

At parse time, the listener SHALL look up the parent thread's `ts` against FOUR sets of per-workspace state:

1. Audit-thread set — the existing canonical audit-thread state (its routing is defined by `send it verb in an audit thread`, not redefined here).
2. Brownfield-survey set — `BrownfieldSurveyState.thread_ts` values across the workspace's stored surveys.
3. Issue-candidate set — the `thread_ts` values recorded on stored issue-candidate states (per `Inbound listener routes send it to issue-candidate promotion when posted in an issue-candidate thread`).
4. Revision-thread set — the `thread_ts` values recorded on stored `RevisionThreadState` entries (per `Inbound listener routes send it to the spec-revision executor when posted in a revision thread`).

The four-set lookup applies ONLY to a `send it` posted as a thread reply (a non-empty parent `thread_ts`). A `send it` at top level (not a thread reply) is NOT a thread context at all — it parses as the unknown-verb fallback (the `?` reaction), per `send it verb in an audit thread`. For a thread reply: if the parent thread matches an audit thread, the audit-thread handler fires (as defined by `send it verb in an audit thread`); if it matches a brownfield-survey thread, the new `BrownfieldBatchAction` is submitted; if it matches an issue-candidate thread, the issue-candidate promotion fires; if it matches a revision thread, the spec-revision executor fires. If a thread reply matches NONE of the four tracked sets, the listener posts the untracked-thread refusal, whose text names all four valid contexts (audit thread, brownfield-survey thread, issue-candidate thread, spec-revision thread).

#### Scenario: Send-it in an audit thread (regression check)
- **WHEN** an operator posts `@<bot> send it` as a reply inside a tracked audit thread
- **THEN** the existing canonical audit-triage action is submitted, as defined by `send it verb in an audit thread` (this requirement does not redefine the audit-thread routing)
- **AND** the brownfield-survey, issue-candidate, and revision routing do not apply

#### Scenario: Send-it in a brownfield-survey thread
- **WHEN** an operator posts `@<bot> send it` as a reply inside a brownfield-survey lifecycle thread
- **AND** the survey's `BrownfieldSurveyState` exists AND its `status` is `Pending` (i.e., not already in progress OR completed)
- **THEN** a `BrownfieldBatchAction { survey_request_id, channel, thread_ts }` is submitted
- **AND** the polling iteration's batch handler begins draining the survey's items one per iteration

#### Scenario: Send-it in a survey thread when batch already running
- **WHEN** the survey's `status` is already `InProgress` OR `Completed`
- **THEN** the bot replies `✗ send it: a brownfield batch is already <in progress | completed> for survey <request_id>.`
- **AND** no duplicate `BrownfieldBatchAction` is submitted

#### Scenario: Send-it in a revision thread fires the revision executor
- **WHEN** an operator posts `@<bot> send it` as a reply whose `thread_ts` matches a stored `RevisionThreadState`
- **THEN** the spec-revision executor fires for that change (per the revision-executor requirement)
- **AND** neither the audit-triage, brownfield-batch, nor issue-candidate handler is invoked

#### Scenario: Send-it in an unrecognized thread is refused; at top level it is the `?` fallback
- **WHEN** an operator posts `@<bot> send it` as a reply in a thread that matches none of the four tracked contexts (audit, survey, issue-candidate, revision)
- **THEN** the bot replies with the untracked-thread rejection message naming the four valid contexts AND no action is submitted
- **WHEN** an operator posts `@<bot> send it` at top level (not a thread reply)
- **THEN** it parses as the unknown-verb fallback (the `?` reaction, per `send it verb in an audit thread`), NOT the untracked-thread refusal
- **AND** no action is submitted

### Requirement: Inbound listener recognizes the `clear-survey` verb
The inbound listener SHALL recognize `@<bot> clear-survey <repo-substring>` as an operator-recovery verb (alongside `clear-perma-stuck`, `clear-revision`, `clear-scout`, `wipe-workspace`, etc.). The listener SHALL parse the repo-substring per the existing match rule AND submit `ClearSurveyAction { repo_url, channel, thread_ts }`.

#### Scenario: Clear-survey happy path
- **WHEN** an operator posts `@<bot> clear-survey myrepo` AND the repo resolves uniquely
- **THEN** a `ClearSurveyAction` is submitted
- **AND** the polling iteration deletes ALL `BrownfieldSurveyState` files for that repo AND replies with the count

#### Scenario: Clear-survey with no surveys present
- **WHEN** an operator posts `@<bot> clear-survey myrepo` AND no `BrownfieldSurveyState` files exist for that repo
- **THEN** the bot replies `✓ Cleared 0 brownfield-survey(s) for <repo_url>.` (idempotent)

#### Scenario: Help verb lists the new verbs
- **WHEN** an operator posts `@<bot> help`
- **THEN** the help output lists `brownfield-survey` (chat-driven workflow) AND `clear-survey` (operator recovery)
- **AND** `send it`'s help text names all four valid thread contexts (audit, brownfield-survey, issue-candidate, AND spec-revision)

## ADDED Requirements

### Requirement: Inbound listener routes `send it` to the spec-revision executor when posted in a revision thread
The `send it` verb SHALL recognize a FOURTH thread context — a spec-revision thread — alongside the audit, brownfield-survey, and issue-candidate contexts. When `@<bot> send it` is posted as a reply whose `thread_ts` matches a stored `RevisionThreadState`, the dispatcher SHALL run the spec-revision executor for that change INSTEAD OF the other contexts' handlers. A `thread_ts` resolves to at most one record across the four sets.

A reply in a revision thread that is an `@<bot>` mention but is NOT the `send it` verb SHALL route to the read-only revision advisor (per the revision-advisor requirement), so the operator can discuss the revision before triggering it. A bare reply with no mention is not seen by the listener (consistent with the other contexts).

#### Scenario: Send-it in a revision thread runs the revision executor
- **WHEN** an operator posts `@<bot> send it` as a reply whose `thread_ts` matches a stored `RevisionThreadState`
- **THEN** the dispatcher runs the spec-revision executor for that change
- **AND** the audit, survey, and issue-candidate lookups are not acted on

#### Scenario: A discussion reply routes to the advisor
- **WHEN** an operator posts an `@<bot>` reply in a revision thread that is not the `send it` verb
- **THEN** the dispatcher routes it to the read-only revision advisor
- **AND** no spec file is written by that reply

#### Scenario: Send-it in an untracked thread names four contexts
- **WHEN** an operator posts `@<bot> send it` in a thread matching no audit, survey, issue-candidate, OR revision record
- **THEN** the bot posts the untracked-thread refusal naming the four valid contexts (audit thread, brownfield-survey thread, issue-candidate thread, spec-revision thread)
- **AND** no action is submitted
