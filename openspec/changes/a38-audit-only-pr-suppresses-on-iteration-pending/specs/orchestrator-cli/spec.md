## MODIFIED Requirements

### Requirement: Polling iteration termination is gated on agent-branch commit count, NOT on implementer-queue outcome

The polling iteration's "no work to ship" early-return SHALL be gated EXCLUSIVELY on the agent branch's commit count relative to base — computed via `git rev-list --count <base_branch>..<agent_branch>` or equivalent. The early-return SHALL NOT use any higher-level signal (the implementer-processed-changes list, the audit-queue length, the reviewer-finding count, etc.) as the sole gate. The reason: any signal captured BEFORE the audit phase runs (the audit phase runs AFTER the queue walk per the canonical "audit phase runs AFTER pending change queue walk" requirement) can miss commits the audit phase subsequently produced. Using a stale signal to gate the push step causes audit-produced commits to be silently destroyed by the next iteration's `recreate_branch` step, which presented in production as "🔍 created proposal notifications without PRs" across multiple repos.

When the agent-branch commit count is zero — meaning neither the implementer NOR any audit produced commits — the iteration SHALL clear `AlertState` AND return `Ok(())`. When the agent-branch commit count is non-zero, the iteration SHALL proceed to the push + PR-creation steps EXCEPT when iteration-pending markers are present per the suppression rule below. The canonical "audit's creation commits ship in iteration N's PR" requirement is thereby implementable: an iteration that did no implementer work but had an audit produce proposal commits ships those commits in a PR.

**Iteration-pending suppression rule (new in this change).** Before invoking the push + PR-creation steps, the polling-loop SHALL scan `<workspace>/openspec/changes/*/.iteration-pending.json` markers. When one or more markers are present, the audit-only-PR path SHALL be SUPPRESSED for this iteration: the iteration SHALL log INFO `audit-only PR path suppressed: iteration-pending markers present for <change-list>` AND return `Ok(())` WITHOUT pushing OR opening a PR. Audit proposals committed during this iteration remain on agent-q AND on disk in `openspec/changes/<aXX>-*` directories; they ship in the NEXT iteration's PR after the iteration-pending change concludes (via `outcome_success`, `outcome_spec_needs_revision`, OR the `a27a1` 5-iteration cap).

The suppression rule trades off two failure modes: opening a PR that mixes iteration_request WIP with audit findings (operator-confusing, mergable-yet-shouldn't-be) versus deferring audit findings by a few polling cycles (operator-invisible, no data loss). The latter is preferred because audit cadences are periodic (operators don't expect immediate proposals on every iteration) AND because the iteration sequence is bounded by `a27a1`'s 5-iteration cap (worst-case audit-finding delay is 5 iterations of the in-progress change).

This requirement is additive: it codifies an invariant that protects against future regressions of the same bug class. Any code change that introduces a new commit-producing iteration phase (a future spec-writing audit type, a future autonomous-fix mechanism, etc.) automatically benefits — the termination gate is already correct, the new commits already get pushed when iteration-pending markers are absent.

#### Scenario: Audit-only iteration pushes and opens PR
- **WHEN** a polling iteration's queue walk produces zero implementer-processed changes (`processed` is empty)
- **AND** a spec-writing audit (e.g., `security_bug_audit`) runs during the audit phase AND returns `SpecsWritten` AND commits its produced proposal directories to the agent branch
- **AND** NO `.iteration-pending.json` markers are present in any change directory
- **THEN** the iteration's commit-count check returns a non-zero value (the audit's commits ARE on the agent branch)
- **AND** the iteration proceeds to `git::push_force_with_lease` AND to `github::create_pull_request`
- **AND** a `✅ PR opened: <url>` notification fires per the existing canonical PR-opened notification requirement
- **AND** the next iteration's `recreate_branch` step DOES NOT destroy the audit's commits (because they were pushed AND the next iteration's pull-from-remote step retrieves them via the open PR's branch state)

#### Scenario: Empty-implementer + empty-audit iteration still returns early correctly
- **WHEN** a polling iteration's queue walk produces zero implementer-processed changes
- **AND** no audit produces commits (either no audits are due OR all due audits return `NoFindings` / `Reported` outcomes that do NOT commit)
- **THEN** the commit-count check returns zero
- **AND** the iteration clears `AlertState` AND returns `Ok(())` without invoking the push step
- **AND** no `BranchPushFailure` chatops alert fires (there was nothing to push)

