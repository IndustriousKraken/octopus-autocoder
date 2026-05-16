## 1. Code

- [x] 1.1 In `autocoder/src/git.rs`, added `pub fn fetch_remote(workspace: &Path, remote: &str) -> Result<()>` invoking `git fetch <remote>`.
- [x] 1.2 In `autocoder/src/workspace.rs::ensure_initialized`, captured `let did_clone = !workspace.exists();` before the conditional clone.
- [x] 1.3 After `git::ensure_remote(workspace, "fork", fork_url)`, when `did_clone && fork_url.is_some()`, call `git::fetch_remote(workspace, "fork")`. Errors are logged at WARN and not propagated.

## 2. Tests

- [x] 2.1 `git::tests::fetch_remote_invokes_git_fetch_for_named_remote` — sets up two local "remotes" (origin + alt), runs `fetch_remote(ws, "alt")`, asserts `refs/remotes/alt/main` resolves.
- [x] 2.2 `workspace::tests::ensure_initialized_fetches_fork_on_fresh_clone` — fork has an extra commit beyond upstream; after `ensure_initialized` on a missing workspace, asserts `refs/remotes/fork/main` resolves AND matches fork's HEAD (not upstream's).
- [x] 2.3 `workspace::tests::ensure_initialized_does_not_re_fetch_fork_on_existing_workspace` — first init captures fork SHA; advance fork by one commit; second init does NOT update the local tracking ref (re-init path doesn't fetch fork).
- [x] 2.4 `workspace::tests::ensure_initialized_tolerates_fork_fetch_failure` — fork URL points at a non-existent path; `ensure_initialized` still returns Ok and the remote is still registered.

## 3. Documentation

- [x] 3.1 README "Operating Notes" — added a "Workspace directory deleted" subsection between busy-marker and perma-stuck describing the snafu scenario and the post-clone fork-fetch recovery.

## 4. Verification

- [x] 4.1 `cargo test` passes (380/381; 1 ignored, unrelated).
- [x] 4.2 `openspec validate fetch-fork-at-workspace-init --strict` passes.
