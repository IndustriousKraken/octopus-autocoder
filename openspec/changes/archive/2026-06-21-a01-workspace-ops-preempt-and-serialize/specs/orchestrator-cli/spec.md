# orchestrator-cli (delta)

OpenSpec: https://github.com/Fission-Labs/openspec

## ADDED Requirements

### Requirement: Workspace-mutating control-socket operations preempt and serialize against the pass
A control-socket operation that mutates a repository's workspace tree OR branch (an "out-of-band workspace op") SHALL NOT run concurrently with that repository's polling pass, AND SHALL preempt an in-flight pass rather than wait for it. The operation SHALL hold the per-repo busy marker for its entire duration so no new pass can start while it runs.

The ordering SHALL be: (1) preempt the in-flight pass — signal it to stop so it stops spending tokens AND never opens a pull request; (2) wait, bounded, for the per-repo busy marker to be released; (3) acquire the busy marker; (4) perform the operation; (5) release the marker (on success OR failure). When no pass is in flight, the operation SHALL skip the preempt step, acquire the marker, AND proceed.

The preempt SHALL stop the in-flight executor subprocess (not merely ask the pass body to drain at its next await point): the operation SHALL terminate the running executor child via the busy-marker subprocess sidecar (read the sidecar PID, send `SIGTERM` to its process group), the same mechanism the `--immediate` spec-rebuild coordination uses. A preempted executor is classified ABORTED, not failed, AND produces no PR. The operation's own clean-base preamble (`checkout <base_branch>` + `reset --hard origin/<base_branch>` + recreate the agent branch) cleans up whatever the cancelled session left behind, so a dirty post-preempt workspace is acceptable AND requires no extra cleanup step.

