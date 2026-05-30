## ADDED Requirements

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

### Requirement: Inbound listener routes `send it` to `BrownfieldBatchAction` when posted in a brownfield-survey thread
The existing `send it` verb (per the canonical `audit-reply-acts` mechanism — unchanged for audit threads) SHALL gain a SECOND recognized context: when posted as a reply inside a brownfield-survey lifecycle thread, the listener SHALL submit a `BrownfieldBatchAction { survey_request_id, channel, thread_ts }` INSTEAD OF the canonical audit-triage action.

At parse time, the listener SHALL look up the parent thread's `ts` against TWO sets of per-workspace state:

1. Audit-thread set — existing canonical mechanism, unchanged.
2. Brownfield-survey set — `BrownfieldSurveyState.thread_ts` values across the workspace's stored surveys.

If the parent thread matches an audit thread, the existing canonical handler fires. If it matches a brownfield-survey thread, the new `BrownfieldBatchAction` is submitted. If it matches neither, the listener posts the existing "send it: only valid as a reply in a known thread context" rejection (the rejection text MAY be updated to name the survey context as one of the valid options).

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
- **WHEN** an operator posts `@<bot> send it` at top level OR in an unrecognized thread (not audit, not survey)
- **THEN** the bot replies with the rejection message naming the valid contexts (audit thread OR brownfield-survey thread)
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
- **AND** `send it`'s help text names BOTH valid thread contexts (audit OR brownfield-survey)
