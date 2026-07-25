# orchestrator-cli delta: hold-covers-pr-creation-failure

## MODIFIED Requirements

### Requirement: Branch-push failure preserves completed work via a push-block hold
When the pass-level branch push fails — OR the pass-level PR creation fails after a successful push — AFTER one or more changes or issue units were committed (and archived) on the agent branch during the pass, autocoder SHALL NOT discard the completed work NOR re-implement it — re-running the executor cannot fix a transport failure. The completed work is held for its remaining delivery step.

autocoder SHALL write a **push-block marker** in the daemon STATE directory, keyed to the workspace (NOT inside any change directory — the carried changes have already been archived during the pass, so a per-change marker location is unavailable). The marker SHALL record: the unpushed-or-unannounced agent-branch tip commit, the change slug(s) AND issue slug(s) the pass was carrying, the rejection reason, AND which step failed (the push, or PR creation after a successful push). autocoder SHALL post the existing throttled alert for the failing category — `BranchPushFailure` for a push failure, `PrCreationFailure` for a PR-creation failure — naming the rejection reason, the operator remedy, AND that the completed work is preserved on the agent branch.

Branch preservation is ANCHORED by the marker: while a push-block marker is present for the workspace AND the agent branch tip still matches the marker's recorded tip commit (the preserved work is intact), the pass SHALL NOT recreate the agent branch NOR re-run the executor for the carried changes or issue units (see the `git-workflow-manager` "Per-pass agent branch" requirement). A marker is written ONLY on a real push or PR-creation failure and removed ONLY when the held work completes its remaining steps, so it never falsely triggers on a branch that was never held (e.g. a stale post-merge branch). If the marker is present but the tip no longer matches (the preserved work is gone — e.g. an operator deleted the branch), the marker is STALE and SHALL be removed, and the pass proceeds normally (recreate the branch).

While the marker is present and matching, each pass SHALL retry the remaining delivery steps ONLY (never the executor — the work is already committed). For a push-step hold this is the push, then PR creation; for a PR-creation hold the tip is already on the remote, so the push retry is a no-op and PR creation is the effective retry. Retrying a failed `git push` or PR-creation API call costs no executor tokens, so a persistent failure simply re-attempts the cheap step each pass and re-posts the throttled alert until it succeeds. On a SUCCESSFUL push, autocoder SHALL open the PR; the marker is removed once the held work has completed its remaining steps — and if PR creation itself then fails, a PR-creation hold is written so the next pass retries PR creation only. Operator recovery: the operator fixes the underlying cause (e.g. grants the missing token scope or lifts branch protection) and the next pass's retry succeeds automatically; to ABANDON the preserved work instead, the operator deletes the marker file directly, which lets the next pass recreate the branch.

This hold is distinct from the perma-stuck counter (which covers repeated EXECUTION failures that re-implementation could fix); a push or PR-creation failure is not an execution failure, so it does NOT increment the perma-stuck counter — it uses this dedicated push-block hold instead.

#### Scenario: Push failure preserves the branch and writes the push-block marker
- **WHEN** the executor returned `Completed` for change `foo` (committed + archived on the agent branch) AND the pass-level branch push then fails
- **THEN** the agent branch AND its commits are retained (never reset)
- **AND** a push-block marker is written in the state directory keyed to the workspace, recording the unpushed tip commit, `foo`, the rejection reason, AND the failed step (push)
- **AND** the throttled `BranchPushFailure` alert fires naming the reason, the remedy, AND that the work is preserved on the agent branch

#### Scenario: PR-creation failure after a successful push writes the hold
- **WHEN** the pass-level branch push succeeded AND the PR-creation call then fails
- **THEN** the agent branch AND its commits are retained (never reset, locally or on the remote)
- **AND** a push-block marker is written recording the pushed tip commit, the carried slugs, the rejection reason, AND the failed step (PR creation)
- **AND** the throttled `PrCreationFailure` alert fires naming the reason AND that the work is preserved
- **AND** the next pass does NOT recreate the branch NOR re-run the executor: it retries PR creation only (the push retry is a no-op on the already-pushed tip) AND removes the marker on success

#### Scenario: Issue-bearing pass failures get the same hold
- **WHEN** a pass carried only issue units (no changes) AND the push or the PR creation fails
- **THEN** the push-block marker is written with the issue slugs recorded
- **AND** the next pass resumes at the failed delivery step without re-running the executor for those units

#### Scenario: Persistent push failure retries the push without re-implementing
- **WHEN** a push-block marker is present AND matching on a subsequent pass AND the push fails again
- **THEN** the pass retries the push step ONLY
- **AND** the executor is NOT re-run for the already-committed changes
- **AND** the marker is retained (its reason refreshed) and the throttled alert re-posts

#### Scenario: Successful push clears the hold and opens the PR
- **WHEN** a push-block marker is present AND a subsequent push of the preserved branch succeeds
- **THEN** the PR is opened (its body derived from the carried change AND issue slugs) AND the push-block marker is removed once the held work has completed its remaining steps
- **AND** if the PR-creation call itself then fails, a PR-creation hold is written so the next pass retries PR creation only
- **AND** the failure-alert state is cleared on the successful iteration

#### Scenario: Operator fixes the cause and the retry succeeds
- **WHEN** the operator fixes the underlying cause (e.g. grants the missing token scope) while a push-block marker is present
- **THEN** the next pass's retry succeeds with no operator command and no re-implementation
- **AND** an operator who instead deletes the marker file abandons the preserved work — the next pass recreates the branch

