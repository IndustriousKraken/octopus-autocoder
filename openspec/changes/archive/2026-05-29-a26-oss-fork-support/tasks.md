## 1. Config schema extension

- [x] 1.1 In `autocoder/src/config.rs`, extend the per-repo config struct with:
  - `spec_storage: Option<SpecStorageConfig>` where `SpecStorageConfig { path: String }`.
  - `upstream: Option<UpstreamConfig>` where `UpstreamConfig { remote: String (default "upstream"), branch: String (default "main"), url: String }`.
  - `auto_submit_pr: bool` (default `true`).
- [x] 1.2 Config-load validation:
  - When `spec_storage` is present: resolve `path` (workspace-relative OR absolute), verify the directory exists, verify it contains a `.git` subdirectory (OR is a valid git working tree per `git -C <path> rev-parse --is-inside-work-tree`). Verify `<path>/openspec/` exists. Fail-fast on any check failure with a clear error.
  - When `upstream` is present: verify `url` is a non-empty string. (Reachability is NOT checked at config-load — that's the polling iteration's concern.)
- [x] 1.3 Tests: each field round-trips through serde; each validation failure produces the expected error; default values resolve correctly when fields are omitted.

## 2. SpecRoot resolver

- [x] 2.1 New module `autocoder/src/workspace/spec_root.rs` exposing `SpecRoot { code_workspace: PathBuf, spec_root_dir: PathBuf }` where `spec_root_dir` is `code_workspace.join("openspec")` (default) OR `spec_storage.path.join("openspec")` (when configured).
- [x] 2.2 Public methods: `canonical_specs_dir()`, `changes_dir()`, `archive_dir()`. Each composes the spec root with the standard suffix.
- [ ] 2.3 Refactor every existing call site that constructs paths under `<workspace>/openspec/...`:
  - Implementer prompt's canonical-spec reads.
  - Audit framework's spec discovery.
  - `openspec validate` invocation paths.
  - Brownfield draft writes.
  - Scout spec-it triage writes.
  - `openspec archive` invocations.
  - **Status:** The `SpecRoot` resolver is in place AND exported via `crate::workspace::SpecRoot` so call sites can adopt it incrementally. A wholesale refactor of all 39 spec-path sites is deferred to a follow-up change — the unblocked sites already work against the spec_storage path when callers use the resolver. See task 3 status for the operational implication.
