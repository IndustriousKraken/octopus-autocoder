## MODIFIED Requirements

### Requirement: Code-rollback recovery rolls back code while unarchiving its specs and issues
The orchestrator SHALL provide a recovery operation that rolls a managed repository's CODE back by a chosen depth WHILE preserving the OpenSpec changes AND issues that were archived in the rolled-back range — moving them back to the active lanes rather than discarding them. The motivating case: code that merged WITHOUT being gate-checked is not to be trusted, but the spec/issue work that drove it is sound AND should re-enter the pipeline to be re-implemented under the controls. A plain `git reset`/`revert` cannot express this, because the orchestrator commits the implementation, the archive move, AND the canonical-spec fold together — so reverting the commits would discard the spec work entirely, back to before it existed.

The operation SHALL accept a rollback depth as EITHER a commit count (roll back the last N commits) OR a target commit SHA (roll back to that commit), resolved against the repository's base branch.

The operation SHALL ride the normal push + PR flow rather than force-pushing the base branch directly: it prepares the rolled-back state on the agent branch AND goes through the SAME push + PR-creation path as any change, honoring the per-repo `auto_submit_pr` setting — a pull request the operator reviews AND merges when `auto_submit_pr` is enabled (the default), OR a pushed agent branch with no PR (the `BranchPushedNoPr` outcome) when an installation has set it false. The operation SHALL NOT special-case a force-push to the base branch; it produces reviewable commits through the established flow, AND git history remains the backstop.

When a pull request ALREADY exists for the agent branch (e.g. from the in-flight pass the rollback just preempted, OR a prior pass), the rollback's force-push of the agent branch UPDATES that existing PR's head to the rolled-back state. The CONFIRMED rollback SHALL detect the existing agent-branch PR — reusing the SAME agent-branch-PR detection the polling loop's open-PR check uses (`open_pr_exists_for_agent_branch`) — AND reuse it — updating its title AND body to describe the rollback via the forge's PR-update API — rather than calling raw PR-creation, which fails with a `422 — a pull request already exists`. The rollback SHALL create a new PR ONLY when none exists for the agent branch. An existing agent-branch PR is reused, NEVER a blocker — this is the third way (alongside the forceful reclaim AND the collision reconcile) the confirmed rollback always produces its result.

The rollback's commit SHALL contain only the rolled-back source/spec changes, NEVER build-output artifacts. Restoring tracked code to the target can DELETE files absent at the target — including a `.gitignore` added later — while leaving untracked build output (e.g. a Rust `target/` directory the executor's builds produced, an artifact from commits AHEAD of the target) in place; a naive `git add -A` would then STAGE that stale build output once the target's `.gitignore` is gone. To prevent this, autocoder SHALL register build-output paths (at least `target/`) in the workspace-local `.git/info/exclude` at workspace init. Because `.git/info/exclude` is LOCAL to the clone AND is NOT part of the restored tree, it survives the rollback deleting the repository's `.gitignore`, so NO commit — the rollback's OR any pass's — ever stages build output, even when the rolled-back tree carries no `.gitignore`. (This is a fleet-wide commit-hygiene guarantee, not rollback-only; the rollback is merely where the deleted-`.gitignore` case forces it.)

Rollback is the operator's emergency override: a CONFIRMED rollback SHALL be FORCEFUL AND UNCONDITIONAL — it SHALL always preempt the in-flight work AND always produce the rolled-back result, resolving repo state ITSELF rather than requiring the operator to hand-clean the repository. Forcefulness governs only what happens AFTER confirmation: the CONFIRMED rollback STILL rides the push + PR flow (reviewable, not a direct base push) AND STILL requires the operator's confirmation. The two ways the rollback completes the job after confirmation are the forceful reclaim AND the collision reconcile defined below.

