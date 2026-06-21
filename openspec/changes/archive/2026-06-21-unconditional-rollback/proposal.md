# Make the confirmed code-rollback forceful and unconditional

## Why

Rollback is the operator's emergency override: once confirmed, it must stop
whatever the daemon is doing and produce the rolled-back result, resolving repo
state itself — the operator does not hand-clean the workspace. Today a confirmed
rollback fails in two observed-in-production ways that violate this. It
fails-closed with "still busy after the preempt wait" when an in-flight pass is
stuck and does not release the busy marker in time. And it ABORTS with a
COLLISIONS error when an in-range unit it would unarchive already has an active
directory. Both leave the operator with no rollback and a repo to fix by hand.

This change makes the CONFIRMED rollback complete the job: it always preempts the
in-flight work (escalating to a forced reclaim) and always reconciles repo state
to the rollback target. It still rides the push + PR flow and still requires the
operator's confirmation — forcefulness is about finishing after confirmation, not
skipping review or confirmation.

## What Changes

- **Forceful preempt — never "busy".** For a confirmed rollback, the
  preempt-and-acquire escalates instead of failing `Busy`: cancel the iteration,
  SIGTERM the executor process group, bounded-wait for the marker, and if the
  marker is still held, SIGKILL the process group AND forcibly reclaim/clear the
  busy marker, then acquire. The rollback always ends up holding the workspace;
  it never returns a "still busy" error. The escalation reuses the busy-marker
  stuck-recovery reclaim (SIGTERM → wait → SIGKILL → clear → acquire), not a new
  kill path.
- **Reconcile, don't refuse, on collisions.** A confirmed rollback no longer
  aborts when an in-range unit it would unarchive already has an active directory.
  It reconciles to the target state: each in-range change/issue ends up
  ACTIVE/pending exactly once with its canon fold undone, and any redundant
  duplicate (e.g. a stale archived copy alongside the active dir) is resolved so
  the result matches the rollback target.
- **Reuse an existing agent-branch PR — never a 422.** When a PR already exists
  for the agent branch (e.g. from the preempted pass, or a prior pass), the
  rollback's force-push updates its head; the rollback reuses AND retitles it (via
  a new forge PR-update API) instead of a raw create that 422s and leaves a
  Frankenstein PR with a stale, unrelated title.
- **Never commit build output.** Register build-output paths (`target/`) in the
  workspace-local `.git/info/exclude` at init, so `git add -A` never stages them
  even after the rollback DELETES the repo's `.gitignore` (restoring to a target
  that predates it). Fleet-wide commit hygiene, not rollback-only.
- **End-to-end test.** A test drives a REAL confirmed rollback through the
  combined adversarial state (stuck pass + collision + existing PR +
  `target/`-with-no-`.gitignore`) and asserts a clean, correct PR — the
  integration the feature never had.
- The DRY-RUN/preview path is unchanged: read-only, no preempt, no lock, and it
  MAY still REPORT detected duplicates/collisions as informational so the
  operator sees the state before confirming.

## Impact

- Affected specs: `orchestrator-cli` — MODIFY the `Code-rollback recovery rolls
  back code while unarchiving its specs and issues` requirement (forceful reclaim
  instead of fail-Busy; reconcile instead of abort on collision), and MODIFY the
  `Workspace-mutating control-socket operations preempt and serialize against the
  pass` invariant to record that a confirmed destructive op (rollback) escalates
  to a forced reclaim rather than failing `Busy`.
- Affected code: `control_socket.rs` (`handle_rollback_recovery`, and a forceful
  variant of the `preempt_and_acquire_busy_marker` path that escalates to the
  busy-marker stuck-recovery reclaim); `rollback.rs` (`prepare_rolled_back_tree`
  and a reconcile path replacing the collision abort; `detect_collisions` and
  `format_preview` retained for the informational dry-run report); `busy_marker.rs`
  (a single shared SIGTERM → SIGKILL → clear reclaim helper used by both the
  stuck-recovery branch AND the rollback escalation); `pr_open.rs` +
  `forge/{mod,github,gitlab}.rs` (reuse + retitle an existing agent-branch PR via a
  new `Forge::update_pr`, instead of a raw create that 422s); AND `agentic_run.rs` +
  `git.rs` + `workspace.rs` (register build-output excludes — `target/` — in the
  workspace-local `.git/info/exclude` at init AND in `add_all`).
- The polite preempt other workspace-mutating ops (defer/undefer) use is
  unchanged — only the confirmed, operator-acknowledged destructive rollback
  escalates to the forced reclaim.