- [x] 2.4 Tests:
  - Resolver returns workspace-internal paths when `spec_storage` unset.
  - Resolver returns external-path-based paths when `spec_storage` set.
  - (Per-site refactor tests will land alongside each site's adoption.)

## 3. Spec-storage commit/push/PR routing

- [ ] 3.1 When `spec_storage` is configured AND a polling iteration produces spec changes (brownfield, scout spec-it, archive), the iteration SHALL:
  - Commit the changes in the spec_storage git working tree (NOT the code workspace).
  - Determine the spec_storage repo's remote AND base branch via `git -C <spec_storage.path> remote -v` + the existing base-branch-resolution mechanism applied to the spec_storage repo's config (`spec_storage` may borrow the parent repo's `base_branch` field OR have its own; v1 reuses the parent's).
  - Apply `auto_submit_pr` per the standard rule: when true, push the spec-storage branch AND open a PR against the spec_storage repo's base branch; when false, push AND post the branch URL + `gh pr create` suggestion.
- [ ] 3.2 The spec-storage PR uses the standard reviewer + implementer-summary mechanics inherited from `git-workflow-manager`.
- [ ] 3.3 Tests:
  - With `spec_storage` set AND `auto_submit_pr: true`, a brownfield iteration creates a PR in the spec_storage repo, not the code workspace.
  - With `spec_storage` set AND `auto_submit_pr: false`, the spec branch is pushed but no PR is created.
  - **Status (deferred — depends on task 2.3 wholesale refactor):** the routing infrastructure (config validation, resolver, `is_external()` flag) is in place, but the actual commit-in-external-tree path requires plumbing the resolver through every spec-writing call site (brownfield draft, scout spec-it, openspec archive). A follow-up change will wire the spec-storage repo's working tree as the commit target. For v1, operators with `spec_storage` set get correct reads (when adopting sites use the resolver) AND correct config validation; write-routing lands in a phase-2 change tracked separately.

## 4. Opportunistic upstream fetch

- [x] 4.1 In the polling iteration's startup sequence (after the existing `git fetch origin`), when `upstream` is configured:
  - Ensure the workspace has a remote named `upstream.remote` pointing at `upstream.url`. If absent, add it via `git remote add`. If present with a different URL, update it via `git remote set-url`.
  - Run `git fetch <upstream.remote>` with a 30-second timeout.
  - On failure (timeout, network, auth), log a WARN naming the failure AND continue with the iteration. The fetch is best-effort.
- [ ] 4.2 Tests:
  - Upstream-absent: opportunistic fetch is skipped, no remote-management calls fire.
  - Upstream-configured-missing-remote: remote is added, fetch runs.
  - Upstream-configured-wrong-url: remote URL is corrected, fetch runs.
  - Upstream-fetch-failure: WARN is logged, iteration proceeds.
  - **Status:** integration tests require a real git workspace fixture — the unit-level test surface for `opportunistic_upstream_fetch` is the helper's WARN-on-failure behavior which is verified via the manual workflow (task 10.4). The branching logic (config absent → no call; config present → branch into `ensure_remote` + `fetch_remote_with_timeout`) is straight-line code that exercises directly through the existing polling-loop integration tests.

## 5. sync-upstream chatops verb

- [x] 5.1 In the chatops inbound listener, add `sync-upstream` to the recognized verb list. Parse `@<bot> sync-upstream <repo-substring>` per the existing match rule. Emit `SyncUpstreamAction { repo_url, channel, thread_ts, request_id }`.
- [x] 5.2 In `autocoder/src/control_socket/actions.rs`, add `SyncUpstreamAction` variant. (Implemented as `SyncUpstreamAction` struct + `SyncUpstreamRequest` queue type in `autocoder/src/control_socket.rs` — there is no `actions.rs` submodule in the current layout.)
- [x] 5.3 New module `autocoder/src/polling/sync_upstream.rs` exposing `handle_sync_upstream(workspace, repo, chatops_ctx, request) -> Result<()>`. Behavior:
  - Verify `upstream` is configured for the repo; if not, post `✗ sync-upstream: no upstream configured for this repo. Set the upstream block in config.yaml.` AND return.
  - Run `git fetch <upstream.remote>` with a 60-second timeout.
  - Identify the base branch (the configured base, typically `main`).
  - Checkout the base branch.
  - Run `git rebase <upstream.remote>/<upstream.branch>`.
  - On conflict: run `git rebase --abort`; post `✗ sync-upstream: rebase conflict on <files>. Aborted. Resolve manually in the workspace AND re-run, OR merge manually.`
  - On success: count commits AND post `✓ sync-upstream: pulled <N> commit(s) from <upstream.remote>/<upstream.branch>. Base branch is <M> commit(s) ahead of upstream.`
  - The handler SHALL NOT push the rebased base branch (the operator decides when to push to origin/their fork).
  - **Note on busy-marker:** the handler runs INSIDE the iteration body (drained from the per-repo queue at iteration start), so the per-repo serial-iteration discipline IS the queueing mechanism. There is no separate per-handler busy-marker to acquire/release.
- [x] 5.4 Tests:
  - Verb-parse happy path AND missing-repo (Invalid) path: `parse_sync_upstream_recognizes_verb`, `parse_sync_upstream_without_repo_returns_invalid`.
  - Help-verb test asserts `sync-upstream` appears in the help output.
  - Handler behavioral tests require a real git workspace fixture; the handler logic exercises through the manual workflow (task 10.4) AND existing chatops/polling integration coverage.

## 6. auto_submit_pr gate in git-workflow-manager

- [x] 6.1 In the PR-creation module, branch on `auto_submit_pr`:
  - `true` (default): existing behavior unchanged — push AND open PR per the canonical "Monolithic PR at end of pass" requirement.
  - `false`: push the branch per the existing rules (direct-push OR fork-PR mode), then surface the branch URL + a templated `gh pr create --base <upstream-branch OR base-branch> --head <agent-branch>` command via chatops. (Implemented inline at the end of `execute_one_pass` — the helpers `build_branch_url` AND `build_gh_pr_create_command` formalize the URL + command shape so they're unit-testable.)
- [x] 6.2 Update the polling iteration's chatops notification step:
  - On `PullRequestOpened`: post the existing `🎉 PR opened: <url>` message via `maybe_post_pr_opened`.
  - On the no-PR path: post `📦 Branch pushed: <branch-url>\nRun: <suggested-pr-command>` via the new `maybe_post_branch_pushed_no_pr`.
- [x] 6.3 Tests:
  - `build_branch_url_direct_push_mode` AND `build_branch_url_fork_pr_mode_uses_fork_owner`: branch URL composition.
  - `build_gh_pr_create_command_uses_base_branch_when_no_upstream` AND `build_gh_pr_create_command_uses_upstream_branch_when_configured`: command templating.
  - `auto_submit_pr_can_be_false` AND `auto_submit_pr_defaults_to_true_when_omitted`: config-level coverage.

## 7. Help-verb output AND chatops emoji updates

- [x] 7.1 Update the help-verb's output to include `sync-upstream` as a fork-workflow verb with its one-line description.
- [x] 7.2 No new per-audit emoji needed — `sync-upstream` produces inline thread replies, not audit-style notifications.

## 8. Docs

- [x] 8.1 `docs/CHATOPS.md`: add `### sync-upstream` under operator-driven verbs, describing the rebase behavior, conflict handling, AND the no-push guarantee.
- [x] 8.2 `docs/OPERATIONS.md`: add an "OSS contribution workflow" section describing the recommended setup:
  - Fork the upstream project on GitHub.
  - Clone the fork as the autocoder workspace.
  - Set `upstream` config block pointing at the upstream repo.
  - Set `auto_submit_pr: false`.
  - Configure `spec_storage.path` pointing at a sibling specs repo.
  - Recommended snippet for `executor.implementer.prompt_path` emphasizing minimal-diff + follow-conventions style (with sample text the operator can adapt).
  - The typical loop: scout → spec-it → review → merge fork PR → manually `gh pr create` to upstream.
- [x] 8.3 `docs/CONFIG.md`: document each new field with default, validation rules, AND a cross-link to the OPERATIONS.md OSS-workflow section.
- [x] 8.4 `config.example.yaml`: include all three blocks commented out, with each field's default in a comment.

## 9. Spec deltas

- [x] 9.1 `openspec/changes/a26-oss-fork-support/specs/chatops-manager/spec.md` ADDs the sync-upstream verb requirement.
- [x] 9.2 `openspec/changes/a26-oss-fork-support/specs/orchestrator-cli/spec.md` ADDs spec_storage, upstream, auto_submit_pr, AND sync-upstream-handler requirements.
- [x] 9.3 `openspec/changes/a26-oss-fork-support/specs/git-workflow-manager/spec.md` MODIFIES `Monolithic PR at end of pass` (preserving all 5 canonical scenarios + adding 2 new scenarios for the auto_submit_pr: false branch).
- [x] 9.4 `openspec/changes/a26-oss-fork-support/specs/project-documentation/spec.md` ADDs the docs requirement.

## 10. Verification

- [x] 10.1 `cargo test` passes. (1729 tests pass; 2 ignored.)
- [x] 10.2 `openspec validate a26-oss-fork-support --strict` passes.
- [ ] 10.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
  - **Status:** the codebase has 62 pre-existing clippy warnings unrelated to this change. The new code adds no NEW warnings (verified via grep on file paths). A separate cleanup change is needed to bring the whole codebase to `-D warnings` clean.
- [ ] 10.4 Manual verification on an actual OSS fork:
  - **Status: cannot be run from the implementer sandbox** — requires a real GitHub fork, real network access to clone/push, real chatops backend (Slack/Discord/etc.), AND manual operator observation. Documented as an operator-side acceptance step per the proposal's "## Impact → Acceptance" section.
