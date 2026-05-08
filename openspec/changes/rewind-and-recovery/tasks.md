## 1. Git branch-deletion utilities

- [ ] 1.1 In `src/git.rs`, implement `pub fn delete_branch_local(workspace: &Path, branch: &str) -> Result<()>` running `git branch -D <branch>`. If the branch does not exist locally, log a debug line and return Ok (idempotent).
- [ ] 1.2 Implement `pub fn delete_branch_remote(workspace: &Path, branch: &str) -> Result<()>` running `git push origin --delete <branch>`. If the branch does not exist remotely (specific git error message), log a debug line and return Ok. Other failures (auth, network) return Err.
- [ ] 1.3 **Verify:** `cargo test git::tests::delete_branch_local_idempotent` against a fixture repo where the branch is created, deleted, then deleted again (second call must not error).

## 2. CLI argument additions

- [ ] 2.1 Update `src/cli.rs`: the `Rewind` subcommand gains a `--repo: Option<String>` argument.
- [ ] 2.2 The argument is documented in `--help` as `"Repository URL or short-name (basename without .git). Required when config has multiple repositories."`
- [ ] 2.3 **Verify:** `./target/release/orchestrator rewind --help` shows the new `--repo` argument with the documented help text.

## 3. Repo-selector resolution

- [ ] 3.1 In a new `src/cli/rewind.rs` (or `src/rewind.rs`), implement `pub fn resolve_repo<'a>(repos: &'a [RepositoryConfig], selector: Option<&str>) -> Result<&'a RepositoryConfig>`:
  - If `repos.len() == 0`: return `Err(anyhow!("no repositories configured"))`.
  - If `repos.len() == 1` AND `selector.is_none()`: return Ok of that single repo.
  - If `repos.len() > 1` AND `selector.is_none()`: return `Err(anyhow!("--repo is required with multiple configured repositories. Available: {list}"))` where `{list}` is the comma-separated short names.
  - If `selector.is_some()`: match against each repo's `url` exactly OR its derived short-name (basename of url stripped of `.git`). Exactly one match → Ok. Zero matches → Err naming available selectors. Multiple matches → Err naming the conflicting repos.
- [ ] 3.2 **Verify:** `cargo test rewind::tests::resolve_single_default`, `resolve_multi_requires_selector`, `resolve_match_by_url`, `resolve_match_by_short_name`, `resolve_zero_matches_errors`, `resolve_multi_matches_errors`.

## 4. Rewind execution

- [ ] 4.1 Implement `pub async fn rewind::execute(repos: Vec<RepositoryConfig>, args: RewindArgs) -> Result<()>`:
  - Resolve the target repo via `resolve_repo`.
  - Initialize the workspace if necessary (`workspace::ensure_initialized` to ensure local clone exists for branch deletion).
  - If `!args.hard`: prompt the user with `"This will delete branch '<agent_branch>' (local) and unarchive <N> change(s) (<names>). Proceed? [y/N] "`. Read a line from stdin. If the trimmed input is not `y` or `Y`, log "rewind cancelled" and return Ok.
  - `git::delete_branch_local(workspace, agent_branch)?` (Err here is fatal).
  - If `args.hard`: `git::delete_branch_remote(workspace, agent_branch)`. Log Err but do not fail.
  - Checkout the base branch: `git::checkout(workspace, &repo.base_branch)?`.
  - For each change name in `args.changes`: call `queue::unarchive(workspace, name)`. Collect successes and failures.
  - At the end: if any unarchive failed, return `Err(anyhow!("rewind partially failed: {summary}"))` listing the failures; otherwise return Ok with a log line summarizing the rewound changes.
- [ ] 4.2 Wire `rewind::execute` into `cli.rs`'s `Rewind` subcommand handler.
- [ ] 4.3 **Verify:** Manual smoke test in `docs/rewind-smoke-test.md`: against a sandbox repo, archive a change manually, run `orchestrator rewind <change> --config config.yaml`, confirm the change directory reappears under `openspec/changes/`. Then push a fake commit to the agent branch and run `orchestrator rewind <change> --config config.yaml --hard`; confirm both local and remote branches are gone.

## 5. Documentation

- [ ] 5.1 Update `README.md` rewind section: `--repo` argument, single vs multi-repo behavior, soft vs hard semantics, the default-to-no confirmation prompt.
- [ ] 5.2 Document the recovery procedure for "I rewound the wrong change": archived directories are not deleted, so re-archiving manually restores state.
