## Why

`fetch-fork-at-workspace-init` shipped `git fetch fork` (no refspec) after the post-clone fork-remote registration. The intent was narrow: populate `refs/remotes/fork/<agent_branch>` so subsequent `git push --force-with-lease fork <agent_branch>` has accurate local tracking data and stops misfiring "stale info" rejections.

The implementation overshot. `git fetch fork` (no refspec) fetches every branch on the fork — including branches with names that already exist on `origin`. Production failure observed on `myrepo` (2026-05-16):

```
fatal: 'dev' matched multiple (2) remote tracking branches
```

The polling loop's `git::checkout(workspace, &repo.base_branch)` (where `base_branch = "dev"`) hits this when DWIM (auto-create-and-track) sees `dev` on both `refs/remotes/origin/dev` and `refs/remotes/fork/dev` and refuses to pick. Iteration fails. Repo stalls.

The fork's other branches are irrelevant to autocoder. The push-with-lease invariant needs exactly one fork ref — the agent branch — to be locally tracked. Everything else is collateral damage.

## What Changes

- **MODIFIED capability:** `workspace-manager` — the "Idempotent workspace initialization" requirement is amended. The post-clone fork fetch SHALL pass an explicit refspec restricted to the agent branch (`+refs/heads/<agent_branch>:refs/remotes/fork/<agent_branch>`) instead of fetching all branches.
- **Code:**
  - `autocoder/src/git.rs`: add `pub fn fetch_remote_branch(workspace, remote, branch) -> Result<()>`. Runs `git fetch <remote> +refs/heads/<branch>:refs/remotes/<remote>/<branch>`. The `+` enables force-update of the local tracking ref so a non-fast-forward agent branch on the fork doesn't fail the fetch.
  - `autocoder/src/workspace.rs::ensure_initialized` — change `fork_url: Option<&str>` parameter to `fork: Option<(&str, &str)>` carrying `(fork_url, agent_branch)`. The post-clone fork fetch uses the new `fetch_remote_branch` helper.
  - Both callers (`cli/run.rs::repo_passes_startup_check`, `polling_loop.rs::run_pass_through_commits`) update the call site to pass the agent branch.
  - Tests under `workspace::tests` that exercise the fork path are updated to pass `Some((fork_url, "main"))` instead of `Some(fork_url)` (the fixture's only branch is `main`).
- **Optional cleanup:** the existing `git::fetch_remote(workspace, remote)` (all-branches) helper has no remaining callers after this change. Keep it (no callers to break) but consider removing in a follow-up if dead-code warnings surface.

## Impact

- Affected specs: `workspace-manager` (one MODIFIED requirement; scenarios updated to reference the new refspec form).
- Affected code: `autocoder/src/git.rs` (new helper), `autocoder/src/workspace.rs` (signature change + call site), `autocoder/src/cli/run.rs` and `autocoder/src/polling_loop.rs` (caller signature update), ~6 workspace test sites (constructor literal update).
- Operator-visible behavior: fork-PR mode workspaces no longer accumulate `refs/remotes/fork/<every-branch>`. Only `refs/remotes/fork/<agent_branch>` is populated. The `git checkout <base_branch>` DWIM ambiguity disappears.
- Breaking: no operator-facing API or config change. Internal Rust signature change to `ensure_initialized` is contained within autocoder.
- Recovery for affected production workspaces: delete the workspace directory (`rm -rf /tmp/workspaces/<repo>`) so the next iteration re-clones from scratch with the new (restricted) fetch. Existing `refs/remotes/fork/*` refs in already-initialized workspaces persist until the workspace is deleted; they don't actively break anything once a local `dev` branch exists, but they remain stale.
