## 1. `DaemonPaths` helper

- [ ] 1.1 In `autocoder/src/paths.rs` (or wherever `DaemonPaths` lives), add:
  ```rust
  impl DaemonPaths {
      pub fn alert_state_dir(&self) -> PathBuf {
          self.state_dir().join("alert-state")
      }
      pub fn alert_state_path(&self, workspace_basename: &str) -> PathBuf {
          self.alert_state_dir().join(format!("{workspace_basename}.json"))
      }
  }
  ```
- [ ] 1.2 Tests: helpers return the expected paths for a fixture `DaemonPaths`.

## 2. Replace workspace-root reads/writes with state-dir paths

- [ ] 2.1 Locate every read AND write of `.alert-state.json` in the codebase:
  ```bash
  grep -rn 'alert-state\.json' autocoder/src/
  ```
- [ ] 2.2 For each hit:
  - Reader sites: `paths.alert_state_path(workspace_basename)`.
  - Writer sites: same path.
  - Delete-on-success sites: same path.
- [ ] 2.3 Ensure `paths.alert_state_dir()` exists before write (`fs::create_dir_all` on first write).
- [ ] 2.4 The `path_literals_audit` CI test from `a09` automatically catches any remaining hard-coded `/tmp/autocoder/...alert-state...` literals; no separate guard needed.
- [ ] 2.5 Tests:
  - Writer: state file appears at `<state_dir>/alert-state/<basename>.json`, NOT at `<workspace>/.alert-state.json`.
  - Reader: reads from the state-dir path; absent file produces the "no entries" default.
  - Clear-on-success: deletes the state-dir file (or writes empty `{ "alerts": {} }` per the existing spec's equivalent semantics).

## 3. `filter_alert_state_lines` becomes defensive no-op

- [ ] 3.1 The helper stays in the polling-loop code path (don't remove yet — a future spec can remove it after a verification window). Its callers continue to invoke it.
- [ ] 3.2 The helper's logic still works as before: if the input porcelain text contains a `.alert-state.json` line, strip it; otherwise return unchanged. Post-migration the input shouldn't contain the line, so the helper becomes effectively a no-op for normal operation.
- [ ] 3.3 No test changes needed; the helper's existing tests continue to pass.

## 4. First-startup migration

- [ ] 4.1 New module `autocoder/src/state/alert_state_migration.rs` (or extension to the existing migration module). Public entry:
  ```rust
  pub async fn migrate_alert_state_from_workspace(
      paths: &DaemonPaths,
      repos: &[RepositoryConfig],
      git: &dyn GitOps,
  ) -> Result<()>;
  ```
- [ ] 4.2 Logic:
  - Check for the migration marker at `<state_dir>/alert-state/.migration-from-workspace-done`. If present, return immediately (idempotent).
  - For each configured repository:
    - Resolve the workspace path AND its basename.
    - Check if `<workspace>/.alert-state.json` exists. If not, skip this repo.
    - If `<state_dir>/alert-state/<basename>.json` already exists, the state-dir version wins — `rm` the workspace file (just `fs::remove_file`; if git-tracked, `git rm --cached` + commit + push).
    - If only the workspace version exists, `fs::rename` if same filesystem (most cases); else copy + delete. Then handle git tracking as above.
    - Log INFO per migrated workspace.
  - After processing all repos: if every repo completed successfully (no errors AND no operator-action-required cases), write the migration marker.
  - If any repository's migration failed (e.g., `git push` rejected due to branch protection), log ERROR naming the repository AND the specific failure, AND do NOT write the marker. The next startup retries.
- [ ] 4.3 The migration runs at daemon startup, before any polling task starts.
- [ ] 4.4 Tests:
  - Workspace file exists + state-dir file absent → migration moves it; marker is set.
  - Workspace file exists + state-dir file present → state-dir wins; workspace file removed; marker set.
  - Workspace file absent for all repos → no-op; marker set.
  - Workspace file tracked in git + push succeeds → `git rm --cached` + commit + push happens; marker set.
  - Workspace file tracked in git + push fails → ERROR logged; marker NOT set; next startup retries.
  - Marker present at startup → migration code is a no-op (no `fs::read` calls outside the marker check).

## 5. Git-tracking handling

- [ ] 5.1 When the migration finds `.alert-state.json` tracked in git (rare; only for repos that committed it accidentally), it runs:
  ```bash
  git -C <workspace> rm --cached .alert-state.json
  git -C <workspace> commit -m 'chore: untrack .alert-state.json (now stored in daemon state dir per a16)'
  git -C <workspace> push origin <base_branch>
  ```
- [ ] 5.2 The push uses the same token + auth path as normal autocoder pushes.
- [ ] 5.3 If push fails (4xx, network, branch protection), the migration logs ERROR with the suggested operator action (manual `git rm --cached` + PR) AND continues to other repositories. The marker is NOT set for the whole batch — so the next startup retries everything (idempotent reads on already-migrated repos).
- [ ] 5.4 If the repository's base branch has branch protection requiring PR review, the migration falls back: opens a PR on a fresh branch named `chore/untrack-alert-state` with the same removal commit AND posts a chatops notification asking the operator to merge. The marker is NOT set; the next startup re-checks AND skips this repo (file still tracked) — operator merges the PR, file becomes untracked, next migration startup finds it untracked AND completes.

## 6. Docs

- [ ] 6.1 In `docs/OPERATIONS.md`'s throttled-failure-alerts section, update the `.alert-state.json` references to name the new state-dir path.
- [ ] 6.2 In `docs/STATE-LAYOUT.md`'s state-dir contents table, add a row for `alert-state/` describing the file naming AND purpose. Remove `.alert-state.json` from the workspace-local-files table if present.
- [ ] 6.3 In `docs/TROUBLESHOOTING.md`, add an entry "git checkout fails with 'local changes to .alert-state.json'" describing the legacy-workspace case AND noting that the migration in `a16` handles it automatically on next daemon startup. For operators who hit it BEFORE the migration runs, the immediate-fix steps from this conversation (rm + push removal + restart).
- [ ] 6.4 In `docs/OPERATIONS.md`, add a brief "Migrations" section enumerating the migration markers the daemon checks at startup AND what each does. Includes the existing `state-paths-out-of-tmp` migration AND the new `alert-state-from-workspace` migration.

## 7. Spec deltas

- [ ] 7.1 `openspec/changes/a16-consolidate-workspace-bookkeeping-to-state-dir/specs/orchestrator-cli/spec.md` MODIFIES `Throttled predictable-failure alerts` (preserves all 7 scenarios verbatim with path references updated) AND ADDs `Alert-state migration from workspace to state-dir on first startup`.
- [ ] 7.2 `openspec/changes/a16-consolidate-workspace-bookkeeping-to-state-dir/specs/workspace-manager/spec.md` ADDs one requirement clarifying that `.alert-state.json` SHALL NOT appear in the workspace post-migration (catches future code drift that might recreate the file there).
- [ ] 7.3 `openspec/changes/a16-consolidate-workspace-bookkeeping-to-state-dir/specs/project-documentation/spec.md` ADDs one requirement covering OPERATIONS.md, STATE-LAYOUT.md, AND TROUBLESHOOTING.md updates.

## 8. Verification

- [ ] 8.1 `cargo test` passes (new + existing).
- [ ] 8.2 `openspec validate a16-consolidate-workspace-bookkeeping-to-state-dir --strict` passes.
- [ ] 8.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
- [ ] 8.4 Manual verification: on a workspace with `.alert-state.json` present, restart the daemon; on next startup the file appears at `<state_dir>/alert-state/<basename>.json` AND no longer exists in the workspace.
