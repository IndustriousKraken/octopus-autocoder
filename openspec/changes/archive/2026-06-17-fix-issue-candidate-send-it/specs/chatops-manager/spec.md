## ADDED Requirements

### Requirement: Inbound listener routes `send it` to issue-candidate promotion when posted in an issue-candidate thread

The `send it` verb SHALL recognize an issue-candidate thread as one of its thread contexts: when `@<bot> send it` is posted as a reply whose `thread_ts` matches a posted issue-candidate's recorded thread, the dispatcher SHALL promote that candidate INSTEAD OF the audit-triage, brownfield-batch, OR spec-revision actions. This requirement DEFINES the issue-candidate routing that the canonical `send it`-context requirements already reference: the `Inbound listener routes send it to BrownfieldBatchAction when posted in a brownfield-survey thread` requirement enumerates the full thread-context set (audit, brownfield-survey, issue-candidate, AND spec-revision) AND cites this requirement for the issue-candidate branch, but the branch itself was not previously defined OR wired.

At parse time, after the audit-thread lookup AND the brownfield-survey lookup both miss, the dispatcher SHALL look up the reply's `thread_ts` against the issue-candidate set — the `thread_ts` values recorded on stored candidate states — BEFORE the spec-revision lookup (matching the context order the canonical brownfield-survey requirement states). A `thread_ts` resolves to at most one record across the four sets.

On a match whose candidate `status` is the posted (not-yet-promoted) state, the dispatcher SHALL submit the promotion control-socket action carrying the candidate identity AND the originating `channel`/`thread_ts`, AND reply with the write-and-queue confirmation. On a match whose candidate is already promoted, the dispatcher SHALL reply that the candidate is already promoted AND submit nothing. When a thread reply matches NONE of the four sets, the dispatcher SHALL post the untracked-thread refusal, whose text names all four valid contexts (audit thread, brownfield-survey thread, issue-candidate thread, spec-revision thread). A `send it` at top level (not a thread reply) is NOT a thread context — it parses as the unknown-verb fallback (the `?` reaction), per the canonical brownfield-survey requirement, NOT the untracked-thread refusal.

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

#### Scenario: Send-it in an untracked thread names all four contexts

- **WHEN** an operator posts `@<bot> send it` as a reply in a thread that
  matches no audit, survey, issue-candidate, OR spec-revision record
- **THEN** the bot posts the untracked-thread refusal naming the four valid
  contexts (audit thread, brownfield-survey thread, issue-candidate thread,
  spec-revision thread)
- **AND** no action is submitted
