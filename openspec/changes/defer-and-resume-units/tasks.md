# Tasks

OpenSpec: implements the `defer`/`undefer` deltas in `specs/chatops-manager/spec.md`
and `specs/orchestrator-cli/spec.md`.

## 1. Chatops verb parsing

- [ ] 1.1 In `autocoder/src/chatops/operator_commands.rs`, add two variants to the
  `OperatorCommand` enum (defined at `operator_commands.rs:256`): `Defer { repo_substring: String, slug: String }` and `Undefer { repo_substring: String, slug: String }`. Mirror the field shape of `ClearPermaStuck { repo_substring, change }`.
- [ ] 1.2 In the verb dispatcher match (`parse_command_outcome_in_thread`, the `match verb.to_ascii_lowercase().as_str()` at `operator_commands.rs:647`), add arms for `"defer"` and `"undefer"` that parse `<repo-substring> <slug>` into the new variants. Follow the two-argument parsing of the `clear-perma-stuck` arm (~`operator_commands.rs:660`). Reuse the existing argument-sanitization path the other verbs use.
- [ ] 1.3 Add `defer` and `undefer` to the help verb's verb list (the `"help"` arm, ~`operator_commands.rs:746`) with a one-line description each, so they appear in `@<bot> help`.

## 2. Chatops dispatch to control socket

- [ ] 2.1 In the dispatcher (`dispatch`, `operator_commands.rs:2982`), add arms for `OperatorCommand::Defer` and `OperatorCommand::Undefer`. Resolve the repo with `match_repo` (`operator_commands.rs:1470`); on `RepoMatch::None` reply via `format_no_match`, on `RepoMatch::Multiple` reply via `format_multiple_matches` (the same helpers the other arms call).
- [ ] 2.2 On a unique repo, submit a control-socket action through the existing `ActionSubmitter::submit` (trait at `operator_commands.rs:2575`): `{"action": "defer_unit", "url": <repo.url>, "slug": <slug>}` and `{"action": "undefer_unit", "url": <repo.url>, "slug": <slug>}`. Mirror the JSON-action construction of the `clear_perma_stuck_marker` submission.
- [ ] 2.3 Render the control-socket JSON response into a single operator reply: `✓` on a performed/no-op-success outcome (deferred / already-deferred / resumed / already-active, with the slug and repo), `✗` on a clear error (not-found, ambiguous). Do NOT add a pending-confirmation store and do NOT add a `defer-confirm` verb — these commands are single-ack (contrast `rollback_pending` / `take_valid` at `operator_commands.rs`~3498-3513).

## 3. Control-socket actions

- [ ] 3.1 In `autocoder/src/control_socket.rs`, add `"defer_unit"` and `"undefer_unit"` arms to the action dispatch match (`match action.as_str()` at `control_socket.rs:779`), routing to `handle_defer_unit` and `handle_undefer_unit`.
- [ ] 3.2 Implement `handle_defer_unit(parsed, state)` and `handle_undefer_unit(parsed, state)` following the shape of `handle_clear_perma_stuck` (`control_socket.rs:1373`) for argument extraction (`require_str(parsed, "url")`, `require_str(parsed, "slug")`) and repo resolution (`find_repo`, `workspace::resolve_path`), and of `handle_rollback_recovery` (`control_socket.rs:3230`) for the agent-branch + PR mechanism (next section). Return a JSON `{"ok": ..., "outcome": ..., "slug": ..., "url": ...}` the dispatcher renders.

## 4. Auto-detect change vs issue (defer) and deferred unit (undefer)

- [ ] 4.1 Add a detection helper that, for a defer, locates the unit by checking `openspec/changes/<slug>/` (a change, mirroring `CHANGES_SUBDIR` at `queue.rs:16`) and `issues/<slug>.md` / `issues/<slug>/` (an issue, mirroring the two forms `list_ready` accepts at `lanes/issues.rs:360-423`). Exactly one present → that unit; neither → not-found error; both → ambiguous error naming both candidate paths.
- [ ] 4.2 For an undefer, the inverse: check `deferred-changes/<slug>/` then `deferred-issues/<slug>.md` / `deferred-issues/<slug>/`. Same not-found / ambiguous handling.
- [ ] 4.3 Idempotency: a defer whose lane location is absent but whose deferred location is present returns an already-deferred no-op success (no commit, no PR). An undefer whose deferred location is absent but whose lane location is present returns an already-active no-op success.

