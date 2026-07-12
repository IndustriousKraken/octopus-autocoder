## MODIFIED Requirements

### Requirement: Inbound listener dispatches `send it` by thread context AND refuses untracked threads
This requirement is the SINGLE canonical owner of the `send it` thread-context dispatch order AND the untracked-thread refusal. The per-context routing requirements (audit, brownfield-survey, issue-candidate, spec-revision, AND discuss) define ONLY their own positive branch AND cite this requirement for the lookup order AND the refusal; they SHALL NOT restate the five-set lookup OR the untracked-thread refusal text.

When `@<bot> send it` (case-insensitive on `send it`) arrives as a thread reply (a non-empty parent `thread_ts`), the listener SHALL look the parent `thread_ts` up against FIVE per-workspace sets, in this order, matching AT MOST ONE record across all five:

1. Audit-thread set (per `send it verb in an audit thread schedules a triage executor run`).
2. Brownfield-survey set — `BrownfieldSurveyState.thread_ts` values (per `Inbound listener routes send it to BrownfieldBatchAction when posted in a brownfield-survey thread`).
3. Issue-candidate set — the `thread_ts` values recorded on stored issue-candidate states (per `Inbound listener routes send it to issue-candidate promotion when posted in an issue-candidate thread`).
4. Revision-thread set — the `thread_ts` values recorded on stored `RevisionThreadState` entries (per `Inbound listener routes send it to the spec-revision executor when posted in a revision thread`).
5. Discuss-thread set — the `thread_ts` values recorded on active `DiscussionState` entries (per `send it in a discuss thread creates an artifact sequentially`).

On a match, the corresponding context's handler fires, as defined by that context's requirement. If the reply matches NONE of the five tracked sets, the listener SHALL post the untracked-thread refusal `✗ This reply is in a thread autocoder is not tracking. The \`send it\` verb only acts in an audit-notification, brownfield-survey, issue-candidate, spec-revision, or discuss thread.` AND submit no control-socket action.

A `send it` at TOP LEVEL (no parent `thread_ts`, not a thread reply) is NOT a thread context: it parses as the unknown-verb fallback (the `?` reaction, per `Unrecognised verbs get a \`?\` reaction`), NOT the untracked-thread refusal.

#### Scenario: Lookup walks the five sets in order, matching at most one
- **WHEN** an operator posts `@<bot> send it` as a thread reply
- **THEN** the listener looks the parent `thread_ts` up against the audit, brownfield-survey, issue-candidate, revision, AND discuss sets in that order
- **AND** at most one record matches across the five sets, AND that context's handler fires

#### Scenario: Untracked thread reply is politely refused
- **WHEN** an operator posts `@<bot> send it` as a reply in a thread that matches none of the five tracked sets
- **THEN** the bot replies `✗ This reply is in a thread autocoder is not tracking. The \`send it\` verb only acts in an audit-notification, brownfield-survey, issue-candidate, spec-revision, or discuss thread.`
- **AND** no control-socket action is submitted

#### Scenario: Top-level send it is the `?` fallback, not the refusal
- **WHEN** an operator posts `@<bot> send it` at top level (no parent `thread_ts`, not a thread reply)
- **THEN** it parses as the unknown-verb fallback (the `?` reaction)
- **AND** it is NOT the untracked-thread refusal AND no action is submitted

### Requirement: Inbound listener recognizes the `brownfield-survey` verb AND submits a `BrownfieldSurveyAction`
The inbound chatops listener SHALL recognize `@<bot> brownfield-survey <repo-substring> [optional guidance]` as a known verb. The listener SHALL parse the repo-substring per the existing match rule AND treat everything after the substring as optional guidance (trimmed, line breaks preserved, capped at 10,000 characters).

On a unique repo match AND `features.brownfield_survey.enabled: true` for that repo, the dispatcher SHALL generate a `request_id`, post a top-level ack `✓ Queued brownfield-survey for <repo_url>. The next polling iteration will run it (~Nm). Follow along in this thread.`, capture the ack's `ts` as `thread_ts`, AND submit `BrownfieldSurveyAction { repo_url, guidance: Option<String>, channel, thread_ts, request_id }`.

#### Scenario: Happy-path queueing with guidance
- **WHEN** an operator posts `@<bot> brownfield-survey myrepo focus on the data layer; skip CLI commands`
- **AND** `myrepo` uniquely resolves AND survey is enabled
- **THEN** the bot posts the top-level ack
- **AND** a `BrownfieldSurveyAction` with the guidance text is submitted
- **AND** the per-repo `pending_brownfield_survey_requests` queue gains the request_id

#### Scenario: Survey disabled per workspace
- **WHEN** the resolved repo has `features.brownfield_survey.enabled: false`
- **THEN** the bot replies `✗ brownfield-survey: disabled in this workspace's config (features.brownfield_survey.enabled=false).`
- **AND** no action is submitted

#### Scenario: Ambiguous repo substring
- **WHEN** the substring matches multiple configured repos
- **THEN** the bot replies with the existing `match_repo`-style candidate list
- **AND** no action is submitted

#### Scenario: Help verb lists the new verbs
- **WHEN** an operator posts `@<bot> help`
- **THEN** the help output lists `brownfield-survey` (chat-driven workflow) AND `clear-survey` (operator recovery)
- **AND** `send it`'s help text names all five valid thread contexts (audit, brownfield-survey, issue-candidate, spec-revision, AND discuss)