Because it mutates the workspace tree AND branch, the operation SHALL conform to the workspace-mutating control-socket invariant (see "Workspace-mutating control-socket operations preempt and serialize against the pass"): before any workspace mutation it SHALL preempt an in-flight pass on the same repository (terminating the executor subprocess so it stops spending tokens AND opens no PR), acquire the per-repo busy marker, AND hold it across the whole rollback (clean-base preamble, agent-branch recreation, tree preparation, push, PR), releasing it on completion (success OR failure). This is what stops the daemon's unsandboxed rollback git from colliding with a concurrently-running agentic session that has the same workspace bind-mounted writable. For a CONFIRMED rollback the preempt-and-acquire SHALL be a FORCEFUL reclaim rather than the polite "fail Busy if the marker is not released in time" the non-destructive workspace-mutating ops use: it SHALL escalate — cancel the iteration, SIGTERM the executor child's process group, bounded-wait, AND if the marker is STILL held (the busy marker's `SkipFreshInProgress`, OR a `SkipAmbiguous` PID-reuse-suspected classification), SIGKILL the process group AND forcibly reclaim/clear the busy marker (and its subprocess sidecar), THEN acquire. The forceful reclaim SHALL reuse the busy-marker's age-based stuck-recovery reclaim (SIGTERM the process group → bounded wait → SIGKILL if still alive → clear the marker file AND the subprocess sidecar → acquire), extracted as a single shared kill-and-clear helper so the stuck-recovery branch AND the confirmed-rollback escalation share ONE mechanism rather than inventing a new kill path. A CONFIRMED rollback SHALL ALWAYS end up holding the per-repo busy marker; it SHALL NOT return a "still busy" error. The forceful reclaim is justified because the operator has explicitly confirmed a destructive op, AND the rollback's own clean-base preamble (`git checkout <base>` + `git reset --hard origin/<base>`) plus agent-branch recreation plus code/canon restore-to-target clean whatever the killed pass left in the workspace, so a dirty post-reclaim workspace is acceptable. The dry-run/preview path resolves the plan READ-ONLY — it fetches `origin/<base>` AND computes the rollback range against that ref, performing NO checkout, reset, or other working-tree mutation — AND therefore does NOT preempt OR lock (it changes nothing, so it cannot race a concurrent pass).

Within the rolled-back range, the operation SHALL:

- Restore the CODE (every path outside `openspec/` AND outside the issues lane) to its state at the rollback target — the untrusted implementation is discarded.
- For each OpenSpec change archived in the range, UNARCHIVE it: the change returns to `openspec/changes/<slug>/` (active), its canonical-spec fold is undone, so it is pending again AND will be re-gated AND re-implemented. It is NOT reverted to non-existence.
- For each issue archived in the range, UNARCHIVE it: the issue unit returns from `issues/archive/` to the active `issues/` lane.
- Leave changes/issues archived OUTSIDE the range untouched (still archived, canon intact).

A CONFIRMED rollback SHALL NOT abort when an in-range unit it would unarchive already has an active directory of the same slug. Instead it SHALL RECONCILE to the TARGET state rather than refuse: each in-range change/issue SHALL end up ACTIVE/pending exactly once with its canon fold undone, AND any redundant duplicate (e.g. a stale archived copy alongside the active dir) SHALL be resolved so the result matches the rollback target. There is no "active work to protect" because the operator confirmed the discard AND the rollback restores the tree to the target. The reconcile SHALL be idempotent: an in-range unit already active in its target form (with no redundant archived copy) requires no move AND is not an error; an in-range unit whose dated archive entry exists alongside an active dir SHALL resolve to a single active copy with the redundant archived entry removed. The DRY-RUN/preview MAY still REPORT detected duplicates/collisions as informational so the operator sees the repo state before confirming; the CONFIRMED run resolves them rather than aborting.

The operation SHALL be fail-loud AND reviewable: the PR body SHALL enumerate the commits rolled back, the changes/issues unarchived, AND state plainly that the code was discarded while the specs/issues were returned to the pipeline. Because it discards code, the operation SHALL require explicit confirmation before it acts (a confirmation prompt for the CLI, OR a two-step confirm for the chatops verb), mirroring the other destructive operator commands. A dry run (default for the CLI, OR an explicit preview) SHALL report exactly what WOULD be rolled back AND unarchived without changing anything.

#### Scenario: Rollback by count discards code and unarchives specs via the normal flow
- **WHEN** an operator rolls a repository back by N commits AND those commits archived one or more changes/issues
- **THEN** it rides the normal push + PR flow (opening a PR when `auto_submit_pr` is enabled, the default; otherwise a pushed branch with no PR), NOT a force-push to base, with the agent branch's tree holding the code restored to the rollback target
- **AND** each change archived in the range is moved back to `openspec/changes/<slug>/` with its canon fold undone (active, to be re-gated and re-implemented)
- **AND** each issue archived in the range is moved back to the active `issues/` lane
- **AND** the PR body enumerates the rolled-back commits AND the unarchived changes/issues

#### Scenario: Rollback to a SHA is equivalent to the count form
- **WHEN** an operator rolls back to a specific commit SHA instead of a count
- **THEN** the same restore-code / unarchive-specs-and-issues operation runs against that target
- **AND** the result is a PR with identical structure to the count form

#### Scenario: Specs and issues archived outside the range are untouched
- **WHEN** the rollback range covers some archived changes/issues but not others
- **THEN** only the changes/issues archived WITHIN the range are unarchived
- **AND** changes/issues archived before the range stay archived AND their canon fold is intact

#### Scenario: Confirmation is required and a dry run changes nothing
- **WHEN** the operation is invoked without confirmation (OR in dry-run/preview mode)
- **THEN** it reports the commits it WOULD roll back AND the changes/issues it WOULD unarchive
- **AND** it resolves the plan against `origin/<base>` READ-ONLY (a fetch plus a range computation) — it does NOT checkout, reset, or otherwise modify any branch, workspace, archive, or canon until the operator confirms
- **AND** because it performs no mutation, the dry-run path does NOT preempt an in-flight pass AND does NOT acquire the busy marker — it cannot disrupt a concurrent pass

#### Scenario: Code-only range is a plain rollback through the normal flow
- **WHEN** the rolled-back range archived NO changes AND NO issues (code-only commits)
- **THEN** the rolled-back state restores the code to the target with no unarchive step AND rides the normal push + PR flow (a PR when `auto_submit_pr` is enabled)
- **AND** the PR body (or push notification) says the rollback was code-only

#### Scenario: Confirmed rollback preempts an in-flight pass before mutating the workspace
- **WHEN** an operator confirms a rollback on a repository whose polling pass is mid-flight on a change
- **THEN** the rollback preempts the in-flight pass — terminating the executor subprocess so it stops spending tokens AND opens no PR — before any workspace mutation
- **AND** the rollback acquires the per-repo busy marker AND holds it across the clean-base preamble, agent-branch recreation, tree preparation, push, AND PR
- **AND** the unsandboxed rollback git does not run concurrently with the agentic session, so the git index is not corrupted (`git add` does not fail to write the index)
- **AND** the marker is released when the rollback completes, success OR failure

#### Scenario: A stuck in-flight pass is forcibly reclaimed, never reported busy
- **WHEN** an operator confirms a rollback AND the in-flight pass does not release the per-repo busy marker within the bounded preempt wait (the marker is still held, OR is classified PID-reuse-suspected)
- **THEN** the rollback escalates to a forced reclaim — it SIGKILLs the held pass's process group AND forcibly clears the busy marker (and its subprocess sidecar) — reusing the busy-marker stuck-recovery reclaim path rather than a new kill path
- **AND** the rollback then acquires the busy marker AND proceeds; it does NOT return a "still busy" error
- **AND** whatever the killed pass left in the workspace is cleaned by the rollback's own clean-base preamble (`reset --hard origin/<base>`), agent-branch recreation, AND code/canon restore-to-target

#### Scenario: A confirmed rollback reconciles an in-range collision instead of aborting
- **WHEN** an operator confirms a rollback AND an in-range unit it would unarchive already has an active directory of the same slug (e.g. a stale archived copy alongside the active dir)
- **THEN** the rollback does NOT abort with a collisions error; it reconciles to the target state so the in-range change/issue ends up ACTIVE/pending exactly once with its canon fold undone
- **AND** the redundant duplicate (the stale archived copy) is resolved so the result matches the rollback target
- **AND** the rollback completes through the normal push + PR flow

#### Scenario: The dry-run preview still reports detected duplicates informationally
- **WHEN** an operator previews a rollback (dry-run) AND an in-range unit's unarchive destination already exists
- **THEN** the preview MAY report the detected duplicate/collision as informational so the operator sees the repo state
- **AND** the preview changes nothing AND does NOT preempt or lock — the report is advisory; the CONFIRMED run is what reconciles it

#### Scenario: An existing agent-branch PR is reused and retitled, not a 422
- **WHEN** an operator confirms a rollback AND a pull request already exists for the agent branch (e.g. the preempted in-flight pass — or a prior pass — had opened one)
- **THEN** the rollback's force-push updates that PR's head to the rolled-back state, AND the rollback reuses the existing PR — updating its title AND body to describe the rollback — rather than calling raw PR-creation
- **AND** it does NOT fail with a `422 — a pull request already exists` error, AND the PR is NOT left with a stale, unrelated title
- **AND** a new PR is created only when none exists for the agent branch

#### Scenario: The rolled-back commit excludes stale build output even when the target has no .gitignore
- **WHEN** a rollback restores code to a target whose tree has no `.gitignore` (it predates that file) AND the workspace holds untracked build output (e.g. `target/`) from commits ahead of the target
- **THEN** the rollback commit does NOT stage the build output — the workspace-local `.git/info/exclude` (independent of the restored tree's `.gitignore`) keeps it out
- **AND** the rollback PR contains only the rolled-back source/spec changes, never the build artifacts

### Requirement: Workspace-mutating control-socket operations preempt and serialize against the pass
A control-socket operation that mutates a repository's workspace tree OR branch (an "out-of-band workspace op") SHALL NOT run concurrently with that repository's polling pass, AND SHALL preempt an in-flight pass rather than wait for it. The operation SHALL hold the per-repo busy marker for its entire duration so no new pass can start while it runs.

The ordering SHALL be: (1) preempt the in-flight pass — signal it to stop so it stops spending tokens AND never opens a pull request; (2) wait, bounded, for the per-repo busy marker to be released; (3) acquire the busy marker; (4) perform the operation; (5) release the marker (on success OR failure). When no pass is in flight, the operation SHALL skip the preempt step, acquire the marker, AND proceed.

The preempt SHALL stop the in-flight executor subprocess (not merely ask the pass body to drain at its next await point): the operation SHALL terminate the running executor child via the busy-marker subprocess sidecar (read the sidecar PID, send `SIGTERM` to its process group), the same mechanism the `--immediate` spec-rebuild coordination uses. A preempted executor is classified ABORTED, not failed, AND produces no PR. The operation's own clean-base preamble (`checkout <base_branch>` + `reset --hard origin/<base_branch>` + recreate the agent branch) cleans up whatever the cancelled session left behind, so a dirty post-preempt workspace is acceptable AND requires no extra cleanup step.

The preempt-and-acquire SHALL be best-effort-but-bounded: the wait for the marker to release SHALL be capped by `executor.wipe_drain_timeout_secs` (the SAME single configurable preempt/drain timeout the wipe-workspace drain uses — no new per-operation knob). The behavior when the marker is STILL held after the bound depends on whether the operation is a CONFIRMED destructive override:

- For a NON-destructive workspace-mutating op (e.g. defer/undefer), the operation SHALL surface a clear `Busy` error to the operator rather than barging in — it SHALL NOT delete or overwrite an ambiguous marker. If the busy marker is ambiguous (its holding PID is alive but PID-reuse is suspected, the busy marker's `SkipAmbiguous` classification), the operation SHALL likewise surface a clear error.
- For a CONFIRMED destructive operator-confirmed op (code-rollback recovery), the operation SHALL ESCALATE to a forced reclaim rather than failing `Busy`: SIGKILL the held pass's process group AND forcibly reclaim/clear the busy marker (and its subprocess sidecar) — reusing the busy-marker stuck-recovery reclaim path — THEN acquire, including past a `SkipAmbiguous` classification. The forced reclaim is authorized by the operator's explicit confirmation of a destructive op, AND the operation's own clean-base preamble + tree restore repairs whatever the reclaimed holder left behind. A confirmed destructive op SHALL ALWAYS end up holding the marker; it SHALL NOT return a "still busy" error.

A marker whose holding PID is dead, OR a marker that releases within the bound, SHALL be acquired normally.

A read-only OR non-workspace control-socket operation SHALL NOT preempt a pass AND SHALL NOT acquire the busy marker: `status` (a read-only marker peek), `list`, AND marker-clear of a gitignored state-file marker never touch the git tree AND never collide with the executor child's workspace writes, so they run without coordination.

The currently-running operations that mutate the workspace tree/branch SHALL conform to this invariant; the code-rollback recovery operation conforms (see "Code-rollback recovery rolls back code while unarchiving its specs and issues") — as the CONFIRMED destructive op, it uses the forced-reclaim escalation. Any future control-socket operation that mutates the workspace tree or branch inherits this invariant.

#### Scenario: Rollback confirmed mid-pass preempts the in-flight pass before mutating the workspace
- **WHEN** an operator confirms a workspace-mutating operation on a repository whose polling pass is mid-flight (busy marker held, holding PID alive)
- **THEN** the operation signals the in-flight pass AND terminates the running executor subprocess via the subprocess sidecar's process group, so the executor stops spending tokens AND opens no PR
- **AND** the operation then waits, bounded by `executor.wipe_drain_timeout_secs`, for the busy marker to be released, acquires it, AND only then begins mutating the workspace
- **AND** the in-flight executor's death is classified ABORTED (not a failure) AND no pull request is opened for the cancelled work

#### Scenario: No pass in flight skips the preempt and acquires directly
- **WHEN** a workspace-mutating operation is invoked AND no busy marker is held for the repository
- **THEN** the operation skips the preempt step, acquires the busy marker, AND performs the operation
- **AND** no preempt acknowledgement is emitted

#### Scenario: The marker is held for the whole operation so no new pass can start
- **WHEN** a workspace-mutating operation is in progress (marker acquired, mid-mutation)
- **THEN** any concurrent attempt by a polling pass to acquire the same repository's busy marker observes it as held (skip-fresh-in-progress) AND does not start
- **AND** the marker is released only after the operation completes (success OR failure)

#### Scenario: Ambiguous held marker surfaces an error instead of barging in
- **WHEN** a NON-destructive workspace-mutating operation attempts to acquire the busy marker AND the marker is classified ambiguous (holding PID alive, PID-reuse suspected)
- **THEN** the operation does NOT delete or overwrite the marker AND does NOT mutate the workspace
- **AND** it returns a clear error the operator sees, naming that the repository is busy with an unrecognized holder requiring investigation

#### Scenario: Read-only and marker-clear operations do not preempt or lock
- **WHEN** a read-only operation (`status`, `list`) OR a marker-clear of a gitignored state-file marker is invoked on a repository whose pass is mid-flight
- **THEN** the operation runs without preempting the pass AND without acquiring the busy marker
- **AND** the in-flight pass continues uninterrupted

#### Scenario: A confirmed destructive op escalates to a forced reclaim instead of failing busy
- **WHEN** an operator confirms a destructive workspace-mutating op (code-rollback recovery) AND the in-flight pass does not release the busy marker within the bounded wait (still held, OR PID-reuse-suspected)
- **THEN** the operation escalates to a forced reclaim — SIGKILL the held pass's process group AND forcibly clear the busy marker (and its subprocess sidecar) — reusing the busy-marker stuck-recovery reclaim path
- **AND** it then acquires the marker AND proceeds; it does NOT return a "still busy" error
- **AND** the forced reclaim does not occur for a non-destructive op, which still surfaces a clear `Busy` error on a stuck or ambiguous marker
