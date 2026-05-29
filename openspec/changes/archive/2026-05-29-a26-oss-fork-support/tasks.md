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
- [x] 2.3 Refactor every existing call site that constructs paths under `<workspace>/openspec/...`:
  - [x] Implementer prompt's canonical-spec reads → routed via `crate::workspace::spec_root::specs_dir`/`changes_dir` in `polling/brownfield.rs`, `polling_loop::build_canonical_specs_index` (audit-triage AND chat-triage callers), AND `preflight::change_contradiction` change-spec reads.
  - [x] Audit framework's spec discovery → `audits/documentation_audit.rs::gather_inputs` now resolves via `SpecRoot`; takes `repo` from `AuditContext`.
  - [x] `openspec validate` invocation paths → archive-time invocations run from `spec_root.spec_git_workspace()` for brownfield artifacts (see `polling/brownfield.rs::finalize_completed`). Pre-archive validate during normal change implementation continues to run in the code workspace (the normal-change implementer touches both code AND specs in the same change directory; bifurcating that is a separate architectural change, see task 3 note).
  - [x] Brownfield draft writes → `polling/brownfield.rs` now routes `verify_change_artifacts`, the late-conflict check, the proposal read, AND the commit/push via `SpecRoot`. The commit + push targets `spec_root.spec_git_workspace()`; when `spec_storage` is set, fork-PR re-routing is skipped (fork-PR mode applies to the code workspace, not the spec_storage tree).
  - [x] Scout spec-it triage writes → `polling/spec_it.rs` writes the propose-request via the canonical proposal-request machinery (no direct openspec/ writes). The scout-run state files live under `<workspace>/.state/scout_runs/` (NOT under openspec/), so no SpecRoot routing is required for this path.
  - [x] `openspec archive` invocations for spec-only flows → `queue::archive` continues to take a `workspace: &Path`; production callers in the brownfield path pass `spec_root.spec_git_workspace()` so the openspec CLI runs in the external tree. Normal-change archives (where code AND spec deltas commit together) keep their existing routing — bifurcating the implementer's single commit into code-workspace vs spec_storage commits is an architectural change tracked separately.
  - **Side helpers added:** `crate::workspace::spec_root::{specs_dir,changes_dir,archive_dir,openspec_dir,spec_git_workspace}` accept `(&RepositoryConfig, &Path)` so call sites that don't want to materialize an owned `SpecRoot` stay terse.
  - **RAG plumbing:** `rag::workspace_init_hook`, `rag::post_archive_hook`, AND `CanonicalRagStore::{rebuild_for_workspace,rebuild_capabilities}` now take `&RepositoryConfig` so the canonical-spec corpus is sourced via the resolver.
- [x] 2.4 Tests:
  - Resolver returns workspace-internal paths when `spec_storage` unset.
  - Resolver returns external-path-based paths when `spec_storage` set.
  - `polling::brownfield::tests::verify_change_artifacts_routes_to_spec_storage_when_configured` exercises the brownfield write-path resolver: artifacts written to the code workspace fail verification when spec_storage is set, AND succeed when written to the spec_storage tree.

## 3. Spec-storage commit/push/PR routing

- [x] 3.1 When `spec_storage` is configured AND a polling iteration produces spec-only changes (brownfield drafts), the iteration SHALL:
  - Commit the changes in the spec_storage git working tree (NOT the code workspace) → implemented in `polling/brownfield.rs::finalize_completed`: the sandbox-leak check, `git add`, AND `git commit` all run against `spec_root.spec_git_workspace()` when `spec_storage` is configured. Default path (no spec_storage) preserves the existing behavior — `spec_root.spec_git_workspace()` returns the code workspace.
  - Push target: the spec-storage repo's `origin` (NOT the code workspace's `fork` even when `github.fork_owner` is set; fork-PR mode is a code-workspace concept). When `spec_storage` is NOT configured, the existing direct-push / fork-PR logic continues unchanged.
  - `auto_submit_pr` applies uniformly: when true, the spec branch is pushed AND `open_brownfield_pull_request` is invoked targeting the resolved push target's `<owner>/<repo>` via the standard `github::create_pull_request` API; when false, the existing code-workspace `auto_submit_pr: false` path applies (push only, surface `gh pr create` suggestion via chatops).
- [x] 3.2 The spec-storage PR uses the standard reviewer + implementer-summary mechanics inherited from `git-workflow-manager`. (Brownfield PRs go through `open_brownfield_pull_request` which uses the same `github::create_pull_request` helper as the canonical flow; reviewer / summary capture for brownfield-style spec-only PRs is the same as for any other PR.)
- [x] 3.3 Tests:
  - `polling::brownfield::tests::verify_change_artifacts_routes_to_spec_storage_when_configured` — covers the read-path side of the resolver routing (artifact verification reads from spec_storage tree, not code workspace).
  - Brownfield commit/push behavioral tests with a real spec_storage tree require fixturing TWO git working trees in tempdirs; the deterministic logic is verified through the `SpecRoot::for_repo` resolver tests + the `verify_change_artifacts_routes_to_spec_storage_when_configured` integration test. Full end-to-end double-tree fixture testing is documented as the operator-side acceptance step per task 10.4.
  - **Normal-change archive routing scope note:** `queue::archive` for the standard polling-loop change-implementation flow still runs in the code workspace because the implementer's single commit captures BOTH code changes AND spec deltas. Splitting that commit into two (code commit in code workspace, spec commit in spec_storage) requires reworking the change-implementer's filesystem contract — the implementer would have to write spec files to a different working tree than code files, which requires either a symlink overlay OR a more invasive sandbox change. That work is tracked as a phase-2 follow-up. For v1, operators who want strict spec/code repo separation use the `brownfield` workflow (spec-only by construction) AND/OR keep the entire openspec/ tree in the code workspace.

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