#### Scenario: Implementer-non-empty + commit-count-non-zero proceeds normally
- **WHEN** a polling iteration's queue walk processes one OR more changes AND produces at least one commit
- **AND** NO `.iteration-pending.json` markers are present (the implementer changes archived cleanly, not iteration_request)
- **THEN** the commit-count check returns non-zero
- **AND** the iteration proceeds to the reviewer step (if configured) AND to the push + PR-creation steps
- **AND** the canonical happy-path scenarios for end-of-iteration push + PR continue to hold

#### Scenario: Implementer-non-empty but commit-count-zero returns early
- **WHEN** a polling iteration's queue walk processes one OR more changes BUT every processed change's executor invocation produced an empty diff (the existing "all completed changes had empty diffs" path)
- **THEN** the commit-count check returns zero
- **AND** the iteration clears `AlertState` AND returns `Ok(())` per the canonical empty-diff handling
- **AND** the iteration logs an info-level line naming that the pass produced no commits

#### Scenario: Reviewer skipped on audit-only iterations
- **WHEN** the iteration reaches the reviewer step AND `processed.is_empty()` is true AND `commit_count > 0`
- **AND** NO `.iteration-pending.json` markers are present (suppression rule doesn't fire)
- **THEN** the reviewer's `review()` method is NOT invoked
- **AND** the PR is opened with NO `## Code Review` section
- **AND** the rationale is: the audit's own validation pass already gated each proposal (`openspec validate --strict` per the canonical "LLM-driven audits validate their generated proposals before committing" requirement); a code-quality reviewer adds no signal against mechanical proposal-writing

#### Scenario: PR body for audit-only iterations names commits by category
- **WHEN** the iteration opens a PR with `processed.is_empty()` AND `commit_count > 0` AND NO iteration-pending markers
- **THEN** the PR body composition partitions the agent-branch commits by message-prefix category: `audit: <type>` → audit-produced; `iteration N of <change>` → iteration WIP; `archive: <change>` → implementer-archived; anything else → manual / unknown
- **AND** the body includes only sections for non-empty categories (the "Audit-produced proposals" section appears ONLY when audit-produced commits exist)
- **AND** when ALL commits are audit-produced, the PR title takes the canonical form `audit-only: <N> proposal(s) from <comma-separated-audit-types>`
- **AND** the PR body lists the agent-branch commit subjects per the present categories (sourced from `git log <base>..<agent> --format=%s`) so reviewers see exactly what the PR contains
- **AND** the PR body notes that the produced `openspec/changes/<prefix>-*` directories will be picked up by the NEXT polling iteration's `list_pending` for implementer routing (when audit-produced commits are present)

#### Scenario: Regression test guards the gate
- **WHEN** the test suite runs
- **THEN** at least one test sets up a fixture iteration with empty `processed` AND a mock audit that produces commits AND NO `.iteration-pending.json` markers AND asserts: (a) the push function IS called, (b) the PR-creation function IS called, (c) the PR's head ref matches the agent branch
- **AND** the test fails against any implementation that gates the early-return on `processed.is_empty()` instead of on the agent-branch commit count

#### Scenario: Iteration-pending suppression with iteration_request WIP only
- **WHEN** a polling iteration's `IterationRequested` arm just committed iteration_request WIP to agent-q for change X
- **AND** the `.iteration-pending.json` marker for change X is present on disk
- **AND** no other commits are on agent-q ahead of base
- **THEN** the commit-count check returns non-zero (the iteration_request commit IS on agent-q)
- **AND** the iteration-pending scan returns `[X]`
- **AND** the audit-only-PR path is SUPPRESSED
- **AND** the iteration logs INFO `audit-only PR path suppressed: iteration-pending markers present for X`
- **AND** the iteration returns `Ok(())` WITHOUT calling `git::push_force_with_lease` OR `github::create_pull_request`
- **AND** no PR opens

#### Scenario: Iteration-pending suppression with audit commits AND iteration WIP
- **WHEN** a polling iteration has BOTH audit-produced commits AND iteration_request WIP commits on agent-q
- **AND** at least one `.iteration-pending.json` marker is present
- **THEN** the audit-only-PR path is SUPPRESSED (the suppression rule fires on ANY marker presence; mixed commit content doesn't change the suppression decision)
- **AND** the audit-produced commits remain on agent-q AND in their respective `openspec/changes/<aXX>-*` directories
- **AND** the next iteration (after the iteration-pending change concludes) opens an audit-only PR with the audit commits

#### Scenario: Iteration-pending absent → audit-only PR opens as today
- **WHEN** a polling iteration has audit-produced commits on agent-q AND NO `.iteration-pending.json` markers anywhere
- **THEN** the audit-only-PR path fires normally (existing happy path)
- **AND** a PR is opened with the canonical audit-only title format

## ADDED Requirements

### Requirement: Integration test verifies `IterationRequested` arm writes the iteration-pending marker AND clears the in-progress lock

A canonical integration test SHALL exercise the polling-loop's `IterationRequested` outcome arm end-to-end against a temp workspace + temp bare git repo fixture AND assert two filesystem postconditions:

1. `<workspace>/openspec/changes/<change>/.iteration-pending.json` exists on disk after the arm completes AND parses to a payload containing `completed_tasks`, `remaining_tasks`, `reason`, AND `iteration_number` per `a27a1`'s "Iteration-pending marker file in the change directory carries state across iteration boundaries" requirement.
2. `<workspace>/openspec/changes/<change>/.in-progress` does NOT exist on disk after the arm completes, per the canonical openspec-queue-engine "Unlocking after any executor outcome" requirement.

This test pins the implementation against the silent-drop failure mode observed in production: the canonical specs require both filesystem effects, the implementation can silently skip either one, AND unit-level tests of the helpers don't exercise the integration. A failure in either postcondition fails the build with a clear message naming which file is missing AND which canonical requirement governs the expectation.

The test SHALL also assert that the iteration_request commit IS present on the agent branch (commit-count > 0) so a regression where the IterationRequested arm fails the commit + push step BUT still drops the lock is caught.

#### Scenario: IterationRequested arm writes marker AND clears in-progress
- **WHEN** the integration test drives a polling iteration whose stub executor returns `IterationRequested { completed_tasks: ["1", "2"], remaining_tasks: ["3"], reason: "scope-overflow", iteration_number: 2 }`
- **AND** the polling loop's `IterationRequested` arm runs end-to-end (commit + force-push + marker write + lock cleanup)
- **THEN** `<workspace>/openspec/changes/<change>/.iteration-pending.json` exists
- **AND** the file parses to `{"completed_tasks": ["1", "2"], "remaining_tasks": ["3"], "reason": "scope-overflow", "iteration_number": 2}` byte-for-byte
- **AND** `<workspace>/openspec/changes/<change>/.in-progress` does NOT exist
- **AND** `git rev-list --count <base>..<agent>` returns 1 (the iteration_request WIP commit was pushed)

#### Scenario: Missing marker fails the test with a clear message
- **WHEN** a hypothetical implementation regression causes the IterationRequested arm to skip the marker write
- **THEN** the integration test fails with a message naming the missing path (`<workspace>/openspec/changes/<change>/.iteration-pending.json`) AND the canonical `a27a1` "Iteration-pending marker file..." requirement as the resolution target

#### Scenario: Stale .in-progress fails the test with a clear message
- **WHEN** a hypothetical implementation regression causes the IterationRequested arm to skip the `.in-progress` cleanup
- **THEN** the integration test fails with a message naming the unexpected file (`<workspace>/openspec/changes/<change>/.in-progress`) AND the canonical "Unlocking after any executor outcome" requirement as the resolution target

#### Scenario: Both filesystem effects exercised in one test (NOT two)
- **WHEN** the integration test is designed
- **THEN** both postconditions are asserted in the SAME test against the SAME fixture (NOT split across two tests)
- **AND** the rationale: a single test that runs the full arm once AND asserts both effects catches the "implementation does the commit + push but neither filesystem effect" failure mode cleanly. Splitting into two tests can let the second test pass against a fixture that the first test mutated, masking the same-arm failure
