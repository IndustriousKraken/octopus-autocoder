# orchestrator-cli delta: audit-failure-backoff

## ADDED Requirements

### Requirement: Repeated audit failures back off between re-attempts
A failed audit attempt SHALL NOT be re-attempted on every polling iteration. Every audit attempt is a billed agentic session; a deterministically failing audit — one that always times out, always trips its write policy, or never submits a verdict — would otherwise re-bill a full session each iteration (hundreds per day at production poll intervals) while its failure alert is throttled to one per 24 hours.

The scheduler SHALL record each failed attempt — a `run()` error, a write-policy violation, or a `DidNotComplete` outcome — in failure-tracking fields (attempt timestamp AND consecutive-failure count) of the audit's per-workspace state, kept distinct from the cadence fields: a failure still never advances cadence. The scheduler SHALL NOT re-attempt an audit whose last attempt failed until a backoff has elapsed since that attempt. The backoff SHALL start at the repository's poll interval and double per consecutive failure, capped at the smaller of the audit's configured cadence interval and 24 hours (so eventual retry is preserved). A successful terminal outcome SHALL clear the failure tracking. A `WorkspaceUnavailable` skip is NOT a failed attempt and SHALL NOT touch the failure tracking (its existing no-state-update semantics stand). Operator-queued on-demand runs (the chatops `audit` verb / CLI `audit run`) SHALL bypass the backoff — an explicit operator retry is always allowed — while their terminal outcome updates the failure tracking normally.

Where another requirement describes a failed audit as retried on "the next iteration", retry ELIGIBILITY is constrained by this backoff: the re-attempt happens on the first iteration AFTER the backoff has elapsed. The per-audit requirements describe what happens when an audit runs and how its failure is surfaced; this requirement owns when a failed audit becomes eligible to run again.

The existing audit-failure alert SHALL name the consecutive-failure count AND when the next automatic re-attempt is eligible, so the operator sees both the repeated spend and the pause. Each backoff-suppressed iteration SHALL log one INFO line naming the audit type, the consecutive-failure count, and the remaining wait.

#### Scenario: A deterministically failing audit stops re-billing every iteration
- **WHEN** an audit's attempt fails AND the next polling iteration begins before the backoff has elapsed
- **THEN** the scheduler does NOT spawn a session for that audit in that iteration
- **AND** an INFO log line names the audit type, the consecutive-failure count, and the remaining wait

#### Scenario: Backoff doubles per consecutive failure and is capped
- **WHEN** an audit fails on consecutive attempts
- **THEN** the wait before each re-attempt doubles, starting from the repository's poll interval
- **AND** the wait never exceeds the smaller of the audit's cadence interval and 24 hours

#### Scenario: Success clears the failure tracking
- **WHEN** an audit with a non-zero consecutive-failure count completes a successful attempt
- **THEN** the failure tracking is cleared
- **AND** subsequent scheduling follows cadence alone

#### Scenario: Operator-queued run bypasses the backoff
- **WHEN** an operator queues the audit (chatops `audit` verb or CLI `audit run`) while a backoff is open
- **THEN** the queued run executes on the next iteration regardless of the backoff
- **AND** its terminal outcome updates the failure tracking normally (success clears it; failure re-arms it)

#### Scenario: WorkspaceUnavailable is not a failed attempt
- **WHEN** an audit returns `WorkspaceUnavailable`
- **THEN** the failure tracking is unchanged AND no backoff is armed

## MODIFIED Requirements

### Requirement: Audit runs fail closed to a non-passing did-not-complete outcome
Every audit run SHALL initialize its outcome to an explicit non-passing "did not complete" state, conforming to the project-documentation `gatekeepers-fail-closed` standard. That initial state SHALL be overwritten ONLY by an evidenced terminal verdict: a session that demonstrably ran to completion AND either produced its expected artifact OR positively declared a survey conclusion. A run that cannot produce such evidence SHALL resolve to a surfaced did-not-complete outcome — never to a passing `NoFindings` / empty `SpecsWritten` result.

The audit framework SHALL expose a `DidNotComplete { audit_type, cause, examined_summary }` outcome variant. `cause` distinguishes at least: a session error (timeout, non-zero exit, **OR an exit status that was not captured**); a session that ended without declaring any terminal verdict; and a session that declared findings it could not persist. The scheduler SHALL treat `DidNotComplete` like the existing audit-failure path — it SHALL NOT advance the audit's cadence state, it SHALL surface the failure (chatops alert when a backend is configured), AND it SHALL record a failed attempt for the re-attempt backoff (per "Repeated audit failures back off between re-attempts") — and SHALL keep it distinct from `NoFindings`, `SpecsWritten`, and `WorkspaceUnavailable`.