The preempt-and-acquire SHALL be best-effort-but-bounded: the wait for the marker to release SHALL be capped by `executor.wipe_drain_timeout_secs` (the SAME single configurable preempt/drain timeout the wipe-workspace drain uses — no new per-operation knob). If the busy marker is ambiguous (its holding PID is alive but PID-reuse is suspected, the busy marker's `SkipAmbiguous` classification), the operation SHALL surface a clear error to the operator rather than barging in — it SHALL NOT delete or overwrite an ambiguous marker. A marker whose holding PID is dead, OR a marker that releases within the bound, SHALL be acquired normally.

A read-only OR non-workspace control-socket operation SHALL NOT preempt a pass AND SHALL NOT acquire the busy marker: `status` (a read-only marker peek), `list`, AND marker-clear of a gitignored state-file marker never touch the git tree AND never collide with the executor child's workspace writes, so they run without coordination.

The currently-running operations that mutate the workspace tree/branch SHALL conform to this invariant; the code-rollback recovery operation conforms (see "Code-rollback recovery rolls back code while unarchiving its specs and issues"). Any future control-socket operation that mutates the workspace tree or branch inherits this invariant.

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
- **WHEN** a workspace-mutating operation attempts to acquire the busy marker AND the marker is classified ambiguous (holding PID alive, PID-reuse suspected)
- **THEN** the operation does NOT delete or overwrite the marker AND does NOT mutate the workspace
- **AND** it returns a clear error the operator sees, naming that the repository is busy with an unrecognized holder requiring investigation

#### Scenario: Read-only and marker-clear operations do not preempt or lock
- **WHEN** a read-only operation (`status`, `list`) OR a marker-clear of a gitignored state-file marker is invoked on a repository whose pass is mid-flight
- **THEN** the operation runs without preempting the pass AND without acquiring the busy marker
- **AND** the in-flight pass continues uninterrupted

## MODIFIED Requirements

### Requirement: Per-repo busy marker prevents concurrent work
autocoder SHALL acquire a per-repo busy marker file at the start of each polling iteration and hold it through every stage of the pass (executor invocation, commit, review, push, PR creation). The marker lives at `<runtime_dir>/busy/<workspace-basename>.json` (resolved per the daemon's path resolver) and is created atomically via POSIX `O_EXCL`. Its presence prevents any other autocoder pass — same daemon or different — from concurrently working on the same repo. Crashes that bypass normal release (SIGKILL, segfault, host power loss, daemon restart mid-iteration) leave the marker behind for the next pass to detect and recover from. Stuck-state recovery SHALL prefer the subprocess-sidecar PGID (set by the executor after spawning Claude) over the marker's own `pgid` field when sending kill signals.

The busy marker SHALL also be the serialization point for out-of-band workspace mutation: a control-socket operation that mutates the workspace tree OR branch SHALL acquire AND hold the same per-repo busy marker for its whole duration (see "Workspace-mutating control-socket operations preempt and serialize against the pass"), so a pass AND an out-of-band workspace op can never write the same workspace concurrently. A read-only OR non-workspace control-socket operation SHALL NOT acquire the marker.

The stale-threshold SHALL be a dedicated `executor.busy_marker_stale_threshold_secs` config field (default `600` seconds, max `7200` with WARN-and-clamp), NOT a derived value from `executor.timeout_secs`. Raising the executor timeout for legitimately long work SHALL NOT proportionally delay stale-marker recovery on unrelated iterations.

Dead-pid recovery (the `Stuck threshold exceeded, PID dead` scenario below) SHALL fire IMMEDIATELY when the marker's recorded `pid` no longer exists in `/proc`, without waiting for the stale-threshold to elapse. A pid that no longer exists cannot be doing legitimate work; the marker is unambiguously stale the moment that's true.

The "busy marker present; skipping iteration" INFO log line SHALL include the marker's age, the resolved `busy_marker_stale_threshold_secs`, the PID-alive state, AND a `recovery_eligible` boolean computed as `!pid_alive || age >= threshold`. Operators reading `journalctl` can see the marker's recovery state inline without reading the marker file separately.

At daemon startup, after resolving both `executor.timeout_secs` AND `executor.busy_marker_stale_threshold_secs`, the daemon SHALL log one INFO line naming both resolved values. If the new threshold field was NOT explicitly set in config AND the pre-spec implicit formula (`timeout_secs + 600`) would have produced a longer threshold, an additional INFO line SHALL name the gap so operators migrating from the pre-spec behavior see the change.

#### Scenario: Acquire on a clean repo
- **WHEN** a polling iteration begins AND no marker file exists at the resolved `<runtime_dir>/busy/<workspace-basename>.json`
- **THEN** the daemon creates the marker via `OpenOptions::new().write(true).create_new(true).open(path)` (atomic against concurrent daemons)
- **AND** the marker contains a JSON document with fields `repo_url`, `pid` (this process's PID), `pgid` (this process's process group ID), `comm` (the value of `/proc/<pid>/comm` at acquire time, on Linux; empty string on other platforms), `started_at` (RFC 3339 UTC timestamp), `stage` (initially `"executor"`), AND `change` (the slug of the change currently being worked, updated as work progresses; empty at initial acquire) — the field the preempt path reads to name the change it cancels
- **AND** the iteration proceeds normally

#### Scenario: Atomic stage transitions
- **WHEN** the iteration moves from one stage to the next (`executor → commit → review → push → pr`)
- **THEN** the daemon updates the marker's `stage` field via a write-to-temp-then-rename sequence so concurrent readers see either the prior stage or the new one, never a partial write
- **AND** stage names are exactly: `executor`, `commit`, `review`, `push`, `pr`

#### Scenario: Release on normal iteration end
- **WHEN** `execute_one_pass` returns (success or any error)
- **THEN** the RAII guard holding the marker drops, and the file is removed
- **AND** the next iteration finds no marker and proceeds normally

#### Scenario: Marker exists, age below stuck threshold
- **WHEN** acquire detects an existing marker AND its `started_at` is less than `executor.busy_marker_stale_threshold_secs` old AND the recorded `pid` is alive in `/proc`
- **THEN** the daemon logs INFO with the enhanced log line including `age`, `threshold`, `pid_alive=true`, `recovery_eligible=false` AND skips this iteration without modifying the marker
- **AND** the polling task continues with its normal sleep + next-iteration cycle

#### Scenario: Stuck threshold exceeded, PID dead
- **WHEN** acquire detects a marker whose recorded `pid` does NOT correspond to a running process (verified via `/proc/<pid>` stat returning `ENOENT`)
- **THEN** the daemon deletes the marker AND the subprocess sidecar file (if present), logs WARN naming the marker's prior contents (so operators see what crashed), AND proceeds to acquire a fresh marker and run the iteration
- **AND** the recovery fires IMMEDIATELY regardless of the marker's age — no age-threshold check applies to this branch
- **AND** this differs from pre-spec behavior, which gated recovery on `age > executor.timeout_secs + 600`, causing repos to remain stuck for up to 100 minutes after daemon restart

#### Scenario: Stuck threshold exceeded, PID alive, comm matches
- **WHEN** acquire detects a marker older than `executor.busy_marker_stale_threshold_secs` AND the recorded `pid` is alive in `/proc` AND the value of `/proc/<pid>/comm` matches the recorded `comm` field (Linux; the comm-check is skipped on non-Linux platforms and the PID liveness check is trusted alone)
- **THEN** the daemon reads the subprocess sidecar file at `<runtime_dir>/busy/<workspace-basename>.subprocess` (if present). If present, the recorded subprocess PID is used as the kill target (its PGID equals its PID because the executor spawns with `process_group(0)`); if absent, the marker's `pgid` field is used as the fallback
- **AND** the daemon sends `SIGTERM` to that process group via `killpg(target_pgid, SIGTERM)`, waits up to 5 seconds for the group to exit, sends `SIGKILL` via `killpg(target_pgid, SIGKILL)` if still alive
- **AND** the daemon deletes the marker AND the subprocess sidecar file, logs WARN with the action taken, attempts to post a chatops alert "repo recovered from stuck state" (best-effort), AND proceeds to acquire a fresh marker and run
- **AND** the iteration proceeds even when no chatops backend is configured

#### Scenario: Stuck threshold exceeded, PID alive, comm differs
- **WHEN** acquire detects a marker older than `executor.busy_marker_stale_threshold_secs` AND the recorded `pid` is alive in `/proc` AND the recorded `comm` field is non-empty AND differs from the live `/proc/<pid>/comm` value
- **THEN** the daemon logs ERROR naming the discrepancy, attempts to post a chatops alert "repo stuck — please investigate" (best-effort), AND SKIPS this iteration without modifying the marker or the subprocess sidecar
- **AND** the marker stays in place for human investigation; the next polling iteration will re-evaluate
- **AND** the iteration is skipped even when no chatops backend is configured (the ERROR log is the operator's only signal in that case)

#### Scenario: Malformed marker JSON
- **WHEN** acquire detects a marker file that cannot be parsed as the expected JSON shape
- **THEN** the daemon logs WARN naming the parse failure, deletes the marker AND the subprocess sidecar (if present), AND proceeds to acquire a fresh one

#### Scenario: Threshold change is independent of `executor.timeout_secs`
- **WHEN** an operator sets `executor.timeout_secs: 5400` AND does NOT explicitly set `executor.busy_marker_stale_threshold_secs`
- **THEN** the resolved threshold is `600` (the default), NOT `6000` (the pre-spec coupled formula)
- **AND** a startup INFO log notes the gap so operators migrating from pre-spec behavior see the change
- **AND** dead-pid markers continue to recover immediately regardless of either value

#### Scenario: Out-of-bounds threshold values are clamped
- **WHEN** an operator sets `executor.busy_marker_stale_threshold_secs: 10000`
- **THEN** the resolved value is `7200` (the max)
- **AND** a WARN log at startup names both the requested and clamped values

#### Scenario: PID-alive check uses `/proc/<pid>` stat
- **WHEN** the classification logic checks whether a pid is alive
- **THEN** the implementation stats `/proc/<pid>` (not signal-0 or other approaches)
- **AND** returns `false` on `ENOENT` (pid does not exist)
- **AND** returns `true` on successful stat
- **AND** on any other error (permission, transient) the implementation treats the pid as "unknown alive" — falling through to the age-based branches rather than incorrectly clearing a possibly-live marker

#### Scenario: Enhanced log line includes age, threshold, pid_alive, recovery_eligible
- **WHEN** any iteration's busy-marker classification produces a "busy marker present; skipping" log line
- **THEN** the line contains `age=<duration>`, `threshold=<duration>`, `pid_alive=<bool>`, AND `recovery_eligible=<bool>` fields
- **AND** the operator can determine from a single log line whether the marker is stale, when recovery will fire, AND whether the pid is alive — without reading the marker file separately

#### Scenario: Out-of-band workspace op holds the marker for its whole duration
- **WHEN** a control-socket operation that mutates the workspace tree OR branch acquires the busy marker AND begins mutating
- **THEN** the marker is held for the operation's whole duration (preempt-acquire through release), so a concurrent polling pass observes it as held AND skips
- **AND** the marker is released on the operation's completion, success OR failure, so the next pass proceeds normally

### Requirement: Code-rollback recovery rolls back code while unarchiving its specs and issues
The orchestrator SHALL provide a recovery operation that rolls a managed repository's CODE back by a chosen depth WHILE preserving the OpenSpec changes AND issues that were archived in the rolled-back range — moving them back to the active lanes rather than discarding them. The motivating case: code that merged WITHOUT being gate-checked is not to be trusted, but the spec/issue work that drove it is sound AND should re-enter the pipeline to be re-implemented under the controls. A plain `git reset`/`revert` cannot express this, because the orchestrator commits the implementation, the archive move, AND the canonical-spec fold together — so reverting the commits would discard the spec work entirely, back to before it existed.

The operation SHALL accept a rollback depth as EITHER a commit count (roll back the last N commits) OR a target commit SHA (roll back to that commit), resolved against the repository's base branch.

The operation SHALL ride the normal push + PR flow rather than force-pushing the base branch directly: it prepares the rolled-back state on the agent branch AND goes through the SAME push + PR-creation path as any change, honoring the per-repo `auto_submit_pr` setting — a pull request the operator reviews AND merges when `auto_submit_pr` is enabled (the default), OR a pushed agent branch with no PR (the `BranchPushedNoPr` outcome) when an installation has set it false. The operation SHALL NOT special-case a force-push to the base branch; it produces reviewable commits through the established flow, AND git history remains the backstop.

Because it mutates the workspace tree AND branch, the operation SHALL conform to the workspace-mutating control-socket invariant (see "Workspace-mutating control-socket operations preempt and serialize against the pass"): before any workspace mutation it SHALL preempt an in-flight pass on the same repository (terminating the executor subprocess so it stops spending tokens AND opens no PR), acquire the per-repo busy marker, AND hold it across the whole rollback (clean-base preamble, agent-branch recreation, tree preparation, push, PR), releasing it on completion (success OR failure). This is what stops the daemon's unsandboxed rollback git from colliding with a concurrently-running agentic session that has the same workspace bind-mounted writable. The dry-run/preview path resolves the plan READ-ONLY — it fetches `origin/<base>` AND computes the rollback range against that ref, performing NO checkout, reset, or other working-tree mutation — AND therefore does NOT preempt OR lock (it changes nothing, so it cannot race a concurrent pass).

Within the rolled-back range, the operation SHALL:

- Restore the CODE (every path outside `openspec/` AND outside the issues lane) to its state at the rollback target — the untrusted implementation is discarded.
- For each OpenSpec change archived in the range, UNARCHIVE it: the change returns to `openspec/changes/<slug>/` (active), its canonical-spec fold is undone, so it is pending again AND will be re-gated AND re-implemented. It is NOT reverted to non-existence.
- For each issue archived in the range, UNARCHIVE it: the issue unit returns from `issues/archive/` to the active `issues/` lane.
- Leave changes/issues archived OUTSIDE the range untouched (still archived, canon intact).

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
