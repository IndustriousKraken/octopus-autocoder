# Tasks: forceful, unconditional confirmed rollback

## 1. Forceful preempt-and-acquire for the confirmed rollback

- [x] 1.1 In `autocoder/src/control_socket.rs`, add a forceful variant of the
  preempt-and-acquire used by the confirmed rollback. The existing
  `preempt_and_acquire_busy_marker` / `preempt_and_acquire_busy_marker_with`
  (control_socket.rs:1877, :1897) map a still-held marker (`try_acquire_with`'s
  `AcquireOutcome::SkipFreshInProgress`, control_socket.rs:1997-2003) AND a
  PID-reuse-suspected marker (`SkipAmbiguous`, control_socket.rs:1992-1996) to
  `PreemptAcquireError::Busy`. Introduce a forceful path (e.g. a `forceful: bool`
  parameter on the `_with` helper, or a sibling `preempt_and_force_acquire…`)
  that, after the existing polite preempt (iteration cancel + `PreemptSignaller`
  SIGTERM + bounded marker-release wait, control_socket.rs:1918-1976), escalates
  on a still-held / ambiguous marker instead of returning `Busy`.
- [x] 1.2 The escalation MUST reuse the busy-marker stuck-recovery reclaim, not a
  new kill path. That reclaim is the stuck branch of
  `busy_marker::try_acquire_with` (autocoder/src/busy_marker.rs:529-548):
  `ops.killpg_terminate(target_pgid)` → `ops.wait_for_exit(wait_pid, 5s)` →
  `if ops.pid_alive(wait_pid) { ops.killpg_kill(target_pgid) }` →
  `remove_file(marker)` + `remove_subprocess_marker(...)` → re-acquire. Drive the
  marker into a forced reclaim for the confirmed rollback regardless of marker
  age (the operator's confirmation, not `stuck_threshold_secs`, is the authority).
  Prefer the subprocess sidecar PID (`busy_marker::read_subprocess_marker`,
  control_socket.rs:1929) as the kill target, falling back to the held marker's
  `pgid`, mirroring the precedence in busy_marker.rs:523-528.
- [x] 1.3 Expose the reclaim primitives so the forceful path can call them through
  the same `busy_marker::ProcessOps` seam (`killpg_terminate`, `wait_for_exit`,
  `killpg_kill`, `pid_alive` — busy_marker.rs:634-640) rather than open-coding
  `libc::killpg`. If the stuck-recovery reclaim is currently inlined inside
  `try_acquire_with`, extract a small reclaim helper in `busy_marker.rs` (e.g.
  `force_reclaim(paths, workspace, target_pgid, wait_pid, ops)`) that both the
  age-based stuck branch AND the confirmed-rollback escalation call, so there is
  exactly one kill-and-clear mechanism.
- [x] 1.4 After the forced reclaim, re-run `busy_marker::try_acquire_with`; a
  cleared marker yields `Acquired`. Guarantee the confirmed rollback's
  preempt-and-acquire has only the terminal outcomes `Acquired` (possibly via the
  escalation) OR `PreemptAcquireError::Internal` (a real filesystem error). It
  MUST NOT return `PreemptAcquireError::Busy`.
- [x] 1.5 In `handle_rollback_recovery` (control_socket.rs:3446), route the
  CONFIRMED path (the `dry_run == false` branch, control_socket.rs:3527-3543) to
  the forceful variant. Leave the `dry_run == true` branch
  (control_socket.rs:3485-3525) exactly as-is — it stays read-only, no preempt,
  no lock. Leave the `_busy_guard` hold-for-whole-op + `Drop`-release contract
  (control_socket.rs:3534-3543) unchanged.
- [x] 1.6 Keep the polite preempt for the non-destructive workspace-mutating ops.
  `handle_defer` / `handle_undefer` (the defer/undefer handlers around
  control_socket.rs:3944-3955) MUST continue calling the polite
  `preempt_and_acquire_busy_marker` that returns `Busy` on a stuck/ambiguous
  marker — only the confirmed rollback escalates.

## 2. Reconcile, don't refuse, on collisions (confirmed path)

- [x] 2.1 In `autocoder/src/rollback.rs`, replace `prepare_rolled_back_tree`'s
  collision abort (rollback.rs:322-334) with a reconcile. Per in-range change
  unit, in the unarchive loop (rollback.rs:352-355): if the active destination
  `openspec/changes/<slug>/` does NOT exist, `queue::unarchive` as today; if it
  EXISTS and the dated archive entry also exists, resolve to a single active copy
  — keep the active dir as the one active unit and remove the redundant dated
  archive entry (so the unit is active exactly once, matching the rollback
  target); if it exists and no dated archive entry remains, it is already in
  target form (no move, no error).
- [x] 2.2 Apply the same reconcile to in-range issue units in the unarchive loop
  (rollback.rs:357-361 / `unarchive_issue`, rollback.rs:414-444). Preserve the
  single-file vs directory form using the existing `ArchivedUnit::is_file` field
  and `issues::resolve_form` (rollback.rs:288, :431): a single-file archive
  reconciles against `issues/<slug>.md`, a directory archive against
  `issues/<slug>/`.
- [x] 2.3 Keep `detect_collisions` (rollback.rs:270-302) AND the collision
  section of `format_preview` (rollback.rs:502-511) — they remain the
  informational dry-run/preview report. Do NOT have the CONFIRMED path call
  `detect_collisions` as a fail-loud gate.
- [x] 2.4 In `handle_rollback_recovery`, remove the confirmed-path collision
  abort (control_socket.rs:3580-3592): the `detect_collisions` →
  `{"ok": false, "error": "rollback aborted: … collide …"}` block. The confirmed
  path proceeds to `recreate_branch` + `prepare_rolled_back_tree`
  (control_socket.rs:3594-3602), which now reconciles. The dry-run path's
  `has_collisions` report (control_socket.rs:3513, :3523) is retained.
- [x] 2.5 Ensure the reconciled tree is staged + committed exactly as the
  non-colliding path: `prepare_rolled_back_tree` ends with `git::add_all` +
  `git::commit` (rollback.rs:363-367); the reconcile feeds the same commit so the
  result rides the same push + PR flow.

## 3. Tests

- [x] 3.1 In `control_socket.rs` tests (the a01 preempt block around
  control_socket.rs:7337+), add a test that the forceful path escalates: given a
  still-held marker that the polite path would classify `SkipFreshInProgress`,
  the forceful variant fires the reclaim (assert via the injected
  `busy_marker::ProcessOps` mock's `killpg_kill` record + marker cleared) AND
  returns `Acquired`, NEVER `Busy`. Reuse the existing test seams
  (`PreemptSignaller` fake + `MockProcessOps`) — do not signal a real process.
- [x] 3.2 Add a test that the forceful path also reclaims a `SkipAmbiguous`
  (PID-reuse-suspected) marker for the confirmed rollback, while a parallel test
  confirms the POLITE path still returns `PreemptAcquireError::Busy` on the same
  marker (the non-destructive ops' behavior is unchanged). Mirror the existing
  `preempt_on_ambiguous_marker_returns_busy_and_leaves_file` test
  (control_socket.rs:7564).
- [x] 3.3 In `rollback.rs` tests, REPLACE the assertion in
  `collision_with_active_dir_is_reported_not_overwritten` (rollback.rs:863-897)
  that `prepare_rolled_back_tree` errors on collision with one asserting it
  RECONCILES: after `prepare_rolled_back_tree`, the in-range change is active
  exactly once at `openspec/changes/<slug>/`, the redundant dated archive entry
  is gone, and the tree is clean/committed. Add a sibling test for the issue lane
  (both single-file and directory forms).
- [x] 3.4 Add a `rollback.rs` test for the idempotent edge: an in-range unit
  already active in target form with no dated archive entry is a no-op (no move,
  no error) during `prepare_rolled_back_tree`.
- [x] 3.5 Keep `preview_changes_nothing` (rollback.rs:901-932) and the
  collision-in-preview assertion (`format_preview` still emits the `COLLISIONS`
  section) green — the dry-run informational report is retained.

## 4. Validation

- [x] 4.1 Run `cargo test -p autocoder` (or the crate's test command) and confirm
  the new + amended rollback / preempt tests pass and nothing regressed.
- [x] 4.2 Run `openspec validate unconditional-rollback --strict` from the repo
  root and confirm it passes.

## 5. Reuse an existing agent-branch PR (no 422)

- [x] 5.1 In `handle_rollback_recovery`'s confirmed PR step (the
  `open_triage_pull_request` calls at control_socket.rs:3665/4112), detect an
  existing PR for the agent branch BEFORE raw-creating — reuse
  `open_pr_exists_for_agent_branch` (pr_open.rs:459) / a find-PR-number helper. The
  force-push of the agent branch already updates any existing PR's head to the
  rolled-back state; when a PR exists, REUSE it — update its title AND body to the
  rollback via a forge PR-update API (add `Forge::update_pr` / a GitHub PATCH +
  GitLab PUT helper) — instead of calling raw `create_pull_request`,
  which 422s "a pull request already exists". Create a new PR only when none
  exists. This kills the "force-push succeeds, THEN 422 on create, leaving a
  Frankenstein PR with a stale unrelated title (e.g. 'adds OCTOPUS.md')" failure.
- [x] 5.2 Test: a confirmed rollback when an agent-branch PR already exists reuses
  it (no 422; the PR head is the rolled-back state; title/body updated to the
  rollback) — assert via the PR-flow seam / returned outcome, not message wording.

## 6. Never commit build output (workspace-local exclude)

- [x] 6.1 Register build-output paths (at least `target/`) in the workspace-local
  `.git/info/exclude` at workspace init — alongside the existing CLI-artifact +
  marker excludes in `workspace::ensure_initialized` (via `git::ensure_local_excludes`).
  Because `.git/info/exclude` is local to the clone and NOT part of the restored
  tree, it keeps `target/` out of `git add -A` even after the rollback DELETES the
  repo's `.gitignore` (restoring to a target that predates it). This is a
  fleet-wide commit-hygiene fix: it also stops normal-pass and OCTOPUS.md-provisioning
  commits from staging build output in any repo whose tracked `.gitignore` doesn't
  cover it. (`WORKSPACE_CLI_ARTIFACT_EXCLUDES` at agentic_run.rs:445 currently lacks
  any build-output entry.)
- [x] 6.2 Test: with a built `target/` present AND the tree's `.gitignore` absent
  (the rollback-to-greenfield case), `git::add_all` does NOT stage `target/`
  (assert via tracked-files / `git status` after add). Mirror the existing
  `add_all_excludes_untracked_cli_artifacts` test in git.rs.

## 7. End-to-end verification (the gap that let this ship broken)

- [x] 7.1 Add an END-TO-END test that drives a REAL confirmed rollback (not
  mock-isolated pieces) through the ADVERSARIAL state all at once and asserts it
  SUCCEEDS and produces a correct, clean PR: a busy marker held by an in-flight
  pass (forcibly reclaimed), an in-range unit colliding with an active dir
  (reconciled), a pre-existing agent-branch PR (reused), AND a built `target/`
  with no `.gitignore` at the target (build output excluded). Assert the rollback
  ends with: the marker released, the in-range units active-exactly-once with canon
  fold undone, a single agent-branch PR carrying the rolled-back source with a
  rollback title/body and NO `target/`. This is the integration the feature never
  had — every unit was green while no real rollback ever ran end-to-end.
