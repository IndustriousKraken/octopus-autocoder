# Tasks

OpenSpec: implements the `defer`/`undefer` deltas in `specs/chatops-manager/spec.md`
and `specs/orchestrator-cli/spec.md`.

## 1. Chatops verb parsing

- [x] 1.1 In `autocoder/src/chatops/operator_commands.rs`, add two variants to the
  `OperatorCommand` enum (defined at `operator_commands.rs:256`): `Defer { repo_substring: String, slug: String }` and `Undefer { repo_substring: String, slug: String }`. Mirror the field shape of `ClearPermaStuck { repo_substring, change }`.
- [x] 1.2 In the verb dispatcher match (`parse_command_outcome_in_thread`, the `match verb.to_ascii_lowercase().as_str()` at `operator_commands.rs:647`), add arms for `"defer"` and `"undefer"` that parse `<repo-substring> <slug>` into the new variants. Follow the two-argument parsing of the `clear-perma-stuck` arm (~`operator_commands.rs:660`). Reuse the existing argument-sanitization path the other verbs use.
- [x] 1.3 Add `defer` and `undefer` to the help verb's verb list (the `"help"` arm, ~`operator_commands.rs:746`) with a one-line description each, so they appear in `@<bot> help`.

## 2. Chatops dispatch to control socket

- [x] 2.1 In the dispatcher (`dispatch`, `operator_commands.rs:2982`), add arms for `OperatorCommand::Defer` and `OperatorCommand::Undefer`. Resolve the repo with `match_repo` (`operator_commands.rs:1470`); on `RepoMatch::None` reply via `format_no_match`, on `RepoMatch::Multiple` reply via `format_multiple_matches` (the same helpers the other arms call).
- [x] 2.2 On a unique repo, submit a control-socket action through the existing `ActionSubmitter::submit` (trait at `operator_commands.rs:2575`): `{"action": "defer_unit", "url": <repo.url>, "slug": <slug>}` and `{"action": "undefer_unit", "url": <repo.url>, "slug": <slug>}`. Mirror the JSON-action construction of the `clear_perma_stuck_marker` submission.
- [x] 2.3 Render the control-socket JSON response into a single operator reply: `✓` on a performed/no-op-success outcome (deferred / already-deferred / resumed / already-active, with the slug and repo), `✗` on a clear error (not-found, ambiguous). Do NOT add a pending-confirmation store and do NOT add a `defer-confirm` verb — these commands are single-ack (contrast `rollback_pending` / `take_valid` at `operator_commands.rs`~3498-3513).

## 3. Control-socket actions

- [x] 3.1 In `autocoder/src/control_socket.rs`, add `"defer_unit"` and `"undefer_unit"` arms to the action dispatch match (`match action.as_str()` at `control_socket.rs:779`), routing to `handle_defer_unit` and `handle_undefer_unit`.
- [x] 3.2 Implement `handle_defer_unit(parsed, state)` and `handle_undefer_unit(parsed, state)` following the shape of `handle_clear_perma_stuck` (`control_socket.rs:1373`) for argument extraction (`require_str(parsed, "url")`, `require_str(parsed, "slug")`) and repo resolution (`find_repo`, `workspace::resolve_path`), and of `handle_rollback_recovery` (`control_socket.rs:3230`) for the agent-branch + PR mechanism (next section). Return a JSON `{"ok": ..., "outcome": ..., "slug": ..., "url": ...}` the dispatcher renders.

## 4. Auto-detect change vs issue (defer) and deferred unit (undefer)

- [x] 4.1 Add a detection helper that, for a defer, locates the unit by checking `openspec/changes/<slug>/` (a change, mirroring `CHANGES_SUBDIR` at `queue.rs:16`) and `issues/<slug>.md` / `issues/<slug>/` (an issue, mirroring the two forms `list_ready` accepts at `lanes/issues.rs:360-423`). Exactly one present → that unit; neither → not-found error; both → ambiguous error naming both candidate paths.
- [x] 4.2 For an undefer, the inverse: check `deferred-changes/<slug>/` then `deferred-issues/<slug>.md` / `deferred-issues/<slug>/`. Same not-found / ambiguous handling.
- [x] 4.3 Idempotency: a defer whose lane location is absent but whose deferred location is present returns an already-deferred no-op success (no commit, no PR). An undefer whose deferred location is absent but whose lane location is present returns an already-active no-op success.

## 5. The move, on the agent branch, riding the PR flow

