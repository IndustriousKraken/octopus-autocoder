## 1. Git helper

- [x] 1.1 In `autocoder/src/git.rs`, add `pub fn fetch_remote_branch(workspace: &Path, remote: &str, branch: &str) -> Result<()>` invoking `git fetch <remote> +refs/heads/<branch>:refs/remotes/<remote>/<branch>`. The `+` enables forced update of the local tracking ref so a non-fast-forward agent branch on the fork doesn't fail the fetch.
- [x] 1.2 Test: `fetch_remote_branch_populates_only_named_branch` — fixture fork has `main` + `extra-branch`; after `fetch_remote_branch(ws, "fork", "main")`, `refs/remotes/fork/main` resolves AND `refs/remotes/fork/extra-branch` does NOT resolve.
- [x] 1.3 Test: `fetch_remote_branch_force_updates_non_ff` — fixture fork advances, rewrites history (force-push equivalent), then `fetch_remote_branch` succeeds (the `+` refspec accepts the non-fast-forward update).

## 2. Workspace signature change

- [x] 2.1 In `autocoder/src/workspace.rs::ensure_initialized`, change the `fork_url: Option<&str>` parameter to `fork: Option<(&str, &str)>` where the tuple carries `(fork_url, agent_branch)`. The post-clone fetch step uses `git::fetch_remote_branch(workspace, "fork", agent_branch)`. The `ensure_remote` step still uses just the URL.
- [x] 2.2 Update existing workspace tests that pass a fork URL to construct the tuple. The fixture's only branch is `main`, so all of them pass `Some((&fork_url, "main"))`.
- [x] 2.3 Test: `ensure_initialized_fetches_only_agent_branch_from_fork` — fixture fork has `main` plus `dev` (the latter shadowing a name that would also exist on upstream). After `ensure_initialized(ws, upstream_url, Some((fork_url, "main")))`, `refs/remotes/fork/main` resolves AND `refs/remotes/fork/dev` does NOT.
- [x] 2.4 Regression test: `checkout_base_branch_after_fork_init_does_not_ambiguate` — fixture upstream has `main` AND `dev`, fork has `main` AND `dev` (shadow). After ensure_initialized fetches only `main` from fork, `git checkout dev` (executed on the cloned workspace) succeeds without the "matched multiple remote tracking branches" error.

## 3. Caller plumbing

- [x] 3.1 `cli/run.rs::repo_passes_startup_check` — change the `fork_url.as_deref()` argument to `fork_url.as_deref().map(|u| (u, repo.agent_branch.as_str()))`.
- [x] 3.2 `polling_loop.rs::run_pass_through_commits` — same update at the `ensure_initialized` call site.

## 4. Documentation

- [x] 4.1 README "Workspace directory deleted" subsection — update the existing sentence "fetches the `fork` remote at that time" to clarify it fetches only the agent branch. Add a short note explaining the rationale (avoiding `git checkout` DWIM ambiguity when the fork has branches that shadow upstream names).

## 5. Verification

- [x] 5.1 `cargo test` passes.
- [x] 5.2 `openspec validate fetch-fork-agent-branch-only --strict` passes.
