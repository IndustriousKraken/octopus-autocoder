# Design

## The race, grounded

`handle_rollback_recovery` (`autocoder/src/control_socket.rs:3230`) mutates the
workspace directly on the daemon thread: `git::checkout` + `git::reset_hard_to_remote`
(`control_socket.rs:3266-3276`), `git::recreate_branch` (`:3314`),
`crate::rollback::prepare_rolled_back_tree` (`:3317`), then push + PR. All of these
shell out to `git` via `git.rs run_git` (`autocoder/src/git.rs:19`) /
`git::add_all` (`git.rs:279`) against the repo's resolved workspace — UNSANDBOXED,
on the daemon's own process.

A polling pass runs concurrently per repo (`polling_loop::run_with_hooks`,
`autocoder/src/polling_loop/mod.rs:438`). The pass invokes the agentic executor
subprocess, which is launched with the SAME workspace bind-mounted WRITABLE: the
sandbox plan's `workspace_writable` field drives a bwrap `--bind` (rw) vs
`--ro-bind` (ro) of the workspace (`autocoder/src/sandbox.rs:651`, also `:496`,
`:722`, `:771`). For the executor role `workspace_writable == true`, so the child
can write the tree.

When a rollback is confirmed mid-pass, the daemon's `git add -A` and the executor
child both write the same `.git/index` → `fatal: Unable to write new index file`.
Observed: confirming a rollback while the repo was working change `a03`.

## What serializes a pass today, and what the rollback handler skips

The pass acquires a per-repo busy marker at iteration start and holds it across
executor → commit → review → push → PR (`busy_marker::try_acquire`, used at
`autocoder/src/polling_loop/pass.rs:29`). Its presence makes any other pass skip
(`AcquireOutcome::SkipFreshInProgress`). The marker records the holding PID; the
RAII `BusyGuard` removes the file on drop. The acquire classification
(`busy_marker.rs:376`) yields `Acquired` / `SkipFreshInProgress` / `SkipAmbiguous`
(`busy_marker.rs:67`). The rollback handler acquires NONE of this — that is the bug.

The marker is the right serialization point: it is the single per-repo "someone is
mutating this workspace" token that both passes and recovery already respect.

## Is there an existing per-repo preempt primitive? Yes — two halves, neither sufficient alone.

There is NO single "abort the current pass cleanly" call, but the two mechanisms it
would compose from both exist:

1. **Drain coordination (`iteration_cancel` / `iteration_drained`).** Each
   `RepoTaskHandle` (`control_socket.rs:275`) carries an `iteration_cancel:
   Arc<Mutex<Option<CancellationToken>>>` and an `iteration_drained: Arc<Notify>`
   (`control_socket.rs:384`, `:390`). The pass loop installs a child cancel token
   at iteration start and, via `IterationGuard::drop` (`polling_loop/mod.rs:359`),
   clears it AND fires `iteration_drained` on every exit path. `handle_wipe_workspace`
   (`control_socket.rs:1690`) already USES this: it fires `iteration_cancel.cancel()`
   then awaits `iteration_drained` with a bounded timeout
   (`executor.wipe_drain_timeout_secs`) before deleting the directory.

   LIMITATION: firing `iteration_cancel` does NOT terminate the in-flight executor
   subprocess. The executor's run is select-and-killed only against its own TIMEOUT
   (`agentic_run.rs:1557`, `:1687`); it does not select on any cancellation token.
   The pass body likewise does not propagate the per-iteration token into the
   executor (`pass.rs:242` even constructs a fresh `CancellationToken::new()` for
   the revision dispatcher). So `iteration_cancel` makes the NEXT await-point in the
   pass body observe cancellation, but a long-running executor child keeps writing
   the workspace until it exits on its own.

2. **Subprocess SIGTERM via the busy-marker sidecar.** The executor writes its
   child PID to a sidecar (`busy_marker::write_subprocess_marker`,
   `busy_marker.rs:161`; child spawned in its own process group via
   `process_group(0)`, so PGID == PID). `coordinate_with_daemon`
   (`autocoder/src/cli/sync_specs.rs:144`) already terminates a running executor
   this way: read the sidecar PID, `killpg(pid, SIGTERM)`, then wait for the marker
   to release (`sync_specs.rs:157-187`). The busy-marker stuck-recovery path does
   the same SIGTERM→wait→SIGKILL sequence (`busy_marker.rs:523-548`).

   This is the half that actually stops the child from writing the workspace AND
   stops it spending tokens. A SIGTERM-killed executor is classified as ABORTED, not
   a failure, and produces NO PR (`executor/claude_cli.rs:899-924`) — which is
   exactly the "never opens a PR" requirement.

