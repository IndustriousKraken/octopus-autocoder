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
