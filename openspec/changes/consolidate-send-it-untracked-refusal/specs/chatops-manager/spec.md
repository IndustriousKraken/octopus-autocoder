## ADDED Requirements

### Requirement: Inbound listener dispatches `send it` by thread context AND refuses untracked threads
This requirement is the SINGLE canonical owner of the `send it` thread-context dispatch order AND the untracked-thread refusal. The per-context routing requirements (audit, brownfield-survey, issue-candidate, AND spec-revision) define ONLY their own positive branch AND cite this requirement for the lookup order AND the refusal; they SHALL NOT restate the four-set lookup OR the untracked-thread refusal text.

When `@<bot> send it` (case-insensitive on `send it`) arrives as a thread reply (a non-empty parent `thread_ts`), the listener SHALL look the parent `thread_ts` up against FOUR per-workspace sets, in this order, matching AT MOST ONE record across all four:

1. Audit-thread set (per `send it verb in an audit thread schedules a triage executor run`).
2. Brownfield-survey set — `BrownfieldSurveyState.thread_ts` values (per `Inbound listener routes send it to BrownfieldBatchAction when posted in a brownfield-survey thread`).
3. Issue-candidate set — the `thread_ts` values recorded on stored issue-candidate states (per `Inbound listener routes send it to issue-candidate promotion when posted in an issue-candidate thread`).
4. Revision-thread set — the `thread_ts` values recorded on stored `RevisionThreadState` entries (per `Inbound listener routes send it to the spec-revision executor when posted in a revision thread`).

On a match, the corresponding context's handler fires, as defined by that context's requirement. If the reply matches NONE of the four tracked sets, the listener SHALL post the untracked-thread refusal `✗ This reply is in a thread autocoder is not tracking. The \`send it\` verb only acts in an audit-notification, brownfield-survey, issue-candidate, or spec-revision thread.` AND submit no control-socket action.

A `send it` at TOP LEVEL (no parent `thread_ts`, not a thread reply) is NOT a thread context: it parses as the unknown-verb fallback (the `?` reaction, per `Unrecognised verbs get a \`?\` reaction`), NOT the untracked-thread refusal.

#### Scenario: Lookup walks the four sets in order, matching at most one
- **WHEN** an operator posts `@<bot> send it` as a thread reply
- **THEN** the listener looks the parent `thread_ts` up against the audit, brownfield-survey, issue-candidate, AND revision sets in that order
- **AND** at most one record matches across the four sets, AND that context's handler fires

#### Scenario: Untracked thread reply is politely refused
- **WHEN** an operator posts `@<bot> send it` as a reply in a thread that matches none of the four tracked sets (audit, brownfield-survey, issue-candidate, revision)
- **THEN** the bot replies `✗ This reply is in a thread autocoder is not tracking. The \`send it\` verb only acts in an audit-notification, brownfield-survey, issue-candidate, or spec-revision thread.`
- **AND** no control-socket action is submitted

#### Scenario: Top-level send it is the `?` fallback, not the refusal
- **WHEN** an operator posts `@<bot> send it` at top level (no parent `thread_ts`, not a thread reply)
- **THEN** it parses as the unknown-verb fallback (the `?` reaction)
- **AND** it is NOT the untracked-thread refusal AND no action is submitted

## MODIFIED Requirements

### Requirement: Inbound listener routes `send it` to `BrownfieldBatchAction` when posted in a brownfield-survey thread
The existing `send it` verb (its full thread-context dispatch order AND the untracked-thread refusal are defined by `Inbound listener dispatches send it by thread context AND refuses untracked threads`, NOT restated here; the audit-thread handler itself is defined by `send it verb in an audit thread schedules a triage executor run`) SHALL recognize a brownfield-survey lifecycle thread as one of its contexts: when posted as a reply whose parent `thread_ts` matches a `BrownfieldSurveyState.thread_ts`, the listener SHALL submit a `BrownfieldBatchAction { survey_request_id, channel, thread_ts }` INSTEAD OF the other contexts' handlers.