## 5. The move, on the agent branch, riding the PR flow

- [ ] 5.1 Implement the directory move as the inverse pair: defer moves `openspec/changes/<slug>/` → `deferred-changes/<slug>/` (change) or `issues/<slug>.md`→`deferred-issues/<slug>.md` / `issues/<slug>/`→`deferred-issues/<slug>/` (issue, form preserved); undefer moves each back. Create the destination parent dir if absent. Do NOT clear any in-unit or sibling markers — preserve the unit as-is.
- [ ] 5.2 Perform the move on the recreated agent branch: in the handler, run the same preamble as `handle_rollback_recovery` (`control_socket.rs`~3246-3276) — `workspace::ensure_initialized`, `git::checkout(base_branch)`, `git::reset_hard_to_remote(base_branch)` — then `git::recreate_branch(workspace, &repo.agent_branch)` (`git.rs:274-277`), perform the move, then `git::add_all` + `git::commit` (`git.rs:279-291,335-338`) with a clear message (e.g. `chore: defer <slug>` / `chore: resume <slug>`). This mirrors `octopus_guide::provision_on_agent_branch` (`octopus_guide.rs:255-290`).
- [ ] 5.3 Push and open the PR honoring `auto_submit_pr`: `git::push_force_with_lease(workspace, &repo.agent_branch, push_remote)` (`git.rs:428-435`), then the PR-creation path. When `repo.auto_submit_pr` is false, return the `branch_pushed_no_pr` outcome (mirror `pr_open.rs:168-192`) rather than calling the PR-open API. The PR body states what was deferred/resumed and from/to which location.
- [ ] 5.4 Do NOT commit to the base branch directly — a base commit diverges from `origin/<base>`, breaks the per-pass `git pull --ff-only` (`pass.rs:473-475`), and is wiped by `attempt_dirty_workspace_recovery` (`git reset --hard origin/<base>` + `git clean -fd`, `pass.rs:607-612`).

## 6. Lanes ignore deferred (no lane code change)

- [ ] 6.1 No edit to `queue.rs` or `lanes/issues.rs` is required. Confirm by inspection that `list_pending` reads only `changes_dir` = `openspec/changes/` (`queue.rs:52-138`) and `list_ready` reads only `issues_dir` = `issues/` (`lanes/issues.rs:360-423`), so `deferred-changes/` and `deferred-issues/` at the repo root are never enumerated. Add the regression tests in section 7 that assert this.

## 7. Tests

- [ ] 7.1 Parser: `@<bot> defer <repo> <slug>` and `@<bot> undefer <repo> <slug>` parse into `OperatorCommand::Defer` / `Undefer` with the expected `repo_substring` and `slug` (test the parsed variant, not the reply wording).
- [ ] 7.2 Detection (defer): a slug present only under `openspec/changes/<slug>/` detects as a change; present only under `issues/<slug>.md` and only under `issues/<slug>/` each detect as an issue; absent from both → not-found; present in both → ambiguous.
- [ ] 7.3 Detection (undefer): a slug present only under `deferred-changes/<slug>/` and only under `deferred-issues/<slug>(.md|/)` each detect correctly; absent → not-found.
- [ ] 7.4 Move: after deferring a change, `openspec/changes/<slug>/` is gone and `deferred-changes/<slug>/` exists with identical contents; after deferring an issue, the single-file vs directory form is preserved at `deferred-issues/<slug>`. Undefer is the exact inverse.
- [ ] 7.5 Lanes ignore deferred: with a unit under `deferred-changes/<slug>/`, `queue::list_pending` does not return it; with a unit under `deferred-issues/<slug>(.md|/)`, `lanes::issues::list_ready` does not return it.
- [ ] 7.6 Idempotency: deferring an already-deferred slug is a no-op success (no new commit); undeferring an already-active slug is a no-op success.
- [ ] 7.7 Mechanism: the deferred move lands on `repo.agent_branch` (not base) and, with `auto_submit_pr=false`, yields the `branch_pushed_no_pr` outcome; with `auto_submit_pr` true/default, the PR-open path is taken. Assert via the handler's returned outcome / branch, not message wording.

## 8. Docs

- [ ] 8.1 Add `defer` / `undefer` to `docs/CHATOPS.md`'s operator-verb reference: the verb syntax, the auto-detection of change vs issue, the deferred locations, that it rides the PR flow, and the single-ack (no two-step confirm) contrast with `rollback` / `wipe-workspace`.
