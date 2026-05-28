## ADDED Requirements

### Requirement: Polling iteration termination is gated on agent-branch commit count, NOT on implementer-queue outcome
The polling iteration's "no work to ship" early-return SHALL be gated EXCLUSIVELY on the agent branch's commit count relative to base — computed via `git rev-list --count <base_branch>..<agent_branch>` or equivalent. The early-return SHALL NOT use any higher-level signal (the implementer-processed-changes list, the audit-queue length, the reviewer-finding count, etc.) as the sole gate. The reason: any signal captured BEFORE the audit phase runs (the audit phase runs AFTER the queue walk per the canonical "audit phase runs AFTER pending change queue walk" requirement) can miss commits the audit phase subsequently produced. Using a stale signal to gate the push step causes audit-produced commits to be silently destroyed by the next iteration's `recreate_branch` step, which presented in production as "🔍 created proposal notifications without PRs" across multiple repos.

When the agent-branch commit count is zero — meaning neither the implementer NOR any audit produced commits — the iteration SHALL clear `AlertState` AND return `Ok(())`. When the agent-branch commit count is non-zero, the iteration SHALL proceed to the push + PR-creation steps regardless of how the implementer-queue walk concluded. The canonical "audit's creation commits ship in iteration N's PR" requirement is thereby implementable: an iteration that did no implementer work but had an audit produce proposal commits ships those commits in a PR.

This requirement is additive: it codifies an invariant that protects against future regressions of the same bug class. Any code change that introduces a new commit-producing iteration phase (a future spec-writing audit type, a future autonomous-fix mechanism, etc.) automatically benefits — the termination gate is already correct, the new commits already get pushed.

#### Scenario: Audit-only iteration pushes and opens PR
- **WHEN** a polling iteration's queue walk produces zero implementer-processed changes (`processed` is empty)
- **AND** a spec-writing audit (e.g., `security_bug_audit`) runs during the audit phase AND returns `SpecsWritten` AND commits its produced proposal directories to the agent branch
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
- **THEN** the reviewer's `review()` method is NOT invoked
- **AND** the PR is opened with NO `## Code Review` section
- **AND** the rationale is: the audit's own validation pass already gated each proposal (`openspec validate --strict` per the canonical "LLM-driven audits validate their generated proposals before committing" requirement); a code-quality reviewer adds no signal against mechanical proposal-writing

#### Scenario: PR body for audit-only iterations names the audit-produced proposals
- **WHEN** the iteration opens a PR with `processed.is_empty()` AND `commit_count > 0`
- **THEN** the PR title takes the form `audit-only: <N> proposal(s) from <comma-separated-audit-types>`
- **AND** the PR body explicitly states this is an audit-only PR with no implementer changes
- **AND** the PR body lists the agent-branch commit subjects (sourced from `git log <base>..<agent> --format=%s`) so reviewers see which audits fired AND how many proposals each produced
- **AND** the PR body notes that the produced `openspec/changes/<prefix>-*` directories will be picked up by the NEXT polling iteration's `list_pending` for implementer routing

#### Scenario: Regression test guards the gate
- **WHEN** the test suite runs
- **THEN** at least one test sets up a fixture iteration with empty `processed` AND a mock audit that produces commits AND asserts: (a) the push function IS called, (b) the PR-creation function IS called, (c) the PR's head ref matches the agent branch
- **AND** the test fails against any implementation that gates the early-return on `processed.is_empty()` instead of on the agent-branch commit count