#### Scenario: Stale marker whose preserved work is gone
- **WHEN** a push-block marker is present BUT the agent branch tip no longer matches the marker's recorded tip commit (e.g. an operator deleted or moved the branch)
- **THEN** the marker is treated as STALE and removed
- **AND** the pass proceeds normally (recreate the agent branch from base)

#### Scenario: Push failure never resets the branch or workspace
- **WHEN** a branch push OR a PR creation fails for any reason
- **THEN** autocoder performs no destructive reset of the agent branch or the workspace as a consequence
- **AND** the only destructive reset remains an explicit operator action (`rewind` / `wipe-repo`)

### Requirement: Iteration-level error tolerance
The polling loop SHALL continue running after a failed iteration; a single iteration's error MUST NOT terminate the task or affect other repositories. Predictable failure categories (workspace init, mid-iteration dirty workspace, branch push, PR creation) SHALL emit a throttled chatops alert via the existing `AlertCategory` + `handle_predictable_failure` mechanism before the iteration returns `Err`. For the mid-iteration dirty-workspace category, the alert SHALL fire only AFTER an auto-recovery attempt has been made and failed to clean the workspace (see "Dirty workspace auto-recovers mid-iteration").

For the branch-push category, BEFORE returning `Err` the iteration SHALL preserve the completed work — the agent branch AND its commits are retained, never reset — AND write a push-block hold (per "Branch-push failure preserves completed work via a push-block hold") so a subsequent pass resumes at the push step instead of re-running the executor for the already-committed changes. A branch-push failure SHALL NEVER cause a destructive branch or workspace reset; the only destructive reset is an explicit operator action (`rewind` / `wipe-repo`). Re-running the executor cannot fix a push failure (the work is already committed), so the work is held for push, not re-implementation.

For the PR-creation category, the push has already succeeded by the time the failure occurs; BEFORE returning `Err` the iteration SHALL write the same push-block hold (recording that PR creation is the failed step) so a subsequent pass retries PR creation ONLY — never branch recreation, never the executor. A PR-creation failure SHALL NEVER cause the already-pushed work to be discarded or force-clobbered by a subsequent pass's fresh run.

#### Scenario: Iteration fails
- **WHEN** any error occurs during a polling iteration (workspace init, git operation, executor failure, PR creation)
- **THEN** the task emits a log line of the form `"polling iteration failed for <url>: <error chain>"` naming the failed step
- **AND** the task sleeps for `poll_interval_sec` and proceeds to the next iteration
- **AND** other repositories' polling tasks are unaffected (their iterations continue on schedule)

#### Scenario: Mid-iteration dirty workspace alerts via chatops
- **WHEN** `run_pass_through_commits` finds `git status --porcelain`
  non-empty at the start of a pass (after filtering autocoder
  bookkeeping files like `.alert-state.json`) AND auto-recovery
  (see "Dirty workspace auto-recovers mid-iteration") has been
  attempted AND a subsequent dirty check is STILL non-empty
  AND chatops is configured AND `failure_alerts_enabled` is true
- **THEN** autocoder posts a throttled chatops notification under
  `AlertCategory::WorkspaceDirtyMidIteration` naming the repository
  URL and a short excerpt of the porcelain output
- **AND** the iteration returns the existing `Err` ("workspace ... is
  dirty before pass; refusing to proceed: ...")
- **AND** subsequent iterations that produce the same dirty state
  within 24 hours do NOT re-post (the per-category 24h throttle
  suppresses duplicates, matching the existing
  `WorkspaceInitFailure`/`BranchPushFailure`/`PrCreationFailure`
  behavior)

#### Scenario: Mid-iteration dirty workspace without chatops still logs
- **WHEN** the dirty-workspace condition above occurs AND chatops is
  not configured (or `failure_alerts_enabled` is false)
- **THEN** no chatops post is attempted
- **AND** the existing ERROR log line is the operator's sole signal
- **AND** the iteration still returns `Err` and the polling loop
  proceeds to the next sleep

#### Scenario: Dirty-workspace alert clears after recovery
- **WHEN** a subsequent iteration succeeds (workspace no longer
  dirty AND the pass produces commits AND push+PR steps both
  succeed)
- **THEN** the existing on-success `AlertState::clear` call clears
  the `WorkspaceDirtyMidIteration` throttle alongside every other
  category
- **AND** if the workspace becomes dirty again later, the next
  occurrence re-alerts immediately (no leftover suppression)

#### Scenario: Branch-push failure preserves the completed work
- **WHEN** the branch push fails AND the pass had committed one or more changes
- **THEN** the agent branch AND its commits are retained (not reset)
- **AND** a push-block hold is written (per "Branch-push failure preserves completed work via a push-block hold")
- **AND** the throttled `BranchPushFailure` alert still fires before the iteration returns `Err`
- **AND** no destructive branch or workspace reset occurs as a result of the push failure

#### Scenario: PR-creation failure preserves the pushed work
- **WHEN** the PR-creation step fails after a successful push AND the pass had committed one or more changes or issue units
- **THEN** the push-block hold is written recording PR creation as the failed step
- **AND** the throttled `PrCreationFailure` alert still fires before the iteration returns `Err`
- **AND** the next pass does NOT recreate the branch NOR re-run the executor; it retries PR creation only
