# Tasks

OpenSpec: implements the deltas in `specs/orchestrator-cli/spec.md` (the
workspace-mutating-op invariant + bounded-preempt primitive, the busy-marker
serialization clause, the rollback handler's conformance) and
`specs/chatops-manager/spec.md` (the legible preempt acknowledgement).

## 1. Shared preempt-and-acquire helper

- [x] 1.1 In `autocoder/src/control_socket.rs`, add an async helper
  `preempt_and_acquire_busy_marker(state: &ControlState, repo: &RepositoryConfig,
  workspace: &Path) -> Result<PreemptAcquireOutcome, PreemptAcquireError>` (place
  it near `handle_wipe_workspace` at `control_socket.rs:1690`, whose
  `iteration_cancel` / `iteration_drained` lookup it reuses). It returns an enum
  carrying the held `busy_marker::BusyGuard` AND a `preempted_change: Option<String>`
  (the slug of the change the cancelled pass was working, read from the marker
  before preempting), so the caller can both hold the marker for the whole op AND
  emit the preempt acknowledgement.
- [x] 1.2 In the helper, read the marker's currently-worked change BEFORE
  preempting: call `busy_marker::current(&state.paths, workspace,
  executor.busy_marker_stale_threshold_secs())` (`busy_marker.rs:239`) and capture
  `summary.change` (empty → `None`). This is the `preempted_change` returned to the
  caller for the chatops acknowledgement.
- [x] 1.3 Preempt the in-flight pass: look up the repo's `RepoTaskHandle` in
  `state.repo_tasks` (`control_socket.rs:437`) under the briefest lock (mirror the
  lookup at `control_socket.rs:1705-1714`), clone its `iteration_cancel` token (if
  set) AND fire `token.cancel()`. THEN terminate the in-flight executor subprocess
  via the busy-marker sidecar — `busy_marker::read_subprocess_marker(&state.paths,
  workspace)` (`busy_marker.rs:326`) and, on a positive PID, `libc::killpg(pid,
  libc::SIGTERM)` — exactly mirroring `coordinate_with_daemon`'s `--immediate` path
  (`autocoder/src/cli/sync_specs.rs:152-180`). The `iteration_cancel.cancel()` makes
  the pass body drain at its next await; the SIGTERM is what actually stops the
  running child from writing the workspace AND from opening a PR. Skip both steps
  when no `RepoTaskHandle` and no sidecar are present (no pass in flight).
- [x] 1.4 Wait, bounded, for the busy marker file to be released: poll
  `busy_marker::marker_path(&state.paths, workspace).exists()` (mirror
  `sync_specs.rs wait_for_marker_release`, `sync_specs.rs:180`) capped at
  `state.last_config.load_full().executor.wipe_drain_timeout_secs_clamped()` (the
  SAME timeout `handle_wipe_workspace` uses at `control_socket.rs:1721-1725` — do
  NOT add a new config field). Use a short poll interval (≈200ms) so the wait wakes
  promptly once the guard drops.
- [x] 1.5 Acquire the marker: call `busy_marker::try_acquire(&state.paths,
  workspace, &repo.url, stuck_threshold_secs)` (`busy_marker.rs:359`). Map the
  `AcquireOutcome` (`busy_marker.rs:67`): `Acquired(guard)` → return the held guard
  + `preempted_change`; `SkipAmbiguous(_)` → return a `PreemptAcquireError::Busy`
  the caller surfaces as a clear "repo busy with an unrecognized holder; investigate"
  error (do NOT delete/overwrite the marker); `SkipFreshInProgress(_)` after the
  bounded wait → also map to `PreemptAcquireError::Busy` (the prior pass did not
  release in time). The dead-pid-immediate branch inside `try_acquire` already
  recovers a child that exited from the SIGTERM, so the common post-preempt case
  yields `Acquired`.

## 2. Rollback handler conforms

- [x] 2.1 Make the `dry_run` branch of `handle_rollback_recovery` (`autocoder/src/control_socket.rs`) genuinely READ-ONLY so it performs no working-tree mutation (and so neither preempts nor locks honestly). Today the shared preamble runs `git::checkout(base)` + `git::reset_hard_to_remote(base)` BEFORE the dry-run short-circuit, which DOES mutate the working tree and can clobber a concurrent pass. For the dry-run path, replace that with a read-only resolution: `git fetch <remote> <base>` (updates the remote-tracking ref, no working-tree change) THEN resolve the plan against `origin/<base>` — i.e. `resolve_plan` / `format_preview` / `detect_collisions` compute the range from the `origin/<base>` ref, with NO checkout and NO reset. This likely means `resolve_plan` (and friends) take the ref to resolve against, so the live path keeps using its checked-out clean base while the dry-run uses `origin/<base>`. The dry-run then neither preempts nor acquires the marker — truthfully, because it now mutates nothing.
- [x] 2.2 For the live path (after the `dry_run` short-circuit AND after the
  collision check at `:3299-3309`, BEFORE the first workspace mutation —
  i.e. before `git::recreate_branch` at `:3314`), call
  `preempt_and_acquire_busy_marker`. NOTE: the existing preamble at `:3258-3276`
  (`workspace::ensure_initialized`, `git::checkout`, `git::reset_hard_to_remote`)
  ALSO mutates the workspace; on the LIVE path the preempt+acquire runs BEFORE that
  preamble (restructure so the live path acquires the marker, then runs
  ensure_initialized → checkout → reset → resolve → recreate → prepare → push → PR
  under the held guard). The dry-run path no longer shares this mutating preamble —
  per task 2.1 it resolves the plan READ-ONLY against `origin/<base>` without the
  guard (nothing to lock). Bind the returned `BusyGuard` to a variable that stays in
  scope until the handler returns so the marker is held for the whole op AND
  released on every return path (success OR error) via `Drop`.
- [x] 2.3 On `PreemptAcquireError::Busy`, return early with
  `json!({"ok": false, "error": <clear busy message naming the repo>})` and do NOT
  mutate the workspace.
- [x] 2.4 When `preempted_change` is `Some(slug)`, include a structured field in the
  success/early-return JSON response (e.g. `"preempted_change": <slug>`) so the
  chatops dispatcher can render the acknowledgement (section 3). When `None`, omit
  it / set null.

## 3. Chatops preempt acknowledgement

- [x] 3.1 In the rollback chatops dispatch path (the `rollback`-confirm handler in
  `autocoder/src/chatops/operator_commands.rs` — locate the confirmed-rollback arm
  that calls the `rollback_recovery` control-socket action), after receiving the
  control-socket response, if it carries a non-null `preempted_change`, post a
  best-effort threaded acknowledgement naming the operation AND the cancelled change
  (e.g. "preempting in-flight work on `<slug>` to roll back") BEFORE/with the result
  reply. Reuse the existing best-effort notification path the other lifecycle
  messages use; a post failure does not abort.
- [x] 3.2 When the response carries no `preempted_change`, post no preempt
  acknowledgement — only the normal result reply.

## 4. Tests (behavior / state, not message wording)

- [x] 4.1 `control_socket.rs` test (or a `polling_loop/tests` integration test):
  with a busy marker pre-populated for a workspace (PID alive, fresh, a recorded
  `change`), `preempt_and_acquire_busy_marker` fires the iteration cancel + sidecar
  SIGTERM path, waits, acquires, AND returns `preempted_change == Some(<slug>)`.
  Assert via the returned outcome + marker state, NOT log/reply strings. Use the
  busy-marker test fixtures (`crate::testing::test_daemon_paths`,
  `pre_populate_marker`, `pre_populate_subprocess_marker` patterns in
  `busy_marker.rs:765,961`) and the `ProcessOps`/test-injection seam where a real
  SIGTERM is undesirable in CI.
- [x] 4.2 With NO marker present, `preempt_and_acquire_busy_marker` acquires
  directly (no sidecar read, `preempted_change == None`) and returns `Acquired`.
- [x] 4.3 Held-for-whole-op: while the helper's returned `BusyGuard` is in scope, a
  second `busy_marker::try_acquire` for the same workspace yields
  `SkipFreshInProgress`; after the guard drops, a subsequent `try_acquire` yields
  `Acquired`.
- [x] 4.4 Ambiguous marker (PID alive, comm differs → `SkipAmbiguous`): the helper
  returns `PreemptAcquireError::Busy` AND leaves the marker file in place (mirror
  the `acquire_when_ambiguous_skips_and_leaves_file` assertion at
  `busy_marker.rs:895`).
- [x] 4.5 Rollback handler: a live (non-dry-run) `handle_rollback_recovery`
  acquires the busy marker before the first workspace mutation AND releases it on
  return; a `dry_run` invocation acquires no marker. Assert via marker
  presence/absence around the call, not message wording.
- [x] 4.7 Dry-run is read-only (per task 2.1): a `dry_run` `handle_rollback_recovery`
  performs NO working-tree mutation — it does not checkout or reset. Seed the
  workspace with a sentinel uncommitted change (or an agent branch with local work)
  and assert it survives the dry-run unchanged, AND that the dry-run still returns a
  correct plan/preview (resolved against `origin/<base>`). Assert via working-tree
  state, not message wording.
- [x] 4.6 Chatops: the dispatcher posts a preempt acknowledgement iff the
  control-socket response carries a non-null `preempted_change`. Assert by feeding a
  response with/without the field and checking the dispatcher's
  posted-notification calls (the test ActionSubmitter / notification seam), not the
  human sentence.

## 5. Docs

- [x] 5.1 In `docs/` (the operator rollback/recovery reference, wherever the
  code-rollback recovery operator flow is documented), note that a confirmed
  rollback preempts an in-flight pass on that repo — cancelling the current change
  (no PR, tokens stop) — and that the operator is told which change was cancelled.
  Keep the tone dry; no exclamation, no "gotcha".
