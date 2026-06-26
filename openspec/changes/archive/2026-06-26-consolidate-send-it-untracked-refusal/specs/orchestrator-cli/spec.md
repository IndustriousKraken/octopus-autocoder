## MODIFIED Requirements

### Requirement: `send it` verb in an audit thread schedules a triage executor run
The chatops listener SHALL recognize `@<bot> send it` (case-insensitive on `send it`) as the `SendItOnAudit` command ONLY when the message arrives with a non-empty `thread_ts` AND the `thread_ts` matches ANY tracked audit-thread state (regardless of status). Same text outside a thread SHALL parse as the unknown-verb fallback (existing `?` reaction). The dispatcher SHALL submit a `trigger_audit_action` control-socket action AND flip the audit-thread state's `status` to `TriagePending` ONLY when the matched state has `status: Open` OR `status: TriageFailed` (and `posted_at` within 7 days); all other statuses produce the per-status refusal defined in the scenarios below. The next polling iteration drains the triage queue and runs the executor in triage mode.

The audit-thread set is the FIRST of the four `send it` thread-context sets; the full dispatch order across all four contexts (audit, brownfield-survey, issue-candidate, spec-revision) AND the untracked-thread refusal for a reply matching none of them are defined by `chatops-manager`'s `Inbound listener dispatches send it by thread context AND refuses untracked threads` — this requirement defines ONLY the audit-thread branch AND does NOT restate the untracked-thread refusal.

#### Scenario: Send-it in tracked, open audit thread schedules triage
- **WHEN** an operator posts `@<bot> send it` as a thread reply where `thread_ts` matches an `AuditThreadState` with `status: Open` AND `posted_at` within the last 7 days
- **THEN** the dispatcher submits `trigger_audit_action` with the `thread_ts`
- **AND** the state file's `status` is updated to `TriagePending`
- **AND** the bot replies in the thread `✓ Triage scheduled for <audit_type> on <repo_url>. The next polling iteration will run it (~Nm).`

#### Scenario: Send-it on stale audit thread is politely refused
- **WHEN** an operator posts `@<bot> send it` in a tracked thread whose `posted_at` is older than 7 days
- **THEN** the bot replies `✗ This audit's findings are too old to act on (>7d). Re-run the audit via @<bot> audit <type> <repo>.`
- **AND** the state file remains unchanged (the prune-stale-entries pass will eventually remove it)

#### Scenario: Send-it on already-acted thread is politely refused
- **WHEN** an operator posts `@<bot> send it` in a thread with `status: Acted` OR `status: TriagePending`
- **THEN** the bot replies `✗ This audit thread is already <status>. No new action taken.`
- **AND** no new triage is scheduled

#### Scenario: Send-it on TriageFailed thread re-attempts triage
- **WHEN** an operator posts `@<bot> send it` in a thread with `status: TriageFailed` AND `posted_at` within the last 7 days
- **THEN** the dispatcher treats the request like the Open case (triage re-scheduled)
- **AND** the state's `status` is reset to `TriagePending` for the new attempt