For a specs-writing audit, "no findings" SHALL be backed by the agent's positive declaration that it examined the code and reached that conclusion; the mere absence of new change directories SHALL NOT by itself be reported as "no findings." A specs-writing audit's terminal outcome — its written-proposals result OR its did-not-complete result — SHALL carry an `examined_summary` (the agent's account of what it looked at) so that even a clean run is accompanied by evidence the audit actually ran, and so the on-demand completion notification can report it.

#### Scenario: Outcome is non-passing until an evidenced verdict overwrites it
- **WHEN** an audit run begins
- **THEN** its outcome is initialized to a non-passing did-not-complete state
- **AND** only a session that ran to completion AND produced its expected artifact OR positively declared a survey conclusion may overwrite that state with a passing outcome

#### Scenario: Uncaptured exit status is a failure, not a pass
- **WHEN** an audit's wrapped session ends AND no exit status was captured (e.g. the process was signal-killed)
- **THEN** the audit resolves to `DidNotComplete { cause: <session-errored>, .. }`
- **AND** the scheduler does NOT advance the cadence state AND surfaces the failure
- **AND** a failed attempt is recorded for the re-attempt backoff
- **AND** the outcome is NOT `NoFindings` or empty `SpecsWritten`

#### Scenario: Findings that cannot be persisted are surfaced, not dropped
- **WHEN** a specs-writing audit's agent declares it found one or more issues but no valid change directory was persisted for them
- **THEN** the audit resolves to `DidNotComplete { cause: <found-but-could-not-persist>, .. }`
- **AND** a chatops alert is posted AND the cadence state is NOT advanced AND a failed attempt is recorded for the re-attempt backoff
- **AND** the run is NOT reported as "0 findings"

#### Scenario: A specs-writing outcome carries an examined summary
- **WHEN** a specs-writing audit reaches a terminal outcome (proposals written, no findings, or did-not-complete)
- **THEN** the outcome carries an `examined_summary` describing what the audit looked at, available to the on-demand completion notification and its conclusion

### Requirement: Periodic audit framework
autocoder SHALL include a periodic audit framework that runs registered audit tasks on per-audit cadences, persists last-run state per workspace, applies per-audit sandbox profiles, enforces post-hoc write restrictions, writes per-invocation logs, AND integrates with the polling loop. **The audit phase SHALL run AFTER the pending change queue walk completes, not before.** This change prevents an audit storm (e.g., 5 audits becoming eligible simultaneously after a HEAD change) from monopolizing the daemon for hours and blocking pending changes. Spec-writing audits' generated changes wait one iteration for implementation — the audit's creation commits ship in iteration N's PR; the implementer's commits for those generated changes ship in iteration N+1's PR.

#### Scenario: Framework runs registered audits after the pending queue walk
- **WHEN** a polling iteration completes its `recreate_branch` step
  AND completes `queue::list_waiting` AND `queue::list_pending`
- **AND** the iteration has remaining wall-clock budget AND has not been gated by an open PR
- **THEN** the framework iterates registered audits in declaration order
- **AND** for each audit, checks `.audit-state.json` to determine whether the configured cadence has elapsed AND `requires_head_change` is satisfied
- **AND** runs the audit only when due

#### Scenario: requires_head_change suppresses re-runs when HEAD unchanged
- **WHEN** an audit's `requires_head_change()` returns `true` AND the recorded `last_run_sha` for that audit equals the current `HEAD` SHA on the base branch
- **THEN** the framework skips the audit for this iteration even if the cadence interval has elapsed
- **AND** the next iteration after a HEAD change re-evaluates cadence and runs the audit if due

#### Scenario: requires_head_change false runs on cadence regardless of HEAD
- **WHEN** an audit's `requires_head_change()` returns `false` AND the cadence has elapsed since `last_run_at`
- **THEN** the framework runs the audit regardless of whether `HEAD` has changed
- **AND** this allows audits whose inputs are external (e.g. package registries, GitHub PR lists) to run periodically without depending on local code changes

#### Scenario: WritePolicy::None audit cannot modify the workspace
- **WHEN** an audit declares `WritePolicy::None` AND it runs
- **THEN** the audit's sandbox allows only `Read`, `Glob`, `Grep`, `Bash` — `Write` and `Edit` are denied at the tool layer
- **AND** after the audit returns, the framework runs `git status --porcelain` and asserts the workspace is clean
- **AND** if either the sandbox blocks a write attempt OR the post-hoc diff is non-empty, the audit is treated as failed: its cadence state is NOT advanced (the failed attempt is recorded for the re-attempt backoff per "Repeated audit failures back off between re-attempts"), a chatops alert is posted, and the diff is reverted via `git reset --hard HEAD`

#### Scenario: WritePolicy::OpenSpecOnly audit may only write under openspec/changes/
- **WHEN** an audit declares `WritePolicy::OpenSpecOnly` AND it runs
- **THEN** the audit's sandbox allows `Write` and `Edit`
- **AND** after the audit returns, the framework inspects `git status --porcelain` and asserts every modified or new path begins with `openspec/changes/`
- **AND** if any path outside that prefix is touched, the audit is treated as failed: its cadence state is NOT advanced (the failed attempt is recorded for the re-attempt backoff per "Repeated audit failures back off between re-attempts"), chatops alert is posted, the entire workspace diff is reverted

#### Scenario: Audit-run log written per invocation
- **WHEN** an audit runs (regardless of outcome)
- **THEN** autocoder writes a timestamped log at the resolved logs-dir path
- **AND** the log contains the audit type, workspace path, start AND end timestamps, resolved cadence + last-run info, the prompt used (for LLM audits), the raw audit output, AND the final `AuditOutcome` variant

#### Scenario: AuditOutcome::Reported posts to chatops
- **WHEN** an audit returns `AuditOutcome::Reported(findings)` AND chatops is configured
- **THEN** autocoder posts a single chatops message with a header line `📋 <repo>: <audit_type> — <N> finding(s)` followed by a bullet list of finding subjects

#### Scenario: AuditOutcome::Reported with no findings posts a brief OK
- **WHEN** an audit returns `AuditOutcome::Reported(vec![])` AND chatops is configured AND the operator has set `audits.<audit_type>.notify_on_clean: true` (default `false`)
- **THEN** autocoder posts `✅ <repo>: <audit_type> — no findings`
- **AND** when `notify_on_clean` is unset or `false`, no chatops post is made for an empty-findings outcome (silence is success)

#### Scenario: AuditOutcome::SpecsWritten records the change names; implementation waits one iteration
- **WHEN** an audit returns `AuditOutcome::SpecsWritten(names)` with non-empty `names`
- **THEN** the framework logs an info line naming each created change
- **AND** the audit's creation commit (one commit titled `audit: <type> proposals (N change(s))`) is on the agent branch when the iteration's push+PR step runs
- **AND** the new changes are NOT processed by THIS iteration's queue walk (because the queue walk already completed before the audit ran)
- **AND** the new changes ARE picked up by the NEXT iteration's `queue::list_pending` for normal implementer processing
- **AND** the implementer's commits for those changes ship in iteration N+1's PR — separable from iteration N's PR which contains only the audit creation commits

#### Scenario: State persists across daemon restarts
- **WHEN** the daemon stops AND restarts later
- **THEN** the framework reads `<workspace>/.audit-state.json` at startup AND resumes the existing cadence
- **AND** an audit due during the daemon's downtime runs on the first qualifying iteration after restart

#### Scenario: Audit failure does not abort the iteration
- **WHEN** an audit's `run()` returns `Err`
- **THEN** the framework logs the error at ERROR level naming the audit type and excerpt
- **AND** the audit's cadence fields in `.audit-state.json` are NOT advanced (only the failure-tracking fields for the re-attempt backoff are updated, per "Repeated audit failures back off between re-attempts")
- **AND** the iteration continues to the push+PR step normally — the audit failure is isolated to that audit; other audits AND the push step are unaffected

#### Scenario: Iteration with pending changes processes them before audits
- **WHEN** an iteration begins AND has 2 pending changes in the queue AND 1 audit eligible to run
- **THEN** the iteration first processes both pending changes via the implementer (commits + archives)
- **AND** THEN runs the eligible audit
- **AND** the push+PR step at iteration end includes commits from both phases
- **AND** an operator watching chatops sees `🚀 starting work on <change>` BEFORE any `🔍 created proposal` or `📋 audit findings` messages for that iteration

#### Scenario: Iteration with only audits processes them when no pending exist
- **WHEN** an iteration begins AND has 0 pending changes AND 1 audit eligible to run
- **THEN** the iteration runs the audit
- **AND** the push+PR step ships the audit's commits (if any)
- **AND** if the audit created new proposals, those become pending for next iteration's queue walk
