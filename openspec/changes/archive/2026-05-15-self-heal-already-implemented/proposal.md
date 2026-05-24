## Why

Observed in production: after a rebase that dropped archive commits, four changes ended up with their implementation already in HEAD but their change directories still in active state. autocoder polled them, the agent correctly reported "already done" without modifying the workspace, and the `no-op-completion-is-failure` rule classified each as Failed. The agent then re-ran them on every 30-minute poll, burning Claude tokens to re-confirm "already done" forever.

The root issue: there's no path for "implementation exists, just needs archiving." Today the only way out is operator intervention (manual `openspec archive`). Across a fleet of managed repos that's enough cost exposure to warrant a structural fix.

## What Changes

- **MODIFIED capability:** `orchestrator-cli`'s "Reject archive-only iterations as Failed" requirement gains a new scenario for the self-heal path. When the agent reports Completed with empty workspace AND `openspec validate <change> --strict` exits 0 AND every task in the change's `tasks.md` is `[x]`, autocoder treats the iteration as Archive (not Failed): the daemon itself runs the archive move (renaming `openspec/changes/<change>/` to `openspec/changes/archive/<YYYY-MM-DD>-<change>/`), commits it on the agent branch with a clear subject, and proceeds through the normal push + PR flow.
- **PR body for self-healed archives** includes a note: `This PR archives a change whose implementation was already present on the base branch. No code diff is included; only the openspec archive move.`
- **Detection criteria (all four must hold):**
  1. Executor returned `Completed`.
  2. `git status --porcelain` is empty (no agent-produced diff).
  3. `openspec validate <change> --strict` exits 0.
  4. Every line of `tasks.md` that starts with `- [` has `[x]` (no remaining `[ ]`).
- **Failure modes within self-heal:**
  - If `openspec validate` fails or errors (binary missing, transient): fall through to the existing Failed path (no change in behavior). This is rare and indistinguishable from a genuinely stuck change.
  - If parsing `tasks.md` for `[ ]` markers fails: fall through to Failed.
- **Code:**
  - `polling_loop` gains an `archive_already_implemented_change` helper invoked from the `handle_outcome` `Completed` branch when porcelain is empty, before returning Failed.
  - New `tasks_md_all_complete(workspace, change) -> Result<bool>` reads `openspec/changes/<change>/tasks.md`, scans for lines matching `^\s*-\s*\[([ x])\]`, returns true iff every match has `[x]` and there is at least one match.
  - New `openspec_validate_strict(workspace, change) -> Result<bool>` shells out to `openspec validate <change> --strict` and reports exit-0 success.
  - The archive move uses the existing `queue::archive` helper.

## Impact

- Affected specs: `orchestrator-cli` (one MODIFIED requirement gains a new scenario).
- Affected code: `autocoder/src/polling_loop.rs`.
- Behavior change visible to operators: changes whose implementation is already on the base branch will self-archive on the next poll, producing a tiny PR (just the archive move) instead of looping Failed.
- Edge case: an operator who manually checks every task in `tasks.md` without doing the work would trigger self-heal and ship an empty-diff PR. That PR is easy to spot and reject in human review; the daemon won't waste subsequent Claude runs on it either way.
- Token-burn improvement: one self-heal pass per affected change after which the change is gone from `list_pending`. No more 30-minute Groundhog Day.
