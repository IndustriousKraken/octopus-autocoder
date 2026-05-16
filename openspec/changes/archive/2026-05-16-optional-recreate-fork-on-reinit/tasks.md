## 1. GitHub helper

- [x] 1.1 In `autocoder/src/github.rs`, add `pub async fn delete_repo(owner: &str, repo: &str, token: &str) -> Result<DeleteOutcome>` invoking `DELETE /repos/{owner}/{repo}`. Returns:
  ```rust
  pub enum DeleteOutcome {
      Deleted,         // 204 (or 200) — fork was deleted by this call
      AlreadyGone,     // 404 — fork was already absent
      Forbidden,       // 403 — typically missing delete_repo scope
  }
  ```
  Any other non-2xx returns `Err` with the status + body excerpt.
- [x] 1.2 Add `pub(crate) async fn delete_repo_at_for_test(api_base, owner, repo, token)` mirroring the existing `create_fork_at_for_test` pattern.
- [x] 1.3 Tests `github::tests`:
  - `delete_repo_returns_deleted_on_204`
  - `delete_repo_returns_already_gone_on_404`
  - `delete_repo_returns_forbidden_on_403`
  - `delete_repo_errors_on_other_non_2xx`

## 2. Config schema

- [x] 2.1 In `autocoder/src/config.rs::GithubConfig`, add `#[serde(default)] pub recreate_fork_on_reinit: bool` (default false).
- [x] 2.2 Tests `config::tests`:
  - `recreate_fork_on_reinit_defaults_to_false`
  - `recreate_fork_on_reinit_parses_true`
  - `recreate_fork_on_reinit_parses_false`

## 3. Workspace helper

Design note: `ensure_initialized` stays SYNC (local git work only). The
async re-fork work lives in a separate helper so the existing sync
callers (`repo_passes_startup_check`, ~10 test sites) don't need to be
async-ified. Callers run `recreate_fork` BEFORE `ensure_initialized`
when the conditions are met.

- [x] 3.1 In `autocoder/src/workspace.rs`, add `pub async fn recreate_fork(github_cfg: &GithubConfig, repo: &RepositoryConfig) -> Result<RecreateOutcome>`. The outcome enum:
  ```rust
  pub enum RecreateOutcome {
      Recreated,            // delete (or already-gone) + create + reachable, all OK
      Forbidden,            // delete returned 403 — caller should fall back
  }
  ```
  Steps:
  - Resolve upstream `(owner, repo_name)` via `github::parse_repo_url`.
  - Resolve token via `github_credentials::resolve_token`.
  - Call `github::delete_repo(fork_owner, repo_name, &token).await`. Match outcome:
    - `Deleted` → INFO log, proceed.
    - `AlreadyGone` → INFO log "fork already absent; proceeding to recreate", proceed.
    - `Forbidden` → ERROR log naming the `delete_repo` scope; return `RecreateOutcome::Forbidden` so caller falls back.
  - `tokio::time::sleep(Duration::from_secs(2)).await`.
  - Call `github::create_fork(upstream_owner, repo_name, &token).await`. Propagate Err.
  - Poll `git::ls_remote_head(&fork_url)` up to 30s (every 2s).
  - On reachable: return `Recreated`. On timeout: return Err.
- [x] 3.2 Workspace tests (use `delete_repo_at_for_test`-style mockito hooks via a private `recreate_fork_at_for_test` that accepts an API-base override):
  - `recreate_fork_returns_recreated_on_normal_path`
  - `recreate_fork_already_gone_proceeds_to_create`
  - `recreate_fork_forbidden_returns_forbidden_without_creating`

## 4. Caller plumbing

- [x] 4.1 `repo_passes_startup_check` (cli/run.rs): if `github.recreate_fork_on_reinit && fork_url.is_some() && !workspace_path.exists()`, log INFO "deferring workspace init to first polling iteration" and return `true` WITHOUT calling `ensure_initialized` or the dirty check. This lets the polling loop run the destructive recreate path in an async context.
- [x] 4.2 `polling_loop::run_pass_through_commits`: before `ensure_initialized`, capture `did_clone = !workspace.exists()`. When `did_clone && fork_url.is_some() && github_cfg.recreate_fork_on_reinit`, call `workspace::recreate_fork(...).await`:
  - `Ok(RecreateOutcome::Recreated)` → mark `did_refork = true`, proceed.
  - `Ok(RecreateOutcome::Forbidden)` → log ERROR with scope hint; proceed to ensure_initialized (conservative fallback).
  - `Err(e)` → log ERROR; proceed to ensure_initialized (conservative fallback).
- [x] 4.3 After `ensure_initialized` succeeds and `did_refork` is true, call `maybe_post_refork_notification(repo, chatops_ctx)` before returning the iteration's normal flow.
- [x] 4.4 At startup (in `cli/run.rs::execute`), if `cfg.github.recreate_fork_on_reinit == true && cfg.github.fork_owner.is_none()`, emit an INFO log: "github.recreate_fork_on_reinit is true but fork_owner is unset; flag will have no effect."

## 5. ChatOps notification

- [x] 5.1 Add `async fn maybe_post_refork_notification(repo: &RepositoryConfig, chatops_ctx: Option<&ChatOpsContext>)` in `polling_loop.rs` mirroring `maybe_post_pr_opened`. Best-effort; logs WARN on post failure. Gated by `failure_alerts_enabled` (this IS an operator-visible destructive event).
- [x] 5.2 Test: with a fake chatops backend wired through `ChatOpsContext`, calling `maybe_post_refork_notification` posts exactly one message containing `re-forked` and the repo URL.

## 6. Documentation

- [x] 6.1 README "Config reference" — under `github:` table, add `recreate_fork_on_reinit` row with type `bool`, default `false`, and a multi-line description noting that the flag deletes the fork on GitHub (closing any open PRs from it) and requires the `delete_repo` PAT scope.
- [x] 6.2 README "Operating Notes" — add a subsection "Fork recreation on workspace reinitialization" describing the use case, the destructive nature, and the PAT scope requirement.

## 7. Verification

- [x] 7.1 `cargo test` passes.
- [x] 7.2 `openspec validate optional-recreate-fork-on-reinit --strict` passes.
