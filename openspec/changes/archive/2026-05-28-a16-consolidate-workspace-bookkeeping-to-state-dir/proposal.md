## Why

`.alert-state.json` is a daemon-bookkeeping file that the throttled-alerts requirement places at the workspace root. The current implementation matches that requirement, AND the result is a category of recurring operator-visible failures:

- The file is daemon-written every time an alert state changes. Git sees this as a workspace modification — even when the file isn't tracked — causing `git checkout` (in the `recreate_branch` step) to abort with `Your local changes ... would be overwritten by checkout` for tracked-but-locally-modified files OR `untracked working tree files would be overwritten` for the untracked case. Either way, the iteration fails.
- The `architecture_consultative` audit's post-hoc `WritePolicy::None` check (`git status --porcelain` after the audit returns) sees the daemon's own write of `.alert-state.json` as a violation, fires a chatops alert AND reverts the workspace via `git reset --hard HEAD`. The audit didn't actually violate anything — the daemon did.
- A `filter_alert_state_lines` helper exists in the polling loop specifically to strip this file from porcelain output, a workaround that requires every git-aware daemon path to know about the exception. `git checkout` itself doesn't run through that filter, so the workaround only patches the daemon's internal checks AND not the cases that bit the operator in production.
- The canonical spec is internally contradictory: requirement `Daemon resolves four standard data-category paths` lists "alert throttles" under `state_dir`, but the `Throttled predictable-failure alerts` requirement places `.alert-state.json` in the workspace. The two parts of the spec disagree.

The principled fix is the one the operator articulated: daemon state belongs to the daemon's data directories, NOT to managed repositories' workspaces. Code goes in the repo; state-tracking is part of the software AND belongs elsewhere. This spec moves `.alert-state.json` out of the workspace entirely into `<state_dir>/alert-state/<workspace-basename>.json`.

This spec is intentionally scoped to alert-state only. `.audit-state.json` AND `.failure-state.json` are working correctly today via the workspace-exclude pattern AND aren't tripping any operator-visible bugs. They can be moved by follow-up specs if the operator wants the same consolidation extended.

## What Changes

**`.alert-state.json` moves from `<workspace>/.alert-state.json` to `<state_dir>/alert-state/<workspace-basename>.json`.** The schema is unchanged (per-category `last_alerted_at` + `last_error_excerpt`). The `<workspace-basename>` is the same sanitized URL form used everywhere else.

**Every read AND write of alert-state SHALL route through the `DaemonPaths` resolver's new `alert_state_path(workspace_basename) -> PathBuf` helper.** Per `a09`'s state-path-resolution rule, no hard-coded `/tmp/autocoder/...` literals. The `path_literals_audit` CI test introduced in `a09` covers this automatically.

**The `filter_alert_state_lines` helper becomes a no-op AND its callers continue to call it for backward compatibility.** The helper exists as a defensive no-op in case any operator's workspace still has a stale `.alert-state.json` after the migration (e.g., on a fresh re-clone of a repo whose history transiently included it). Future specs can remove the helper entirely after a verification window; for now, defensive call sites stay.

**First-startup migration.** On the first daemon start after this spec ships, for each configured repository AND for each detected workspace, the daemon SHALL:

1. Check whether `<workspace_root>/.alert-state.json` exists. If not, no migration needed.
2. If it exists AND `<state_dir>/alert-state/<workspace-basename>.json` does NOT exist, move the file (`fs::rename` if same filesystem, else copy + delete).
3. If both exist, prefer the state_dir version (more recently authoritative AND survived any prior workspace wipes); delete the workspace copy.
4. `rm` the workspace file even if the file was tracked by git (call `git rm --cached <workspace>/.alert-state.json` first if the file is in the index; commit + push the removal with subject `chore: untrack .alert-state.json (now stored in daemon state dir per a16)`).
5. The migration is per-workspace AND idempotent. A migration marker `<state_dir>/alert-state/.migration-from-workspace-done` (single file, daemon-wide) records that the scan ran AND prevents subsequent scans from re-attempting work.

**Migration interacts with `git rm --cached` carefully.** If the file is tracked AND the repository's base branch has branch protection that forbids direct push by the daemon, the migration fails for that repository — the daemon logs ERROR naming the operator action required (manual `git rm --cached` + commit on a PR branch) AND continues processing other repositories. The migration marker is NOT set in this case, so subsequent daemon starts retry.

**The workspace-init step no longer registers `.alert-state.json` in `.git/info/exclude`.** It never did (per the operator's diagnostic showing the exclude list contains only the four other state files). No code change here; just clarifying that the workspace shouldn't see the file at all post-migration. `filter_alert_state_lines`'s defensive no-op handles the transient window during a re-clone.

## Impact

- **Affected specs:**
  - `orchestrator-cli` — one MODIFIED requirement (`Throttled predictable-failure alerts`) — every reference to the workspace-root path becomes the state-dir path. All 7 existing scenarios preserved verbatim with path references updated.
  - `orchestrator-cli` — one ADDED requirement: `.alert-state.json migration from workspace to state-dir on first startup`.
  - `workspace-manager` — no MODIFIED requirement (workspace-init doesn't currently register alert-state in exclude; nothing to change).
  - `project-documentation` — one ADDED requirement: `OPERATIONS.md and STATE-LAYOUT.md document the alert-state migration AND new path`.
- **Affected code:**
  - `autocoder/src/paths.rs` (or wherever `DaemonPaths` lives) — add `pub fn alert_state_path(&self, workspace_basename: &str) -> PathBuf { self.state_dir().join("alert-state").join(format!("{workspace_basename}.json")) }`.
  - `autocoder/src/chatops/alert_state.rs` (or wherever alert-state I/O lives) — replace every `workspace.join(".alert-state.json")` call with `paths.alert_state_path(workspace_basename)`. The reader, writer, AND the "clear on success" delete all change.
  - `autocoder/src/polling_loop.rs` — `filter_alert_state_lines` stays but becomes a defensive no-op (returns input unchanged unless the input contains a literal `.alert-state.json` line, which post-migration should be rare).
  - `autocoder/src/state/migration.rs` (or new module `autocoder/src/state/alert_state_migration.rs`) — new migration logic per the spec, gated by the `<state_dir>/alert-state/.migration-from-workspace-done` marker.
  - `docs/OPERATIONS.md` — update the throttled-failure-alerts section to name the new path.
  - `docs/STATE-LAYOUT.md` — add `alert-state` to the state-dir contents table; remove `.alert-state.json` from the workspace-local-files table.
- **Operator-visible behavior:**
  - The `git checkout` failures the operator observed stop occurring; the workspace never has `.alert-state.json` post-migration.
  - The `WritePolicy::None` audit-violation alerts caused by daemon writes during audit runs stop firing.
  - Operators inspecting `<state_dir>/alert-state/` see one file per workspace, named by the workspace's sanitized URL basename.
- **Breaking:** the on-disk location of alert state changes. The migration handles the move automatically on first startup. Operators reading the file by hand for diagnostic purposes need to update their path expectations. Documentation reflects the change.
- **Acceptance:** `cargo test` passes; `openspec validate a16-consolidate-workspace-bookkeeping-to-state-dir --strict` passes. New unit tests cover: (a) reader + writer use the state-dir path; (b) migration moves a workspace file to state-dir; (c) migration handles the both-exist case (state-dir wins); (d) migration sets the marker; (e) marker prevents re-scan; (f) tracked-in-git case attempts `git rm --cached` + commit + push, and on push failure logs ERROR + doesn't set the marker.
