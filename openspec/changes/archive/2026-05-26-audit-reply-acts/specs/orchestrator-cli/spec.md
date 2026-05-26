## ADDED Requirements

### Requirement: `send it` verb in an audit thread schedules a triage executor run
The chatops listener SHALL recognize `@<bot> send it` (case-insensitive on `send it`) as the `SendItOnAudit` command ONLY when the message arrives with a non-empty `thread_ts` AND the `thread_ts` matches a tracked audit-thread state with `status: Open` OR `status: TriageFailed`. Same text outside a thread SHALL parse as the unknown-verb fallback (existing `?` reaction). When recognized, the dispatcher SHALL submit a `trigger_audit_action` control-socket action AND flip the audit-thread state's `status` to `TriagePending`. The next polling iteration drains the triage queue and runs the executor in triage mode.

#### Scenario: Send-it in tracked, open audit thread schedules triage
- **WHEN** an operator posts `@<bot> send it` as a thread reply where `thread_ts` matches an `AuditThreadState` with `status: Open` AND `posted_at` within the last 7 days
- **THEN** the dispatcher submits `trigger_audit_action` with the `thread_ts`
- **AND** the state file's `status` is updated to `TriagePending`
- **AND** the bot replies in the thread `✓ Triage scheduled for <audit_type> on <repo_url>. The next polling iteration will run it (~Nm).`

#### Scenario: Send-it in untracked thread is politely refused
- **WHEN** an operator posts `@<bot> send it` in a thread that has no corresponding `AuditThreadState`
- **THEN** the bot replies `✗ This reply is in a thread autocoder is not tracking. The \`send it\` verb only acts in audit-notification threads.`
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

### Requirement: Triage mode runs the executor with an explore-then-classify prompt
The polling iteration SHALL drain its per-repo triage queue (alongside the existing revision-request queue) at iteration start. For each queued triage, the iteration SHALL invoke `executor.run_triage(workspace, ctx)` with a `TriageContext` carrying the audit findings, audit type, repo URL, and a brief canonical-specs index. The triage-mode prompt template (`prompts/audit-triage.md`) SHALL instruct the LLM to first explore the codebase, then triage findings into quick-fix vs spec-worthy categories, apply quick fixes directly to the working tree, and create new `openspec/changes/<derived-slug>/` directories for spec-worthy findings. The slug derives from `<audit-type>-<short-hash-of-findings>` with collision-suffixing when needed.

#### Scenario: Triage mode invokes the executor with the documented context
- **WHEN** the polling iteration drains a queued triage
- **THEN** the executor is invoked via `run_triage` with `TriageContext { findings, audit_type, repo_url, canonical_specs_index }`
- **AND** the prompt sent to the wrapped CLI contains the four substituted variables AND the four-step instruction (explore → classify → fix → spec)

#### Scenario: Triage executor returning AskUser escalates without committing
- **WHEN** the triage executor returns `AskUser { question, resume_handle }`
- **THEN** the existing chatops escalation fires (the question posts to the configured channel)
- **AND** no commit is made on any branch
- **AND** no PR is opened
- **AND** the audit-thread state's `status` stays `TriagePending`

#### Scenario: Triage executor returning Failed flips state and posts a reply
- **WHEN** the triage executor returns `Failed { reason }`
- **THEN** the audit-thread state's `status` flips to `TriageFailed` with `reason` populated
- **AND** the bot posts a reply in the audit thread naming the failure
- **AND** no PRs are created

### Requirement: Completed triage splits into one or two PRs by content path
After the triage executor returns `Completed`, the daemon SHALL inspect the working tree's changed paths and split them by whether each path is inside `openspec/changes/<derived-slug>/`. Paths inside that subtree go to the spec PR; all other paths go to the fixes PR. Each PR is created on its own branch off the same base, with the existing PR-creation helpers. PR bodies cross-link each other when both are created.

#### Scenario: Mixed diff produces two PRs that cross-link
- **WHEN** the triage executor's Completed diff contains code changes outside `openspec/changes/<new_slug>/` AND new files inside `openspec/changes/<new_slug>/`
- **THEN** the daemon creates a fixes branch + PR with the code paths
- **AND** the daemon creates a spec branch + PR with the openspec paths
- **AND** each PR body contains a link to the other ("This PR carries the code fixes; see #<other_pr> for the new spec change." and vice versa)
- **AND** the audit-thread state's `status` flips to `Acted`

#### Scenario: Code-only triage produces only a fixes PR
- **WHEN** the triage diff has only code paths (no new `openspec/changes/<new_slug>/`)
- **THEN** only the fixes PR is created
- **AND** no spec PR is created
- **AND** the audit-thread state's `status` flips to `Acted`

#### Scenario: Spec-only triage produces only a spec PR
- **WHEN** the triage diff has only new `openspec/changes/<new_slug>/` paths
- **THEN** only the spec PR is created
- **AND** no fixes PR is created
- **AND** the audit-thread state's `status` flips to `Acted`

#### Scenario: Empty-diff triage posts a no-action reply
- **WHEN** the triage executor returns Completed but the diff is empty (the LLM decided nothing was actionable)
- **THEN** no PRs are created
- **AND** the bot posts a reply in the audit thread containing the LLM's final-summary text explaining the decision
- **AND** the audit-thread state's `status` flips to `Acted`

#### Scenario: Slug collision is suffixed
- **WHEN** the derived slug `<audit-type>-<hash>` already exists as `openspec/changes/<slug>/`
- **THEN** the daemon increments a suffix (`-2`, `-3`, ...) until it finds a free path
- **AND** the resulting spec directory uses the suffixed slug

### Requirement: Triage-created PRs participate in the existing PR-comment-revision-loop
PRs spawned by audit-reply triage SHALL be structurally identical to polling-loop-spawned PRs from the revision-loop dispatcher's perspective. Operators replying `@<bot> revise <text>` on either the fixes PR OR the spec PR get revisions through the standard channel from `a01-pr-comment-revision-loop`; the dispatcher does not need to distinguish triage-PRs from regular PRs.

#### Scenario: Revision comment on a triage PR is processed normally
- **WHEN** a triage-spawned PR has an operator comment `@<bot> revise <text>`
- **THEN** the existing revision-loop dispatcher (per `a01-pr-comment-revision-loop`) picks up the comment
- **AND** the revision executes against that PR's branch normally
- **AND** the audit-thread state file is not consulted (the revision is its own scope, separate from the audit-thread tracking)

### Requirement: Audit-thread state files are pruned after 7 days
The daemon SHALL prune audit-thread state files whose `posted_at` is older than 7 days. The prune runs periodically (at iteration start, or once per day per the existing housekeeping pattern). Stale entries are removed regardless of `status` — even `Acted` entries are pruned eventually so the audit-threads directory stays bounded.

#### Scenario: Stale entry is removed
- **WHEN** the prune runs AND an `AuditThreadState` has `posted_at` more than 7 days in the past
- **THEN** the state file is removed
- **AND** subsequent `send it` replies in that thread fall through to the untracked-thread polite-refusal

#### Scenario: Fresh entry is preserved
- **WHEN** the prune runs AND an `AuditThreadState` has `posted_at` within the last 7 days
- **THEN** the state file is NOT removed regardless of status
