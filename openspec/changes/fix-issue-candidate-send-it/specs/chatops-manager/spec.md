## MODIFIED Requirements

### Requirement: Inbound listener routes `send it` to `BrownfieldBatchAction` when posted in a brownfield-survey thread
The existing `send it` verb (per the canonical `audit-reply-acts` mechanism — unchanged for audit threads) SHALL gain a SECOND recognized context: when posted as a reply inside a brownfield-survey lifecycle thread, the listener SHALL submit a `BrownfieldBatchAction { survey_request_id, channel, thread_ts }` INSTEAD OF the canonical audit-triage action.

At parse time, the listener SHALL look up the parent thread's `ts` against THREE sets of per-workspace state:

1. Audit-thread set — existing canonical mechanism, unchanged.
2. Brownfield-survey set — `BrownfieldSurveyState.thread_ts` values across the workspace's stored surveys.
3. Issue-candidate set — the `thread_ts` values recorded on stored issue-candidate states (per `Inbound listener routes send it to issue-candidate promotion when posted in an issue-candidate thread`).

If the parent thread matches an audit thread, the existing canonical handler fires. If it matches a brownfield-survey thread, the new `BrownfieldBatchAction` is submitted. If it matches an issue-candidate thread, the issue-candidate promotion fires. If it matches NONE of the three, the listener posts the untracked-thread refusal, whose text names all three valid contexts (audit thread, brownfield-survey thread, issue-candidate thread).

#### Scenario: Send-it in an audit thread (regression check)
- **WHEN** an operator posts `@<bot> send it` as a reply inside an audit thread (per the canonical mechanism)
- **THEN** the existing canonical audit-triage action is submitted
- **AND** behavior is unchanged from the pre-`a29` flow

#### Scenario: Send-it in a brownfield-survey thread
- **WHEN** an operator posts `@<bot> send it` as a reply inside a brownfield-survey lifecycle thread
- **AND** the survey's `BrownfieldSurveyState` exists AND its `status` is `Pending` (i.e., not already in progress OR completed)
- **THEN** a `BrownfieldBatchAction { survey_request_id, channel, thread_ts }` is submitted
- **AND** the polling iteration's batch handler begins draining the survey's items one per iteration

#### Scenario: Send-it in a survey thread when batch already running
- **WHEN** the survey's `status` is already `InProgress` OR `Completed`
- **THEN** the bot replies `✗ send it: a brownfield batch is already <in progress | completed> for survey <request_id>.`
- **AND** no duplicate `BrownfieldBatchAction` is submitted

#### Scenario: Send-it outside any known thread context
- **WHEN** an operator posts `@<bot> send it` at top level OR in an unrecognized thread (not audit, not survey, not issue-candidate)
- **THEN** the bot replies with the rejection message naming the three valid contexts (audit thread, brownfield-survey thread, issue-candidate thread)
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
- **AND** `send it`'s help text names all three valid thread contexts (audit, brownfield-survey, AND issue-candidate)

## ADDED Requirements

### Requirement: Inbound listener routes `send it` to issue-candidate promotion when posted in an issue-candidate thread

The `send it` verb SHALL recognize a THIRD thread context — an issue-candidate
thread — alongside the audit-thread and brownfield-survey contexts. When
`@<bot> send it` is posted as a reply whose `thread_ts` matches a posted issue-
candidate's recorded thread, the dispatcher SHALL promote that candidate
INSTEAD OF the audit-triage or brownfield-batch actions.

At parse time, after the audit-thread lookup AND the brownfield-survey lookup
both miss, the dispatcher SHALL look up the reply's `thread_ts` against the
issue-candidate set — the `thread_ts` values recorded on stored candidate
states. A `thread_ts` resolves to at most one record across the three sets.

On a match whose candidate `status` is the posted (not-yet-promoted) state, the
dispatcher SHALL submit the promotion control-socket action carrying the
candidate identity AND the originating `channel`/`thread_ts`, AND reply with the
write-and-queue confirmation. On a match whose candidate is already promoted,
the dispatcher SHALL reply that the candidate is already promoted AND submit
nothing. When the reply matches NONE of the three sets, the dispatcher SHALL
post the untracked-thread refusal, whose text names all three valid contexts
(audit thread, brownfield-survey thread, issue-candidate thread).

#### Scenario: Send-it in an audit thread is unchanged

- **WHEN** an operator posts `@<bot> send it` as a reply inside an audit thread
- **THEN** the existing audit-triage action is submitted
- **AND** the issue-candidate lookup is not consulted

#### Scenario: Send-it promotes a posted issue candidate

- **WHEN** an operator posts `@<bot> send it` as a reply whose `thread_ts`
  matches a stored issue candidate whose status is the posted (not-yet-promoted)
  state
- **THEN** the dispatcher submits the promotion control-socket action carrying
  the candidate identity AND the originating `channel`/`thread_ts`
- **AND** on success the bot replies that it wrote `issues/<slug>/` AND queued
  it for the issues lane

#### Scenario: Send-it on an already-promoted candidate takes no new action

- **WHEN** an operator posts `@<bot> send it` in an issue-candidate thread whose
  candidate is already promoted
- **THEN** the bot replies that the candidate is already promoted
- **AND** no promotion action is submitted

#### Scenario: Send-it outside any known thread context names all three contexts

- **WHEN** an operator posts `@<bot> send it` at top level OR in a thread that
  matches no audit, survey, OR issue-candidate record
- **THEN** the bot posts the untracked-thread refusal naming the three valid
  contexts (audit thread, brownfield-survey thread, issue-candidate thread)
- **AND** no action is submitted