- [x] 5.1 Implement the directory move as the inverse pair: defer moves `openspec/changes/<slug>/` → `deferred-changes/<slug>/` (change) or `issues/<slug>.md`→`deferred-issues/<slug>.md` / `issues/<slug>/`→`deferred-issues/<slug>/` (issue, form preserved); undefer moves each back. Create the destination parent dir if absent. Do NOT clear any in-unit or sibling markers — preserve the unit as-is.
- [x] 5.2 Acquire exclusive workspace access FIRST, then perform the move on the recreated agent branch. The defer/undefer handler is a workspace-mutating control-socket op, so per `a01-workspace-ops-preempt-and-serialize` it MUST call that change's shared `preempt_and_acquire_busy_marker` helper before any workspace mutation — preempting any in-flight pass AND holding the per-repo busy marker — and release the marker after the move (on success OR failure). Without this the handler races a concurrent agentic session writing the same workspace (the exact `git add -A: Unable to write new index file` corruption a01 fixes for rollback). THEN run the same preamble as `handle_rollback_recovery` (`control_socket.rs`~3246-3276) — `workspace::ensure_initialized`, `git::checkout(base_branch)`, `git::reset_hard_to_remote(base_branch)` — then `git::recreate_branch(workspace, &repo.agent_branch)` (`git.rs:274-277`), perform the move, then `git::add_all` + `git::commit` (`git.rs:279-291,335-338`) with a clear message (e.g. `chore: defer <slug>` / `chore: resume <slug>`). This mirrors `octopus_guide::provision_on_agent_branch` (`octopus_guide.rs:255-290`) AND the rollback handler's a01-added preempt+lock.
- [x] 5.3 Push and open the PR honoring `auto_submit_pr`: `git::push_force_with_lease(workspace, &repo.agent_branch, push_remote)` (`git.rs:428-435`), then the PR-creation path. When `repo.auto_submit_pr` is false, return the `branch_pushed_no_pr` outcome (mirror `pr_open.rs:168-192`) rather than calling the PR-open API. The PR body states what was deferred/resumed and from/to which location.
- [x] 5.4 Do NOT commit to the base branch directly — a base commit diverges from `origin/<base>`, breaks the per-pass `git pull --ff-only` (`pass.rs:473-475`), and is wiped by `attempt_dirty_workspace_recovery` (`git reset --hard origin/<base>` + `git clean -fd`, `pass.rs:607-612`).

## 6. Lanes ignore deferred (no lane code change)

- [x] 6.1 No edit to `queue.rs` or `lanes/issues.rs` is required. Confirm by inspection that `list_pending` reads only `changes_dir` = `openspec/changes/` (`queue.rs:52-138`) and `list_ready` reads only `issues_dir` = `issues/` (`lanes/issues.rs:360-423`), so `deferred-changes/` and `deferred-issues/` at the repo root are never enumerated. Add the regression tests in section 7 that assert this.

## 7. Tests

- [x] 7.1 Parser: `@<bot> defer <repo> <slug>` and `@<bot> undefer <repo> <slug>` parse into `OperatorCommand::Defer` / `Undefer` with the expected `repo_substring` and `slug` (test the parsed variant, not the reply wording).
- [x] 7.2 Detection (defer): a slug present only under `openspec/changes/<slug>/` detects as a change; present only under `issues/<slug>.md` and only under `issues/<slug>/` each detect as an issue; absent from both → not-found; present in both → ambiguous.
- [x] 7.3 Detection (undefer): a slug present only under `deferred-changes/<slug>/` and only under `deferred-issues/<slug>(.md|/)` each detect correctly; absent → not-found.
- [x] 7.4 Move: after deferring a change, `openspec/changes/<slug>/` is gone and `deferred-changes/<slug>/` exists with identical contents; after deferring an issue, the single-file vs directory form is preserved at `deferred-issues/<slug>`. Undefer is the exact inverse.
- [x] 7.5 Lanes ignore deferred: with a unit under `deferred-changes/<slug>/`, `queue::list_pending` does not return it; with a unit under `deferred-issues/<slug>(.md|/)`, `lanes::issues::list_ready` does not return it.
- [x] 7.6 Idempotency: deferring an already-deferred slug is a no-op success (no new commit); undeferring an already-active slug is a no-op success.
- [x] 7.7 Mechanism: the deferred move lands on `repo.agent_branch` (not base) and, with `auto_submit_pr=false`, yields the `branch_pushed_no_pr` outcome; with `auto_submit_pr` true/default, the PR-open path is taken. Assert via the handler's returned outcome / branch, not message wording.
- [x] 7.8 Preempt + serialize (per a01): deferring while a pass is in flight preempts that pass AND the handler holds the per-repo busy marker for the duration of the move — assert the busy marker is acquired before the move and the in-flight pass is cancelled (behaviour/state, not message wording). Mirror a01's preempt-and-serialize tests for the rollback handler.

## 8. Docs

- [x] 8.1 Add `defer` / `undefer` to `docs/CHATOPS.md`'s operator-verb reference: the verb syntax, the auto-detection of change vs issue, the deferred locations, that it rides the PR flow, and the single-ack (no two-step confirm) contrast with `rollback` / `wipe-workspace`.

## 9. Conformance corrections (post-review)

- [x] 9.1 Preempt acknowledgement (canon conformance): `defer`/`undefer` preempt an in-flight pass, so per the now-folded `Preempting in-flight work is acknowledged to the operator` requirement they MUST emit the operator-facing preempt ack. Thread `preempted_change` from `preempt_and_acquire_busy_marker` into the `handle_defer_unit`/`handle_undefer_unit` JSON response (as `handle_rollback_recovery` does), AND in the chatops dispatcher emit the preempt acknowledgement naming the operation + the cancelled change when the response carries a non-null `preempted_change` — reusing the SAME best-effort ack path the `rollback`-confirm arm uses (a01). When `preempted_change` is null, post only the result reply. Add a test asserting the ack fires iff a preempt occurred (behaviour/state, not message wording).
- [x] 9.2 Detect-before-preempt for the no-op (avoid disrupting a pass for nothing): restructure `handle_defer_or_undefer` so the already-deferred / already-active no-op is detected READ-ONLY (clone-if-absent is fine; no checkout/reset) BEFORE calling `preempt_and_acquire_busy_marker`. An already-done request returns its no-op success WITHOUT preempting any in-flight pass or acquiring the busy marker. Only when an actual move is required does the handler preempt + acquire. Add a test: an already-deferred `defer` issued while a pass is in flight does NOT cancel the pass (the iteration-cancel token is not fired) and acquires no marker.
