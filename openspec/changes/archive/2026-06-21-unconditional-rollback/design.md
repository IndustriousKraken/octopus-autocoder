# Design: forceful, unconditional confirmed rollback

## Problem framing

Rollback is the emergency override. The operator's directive: once confirmed, it
stops everything and produces the rolled-back result, resolving repo state itself.
The operator will not hand-clean the repo. Two confirmed-rollback failures
observed in production break that contract:

1. **Fail-Busy on a stuck pass.** The shared preempt primitive
   `preempt_and_acquire_busy_marker` (`autocoder/src/control_socket.rs:1877`)
   does a polite preempt: cancel the iteration, SIGTERM the executor process
   group, bounded-wait for the marker to release, then `try_acquire`. If the
   marker is still held after the wait, `try_acquire_with` returns
   `SkipFreshInProgress`, which the primitive maps to
   `PreemptAcquireError::Busy("repository … is still busy after the preempt wait …
   the prior pass did not release in time")` (control_socket.rs:1997-2003).
   `handle_rollback_recovery` returns that as `{"ok": false, "error": msg}`
   (control_socket.rs:3537-3539). Result: a confirmed rollback that does nothing.

2. **Abort on collision.** `handle_rollback_recovery` computes
   `detect_collisions` and, when non-empty, returns a `rollback aborted: … collide
   with existing active directories` error (control_socket.rs:3583-3592).
   `prepare_rolled_back_tree` ALSO aborts on collisions (rollback.rs:322-334), and
   `queue::unarchive` / `unarchive_issue` each error when the destination already
   exists (queue.rs:755-760; rollback.rs:431-436). Result: a confirmed rollback
   that refuses, leaving the operator to resolve the duplicate by hand.

Both are correct fail-loud defaults for an UNconfirmed, non-destructive op. They
are wrong for a CONFIRMED rollback, where the operator has explicitly elected to
discard code and the rollback's own clean-base preamble + tree restore cleans up
whatever it overrides.

## Decision 1: forceful preempt reuses the busy-marker stuck-recovery reclaim

The escalation must not invent a new kill path. The existing forced reclaim lives
in `busy_marker::try_acquire_with` (`autocoder/src/busy_marker.rs:376`), the
stuck-recovery branch (busy_marker.rs:529-555):

- `ops.killpg_terminate(target_pgid)` — SIGTERM the process group (prefers the
  subprocess sidecar PID over the marker's `pgid`).
- `ops.wait_for_exit(wait_pid, Duration::from_secs(5))` — bounded grace.
- `if ops.pid_alive(wait_pid) { ops.killpg_kill(target_pgid) }` — SIGKILL the
  group if still alive.
- `std::fs::remove_file(&path)` + `remove_subprocess_marker(...)` — clear the
  busy marker AND the subprocess sidecar.
- loop back and re-create the marker file → `Acquired`.

This is the same SIGTERM → wait → SIGKILL → clear → acquire ladder the operator
needs. The forceful preempt drives the busy marker into this branch
DETERMINISTICALLY for a confirmed rollback rather than letting it return
`SkipFreshInProgress`. The existing flow normally reaches the stuck branch only
when `age_secs >= stuck_threshold_secs`; a confirmed rollback must reclaim
regardless of age, because the operator's confirmation, not the marker's age, is
the authority to kill.

Approach: after the existing polite preempt (iteration cancel + SIGTERM via
`PreemptSignaller` + bounded marker-release wait) the rollback acquires the
marker. When acquire yields `SkipFreshInProgress` (marker still held), the
forceful path escalates: it invokes the busy-marker reclaim against the held
marker's kill target — the subprocess sidecar PID when present, else the marker's
`pgid` — issuing SIGKILL to the process group and clearing the marker file + the
subprocess sidecar, then acquires. The escalation reuses
`busy_marker`'s `ProcessOps` reclaim primitives (`killpg_terminate`,
`wait_for_exit`, `killpg_kill`) rather than open-coding `libc::killpg`, keeping a
single kill mechanism and a single test seam.

