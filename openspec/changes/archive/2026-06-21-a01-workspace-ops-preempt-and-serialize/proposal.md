# Workspace-mutating control-socket ops preempt and serialize against the pass

## Why

The code-rollback-recovery handler does all its workspace git surgery (checkout
base, reset, recreate the agent branch, unarchive, `git add -A`, push, PR) with
no coordination with the per-repo pass loop. The pass holds a busy marker; the
rollback handler holds nothing. So a rollback confirmed while a pass is mid-flight
on that repo races the unsandboxed daemon git against the concurrently-running
agentic session, which has the SAME workspace bind-mounted writable. Two writers
on one workspace corrupt the git index: `git add -A` fails with
`fatal: Unable to write new index file`. Observed in production. Waiting for the
in-flight pass is wrong too — a rollback would discard whatever that pass
produces, after paying for its tokens AND leaving an unmergeable PR. The
in-flight work must be PREEMPTED.

## What Changes

- A general invariant (`orchestrator-cli`): a workspace-mutating control-socket
  operation MUST NOT run concurrently with a pass on the same repo, AND MUST
  preempt the in-flight pass rather than wait for it. Order: preempt the
  in-flight pass (so it stops spending tokens AND never opens a PR) → acquire the
  per-repo busy marker → perform the operation → release the marker. The marker
  is held for the whole operation so no new pass can start mid-op.
- The shipped rollback handler (`handle_rollback_recovery`) is made to conform —
  this is a shipped bug, fixed now.
- The preempt is best-effort-but-bounded: signal the in-flight pass, wait
  (bounded) for it to release the marker, then acquire. An ambiguous/stuck marker
  (holding PID dead or PID-reuse-suspected) surfaces clearly rather than barging
  in.
- A legible operator acknowledgement (`chatops-manager`): when a preempt
  cancels in-flight work, the operator is told (e.g. "preempting in-flight work
  on `<slug>` to roll back") so the cancelled change is not a silent surprise.
- A read-only / non-workspace control op (status, list, marker-clear of a
  gitignored marker) is unaffected — the invariant scopes to ops that mutate the
  workspace tree or branch.

## Impact

- Affected specs: `orchestrator-cli` (MODIFY the busy-marker requirement to scope
  it over workspace-mutating control ops; MODIFY the rollback requirement to add
  the preempt+lock clause; ADD a requirement establishing the general invariant
  and the bounded-preempt primitive), `chatops-manager` (ADD the legible
  preempt acknowledgement).
- Affected code: `control_socket.rs` (`handle_rollback_recovery`, a new shared
  preempt+acquire helper), `busy_marker.rs` (`try_acquire` reuse, `AcquireOutcome`),
  the per-repo `RepoTaskHandle` preempt primitive (`iteration_cancel` /
  `iteration_drained` and the subprocess-sidecar SIGTERM path).
- The as-yet-unbuilt `defer`/`undefer` operations (`defer-and-resume-units`) are
  also workspace-mutating; they conform to this invariant when built. This change
  does not implement them.