### Decision: compose a shared preempt primitive from the two existing halves; do not invent a third.

The change ADDs ONE new shared helper (placement: `control_socket.rs`, callable by
every workspace-mutating handler) that:

1. Looks up the repo's `RepoTaskHandle` and, if present, fires `iteration_cancel`
   AND signals the executor child via the busy-marker subprocess sidecar
   (`read_subprocess_marker` → `killpg(pid, SIGTERM)`), mirroring
   `coordinate_with_daemon`'s `--immediate` path. This is the "stop tokens, never
   open a PR" step.
2. Waits, bounded, for the busy marker file to be released (the same
   "wait_for_marker_release"-shaped poll `coordinate_with_daemon` uses, bounded by a
   timeout — reuse `executor.wipe_drain_timeout_secs` so there is ONE configurable
   preempt/drain timeout, not a new knob).
3. Acquires the busy marker via `busy_marker::try_acquire`. On `Acquired`, hold the
   `BusyGuard` for the whole operation and proceed. On `SkipAmbiguous` (holding PID
   alive but PID-reuse-suspected) the helper does NOT barge in — it returns a clear
   error the handler surfaces to the operator. A dead-PID or released marker is
   acquired normally (the existing `try_acquire` dead-pid-immediate recovery
   handles a child that already exited).

The operation's own `checkout base` + `reset --hard origin/<base>` +
`recreate_branch` clean up whatever the cancelled session left behind, so a dirty
post-cancel workspace is fine — no extra cleanup step is required.

### Why preempt, not wait

If we waited for the in-flight pass to finish, the rollback would then discard
exactly the code that pass just produced — after paying for its tokens — AND that
pass would have opened a PR that the rollback makes unmergeable. There is no case
where a rollback wants the in-flight change to complete. Preempt is the only
correct order.

## Scope of the invariant

WORKSPACE-MUTATING control-socket operations: rollback (fixed now), and the
as-yet-unbuilt defer/undefer (`defer-and-resume-units`), which mutate the
workspace tree + branch and ride the same agent-branch + PR mechanism. The
invariant is stated generally over "a control-socket operation that mutates the
workspace tree or branch" so any future such operation inherits it; it is made
concrete on the shipped rollback handler here.

OUT of scope: read-only / non-workspace ops — `status` (read-only marker peek via
`busy_marker::current`, `busy_marker.rs:239`), `list`, and marker-clear of a
gitignored marker (clears a state file, never touches the git tree). These never
collide with the executor child's workspace writes and MUST NOT preempt a pass.

## Why reuse `wipe_drain_timeout_secs` rather than a new timeout

`handle_wipe_workspace` already drains an in-flight iteration with this exact
timeout for the same reason (do not yank the workspace out from under a live
child). A rollback's preempt is the same shape of bounded drain. The fleet-ops
direction is ONE configurable gate/preempt/drain timeout, not a proliferation of
per-operation knobs, so this change reuses the existing
`executor.wipe_drain_timeout_secs` rather than adding a sibling.

## Operator legibility

When a preempt actually cancels in-flight work (a pass was in flight and a change
was being worked), the operator receives an acknowledgement naming the cancelled
change before the operation proceeds, e.g. "preempting in-flight work on `<slug>`
to roll back". When no pass is in flight, no preempt message is emitted — the
operation just acquires and proceeds. The currently-worked change slug is already
recorded on the marker (`busy_marker.rs:191` `update_change`, surfaced by
`current`) so the acknowledgement can name it.

## Tests (behavior, not message wording)

- A workspace-mutating handler invoked while a busy marker is held (PID alive,
  fresh) preempts: it signals the in-flight pass, then acquires the marker, then
  performs the op — asserted via marker state transitions and the handler outcome,
  NOT log/reply strings.
- The same handler invoked with no marker present acquires directly and performs
  the op with no preempt step.
- The handler holds the marker across the whole op: a concurrent `try_acquire`
  during the op yields `SkipFreshInProgress`.
- An ambiguous held marker (`SkipAmbiguous`) makes the handler return an error
  outcome rather than proceeding.
- The chatops preempt acknowledgement is exercised by asserting the dispatcher
  surfaces a preempt-occurred signal from the control-socket response (a structured
  field), not by asserting the human sentence.