#### Scenario: Send-it in a brownfield-survey thread
- **WHEN** an operator posts `@<bot> send it` as a reply inside a brownfield-survey lifecycle thread
- **AND** the survey's `BrownfieldSurveyState` exists AND its `status` is `Pending` (i.e., not already in progress OR completed)
- **THEN** a `BrownfieldBatchAction { survey_request_id, channel, thread_ts }` is submitted
- **AND** the polling iteration's batch handler begins draining the survey's items one per iteration

#### Scenario: Send-it in a survey thread when batch already running
- **WHEN** the survey's `status` is already `InProgress` OR `Completed`
- **THEN** the bot replies `✗ send it: a brownfield batch is already <in progress | completed> for survey <request_id>.`
- **AND** no duplicate `BrownfieldBatchAction` is submitted

### Requirement: Inbound listener routes `send it` to the spec-revision executor when posted in a revision thread
The `send it` verb SHALL recognize a spec-revision thread as one of its thread contexts (the full dispatch order AND the untracked-thread refusal are defined by `Inbound listener dispatches send it by thread context AND refuses untracked threads`, NOT restated here): when `@<bot> send it` is posted as a reply whose `thread_ts` matches a stored `RevisionThreadState`, the dispatcher SHALL run the spec-revision executor for that change INSTEAD OF the other contexts' handlers.

A reply in a revision thread that is an `@<bot>` mention but is NOT the `send it` verb SHALL route to the read-only revision advisor (per the revision-advisor requirement), so the operator can discuss the revision before triggering it. A bare reply with no mention is not seen by the listener (consistent with the other contexts).

#### Scenario: Send-it in a revision thread runs the revision executor
- **WHEN** an operator posts `@<bot> send it` as a reply whose `thread_ts` matches a stored `RevisionThreadState`
- **THEN** the dispatcher runs the spec-revision executor for that change
- **AND** the audit, survey, and issue-candidate lookups are not acted on

#### Scenario: A discussion reply routes to the advisor
- **WHEN** an operator posts an `@<bot>` reply in a revision thread that is not the `send it` verb
- **THEN** the dispatcher routes it to the read-only revision advisor
- **AND** no spec file is written by that reply

### Requirement: Inbound listener routes `send it` to issue-candidate promotion when posted in an issue-candidate thread
The `send it` verb SHALL recognize an issue-candidate thread as one of its thread contexts (the full dispatch order AND the untracked-thread refusal are defined by `Inbound listener dispatches send it by thread context AND refuses untracked threads`, NOT restated here): when `@<bot> send it` is posted as a reply whose `thread_ts` matches a posted issue-candidate's recorded thread, the dispatcher SHALL promote that candidate INSTEAD OF the other contexts' handlers.

On a match whose candidate `status` is the posted (not-yet-promoted) state, the dispatcher SHALL submit the promotion control-socket action carrying the candidate identity AND the originating `channel`/`thread_ts`, AND reply with the write-and-queue confirmation. On a match whose candidate is already promoted, the dispatcher SHALL reply that the candidate is already promoted AND submit nothing.

#### Scenario: Send-it promotes a posted issue candidate
- **WHEN** an operator posts `@<bot> send it` as a reply whose `thread_ts` matches a stored issue candidate whose status is the posted (not-yet-promoted) state
- **THEN** the dispatcher submits the promotion control-socket action carrying the candidate identity AND the originating `channel`/`thread_ts`
- **AND** on success the bot replies that it wrote `issues/<slug>/` AND queued it for the issues lane

#### Scenario: Send-it on an already-promoted candidate takes no new action
- **WHEN** an operator posts `@<bot> send it` in an issue-candidate thread whose candidate is already promoted
- **THEN** the bot replies that the candidate is already promoted
- **AND** no promotion action is submitted