A confirmed rollback's acquire therefore has only three terminal outcomes:
`Acquired` (held the workspace), an escalated reclaim that ends in `Acquired`, or
an `Internal` filesystem error. It does NOT return `Busy`. The `SkipAmbiguous`
(PID-reuse-suspected) classification — which the polite path refuses to barge past
— is also reclaimed under a confirmed rollback: the operator has authorized a
destructive override, and the rollback's `git reset --hard origin/<base>` +
recreate-branch + tree restore repairs whatever the reclaimed holder left behind.

The escalation is scoped to the destructive, operator-confirmed rollback. The
polite preempt other workspace-mutating ops use (defer/undefer, which preserve
content and need only an acknowledgement) is unchanged: they still surface `Busy`
on a stuck marker. This is the clean distinction the a01 invariant draws between
"preempt and wait, bounded" and "forced reclaim after confirmation".

Justification for force: the operator has explicitly confirmed a destructive op,
AND the rollback's own clean-base preamble (`git checkout <base>` + `git reset
--hard origin/<base>`), agent-branch recreation, and code/canon restore-to-target
clean whatever the killed pass left in the tree. A dirty post-kill workspace is
acceptable, exactly as the a01 invariant already states for the polite preempt.

## Decision 2: reconcile to target instead of aborting on collision

A "collision" is an in-range unit whose unarchive destination
(`openspec/changes/<slug>/` or `issues/<slug>(.md|/)`) already exists. In the
production case this is a stale archived copy sitting ALONGSIDE an active dir of
the same slug — both present at the base tip the rollback restores against.

For a confirmed rollback the target state is unambiguous: the in-range
change/issue ends up ACTIVE/pending exactly once with its canon fold undone. There
is no "active work to protect" because the operator confirmed the discard.

Reconcile rule applied during `prepare_rolled_back_tree`, per in-range unit:

- If the active destination does NOT exist: unarchive as today (move the dated
  archive entry → active).
- If the active destination already exists AND the dated archive entry also
  exists: resolve to one active copy — keep the active destination as the single
  active unit and remove the redundant dated archive entry, so the unit is active
  exactly once and no stale archived duplicate remains. (Code and canon are
  already restored to the target by the preceding restore steps, so the active
  copy is the target-state unit.)
- If the active destination exists AND no dated archive entry exists: the unit is
  already active in target form — no move, no error.

The reconcile makes the result idempotent and target-matching regardless of the
pre-existing duplicate shape. Because the daemon has already `reset --hard
origin/<base>` and recreated the agent branch, the working tree is the base tip's
tree before restore; the reconcile operates on that committed shape, then the
result is staged and committed on the agent branch exactly as the non-colliding
path is. The single-file vs directory issue form is preserved (the existing
`is_file` field and `issues::resolve_form` already distinguish them).

`detect_collisions` and `format_preview`'s collision section are RETAINED — the
dry-run/preview still reports detected duplicates/collisions as informational so
the operator sees the repo state before confirming. What changes is the CONFIRMED
path: it reconciles rather than aborting. The confirmed-path collision abort in
`handle_rollback_recovery` (control_socket.rs:3583-3592) and
`prepare_rolled_back_tree` (rollback.rs:322-334) are replaced by the reconcile.

## What stays unchanged

- The push + PR flow, honoring `auto_submit_pr` (PR by default, `BranchPushedNoPr`
  when false) — `handle_rollback_recovery` lines 3604-3689.
- The operator confirmation step — chatops `rollback` → `rollback-confirm`
  (operator_commands.rs:1307-1361) and CLI `--confirm` (cli/rollback.rs).
- a01's read-only dry-run: `dry_run` short-circuits before any preempt or lock
  (control_socket.rs:3485-3525); it neither preempts nor reclaims.

## Relationship to a01

a01 introduced "Workspace-mutating control-socket operations preempt and serialize
against the pass" (polite preempt → bounded wait → fail `Busy` if not released)
and the rollback requirement that conforms to it. This change STRENGTHENS the
confirmed rollback specifically to a forced reclaim — distinct from the polite
preempt the non-destructive ops keep. The a01 invariant is MODIFIED only to record
that a confirmed destructive op (rollback) escalates to a forced reclaim rather
than failing `Busy`; the polite-preempt behavior for non-destructive ops is left
intact, and every existing a01 scenario is preserved.
