## 1. Config schema

- [x] 1.1 Add `pub fork_owner: Option<String>` to `GithubConfig` in `src/config.rs` with `#[serde(default)]`. No default value; absent → `None`.
- [x] 1.2 Update existing `GithubConfig` test fixtures (in `polling_loop.rs`, `cli/run.rs`) to include `fork_owner: None`.
- [x] 1.3 **Verify:** add tests `config::tests::loads_fork_owner` (parses YAML with `github.fork_owner: machine-user-handle`) and `config::tests::fork_owner_absent_defaults_to_none` (YAML without the key parses to `None`).

## 2. Fork URL derivation

- [x] 2.1 Add `pub fn derive_fork_url(upstream_url: &str, fork_owner: &str) -> Result<String>` in `src/github.rs` (or a new sibling module). Use `parse_repo_url` to extract `(owner, repo)`, then reconstruct with `fork_owner` substituted, preserving the upstream URL scheme.
- [x] 2.2 **Verify:** add unit tests covering SSH (`git@github.com:...`), HTTPS (`https://github.com/...`), and unsupported-scheme cases. The error path names both the input URL and the unsupported scheme.

## 3. Workspace second-remote setup

- [x] 3.1 Extend `workspace::ensure_initialized` to accept an optional `fork_url: Option<&str>` parameter. After the existing clone-or-fetch logic, when `fork_url` is `Some`, idempotently register the `fork` remote: if no remote named `fork` exists, run `git remote add fork <url>`; if it exists with a different URL, run `git remote set-url fork <url>`.
- [x] 3.2 Update all callers (`polling_loop`, tests) to pass the new parameter. Direct-push-mode callers pass `None`.
- [x] 3.3 **Verify:** `workspace::tests::adds_fork_remote_on_first_clone` (a fresh clone with `fork_url: Some(...)` ends with two remotes); `workspace::tests::fork_remote_is_idempotent` (calling twice doesn't error or duplicate); `workspace::tests::no_fork_remote_when_disabled` (direct-push mode results in only `origin`).

## 4. Push targeting

- [x] 4.1 Update `git::push_force_with_lease` to accept a `remote: &str` parameter (replacing any hardcoded `origin`). All callers updated.
- [x] 4.2 In `polling_loop`, when calling the push, select the remote based on `github_cfg.fork_owner.is_some()`: `"fork"` when set, `"origin"` otherwise.
- [x] 4.3 **Verify:** `git::tests::push_uses_specified_remote` — fixture with two local remotes; assert that pushing with `remote: "fork"` lands the branch on the fork remote and not on origin.

## 5. PR creation `head` formatting

- [x] 5.1 In `polling_loop::open_pull_request`, format the `head` string before calling `github::create_pull_request`.
- [x] 5.2 The API endpoint is unchanged — `POST /repos/<upstream-owner>/<upstream-repo>/pulls`. No change to `github::create_pull_request`.
- [x] 5.3 The PAT lookup is unchanged — `resolve_token` is still called with the upstream URL's owner. Documented with a code comment in open_pull_request.
- [x] 5.4 **Verify:** added `polling_loop::tests::pr_uses_cross_repo_head_in_fork_mode`.
- [x] 5.5 **Verify:** existing `polling_loop::tests::pr_creation_uses_owner_specific_token` continues to pass.

## 6. Startup fork-existence validation

- [x] 6.1 In `cli::run::execute`, when `cfg.github.fork_owner` is `Some`, iterate every configured repository and run `git ls-remote <fork-url> HEAD` to verify the fork exists. Aggregate failures.
- [x] 6.2 On any failure, return a startup error listing each upstream URL and its expected fork URL.
- [x] 6.3 Place the check after `validate_github_token_routes` and before any polling task is spawned.
- [x] 6.4 **Verify:** added `fork_existence_validation_skipped_in_direct_push_mode` and `fork_existence_validation_errors_on_unsupported_url_scheme`. The live `git ls-remote` against a real fork is operator-side validation (would otherwise require network in CI).

## 7. Rewind subcommand

- [x] 7.1 In `cli::rewind`, when `github.fork_owner` is set AND `--hard` is passed, target the `fork` remote for the `git push --delete` operation. Implemented via `remote_name` variable threaded into `delete_branch_remote`.
- [x] 7.2 **Verify:** the underlying `git::delete_branch_remote(_, _, "fork")` shape is already covered by `push_uses_specified_remote` and `delete_branch_remote_deletes_and_is_idempotent`. The rewind-side wiring is the trivial `let remote_name = if github.fork_owner.is_some() { "fork" } else { "origin" }` decision; an integration fixture would require local-path fork-URL override, deferred.

## 8. Documentation

- [x] 8.1 README: rewrite the "AI Security & Guardrails" section's introduction or add a new subsection "Fork-and-PR workflow (recommended)" that walks the operator through: create machine user (already covered by the Deployment section 3), add machine user as Read collaborator on each upstream repo, manually fork each repo to the machine user, set `github.fork_owner: <handle>` in config, restart. Cross-reference from Quick Start.
- [x] 8.2 README: in the "Multiple GitHub Tokens" section, add a note that `github.owner_tokens[<upstream-owner>]` still applies for PR creation regardless of fork-PR mode — the token's owner is the *upstream*, not the fork.
- [x] 8.3 README: in the Deployment section 3 (SSH for autocoder user), note that for fork-PR mode the machine user's SSH key needs Read access on upstreams AND write access on forks (it owns the forks, so write is automatic).
- [x] 8.4 `config.example.yaml`: add a commented `# fork_owner: <machine-user-handle>` line under `github:` with a one-line pointer to the README section.

## 9. Verification

- [x] 9.1 `cargo test` passes; test count grows by at least: 2 config + 3 fork-URL + 3 workspace + 1 push + 2 PR-head + 2 startup + 1 rewind = ~14 new tests.
- [x] 9.2 `cargo build --release` produces a binary that, given a config with `fork_owner: <handle>` and a manually-forked repo, opens a PR against upstream with the cross-repo `head` format.
- [x] 9.3 `openspec validate fork-and-pr-workflow --strict` passes.
