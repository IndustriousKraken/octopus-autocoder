# Implementation tasks

> Throughout: this is a **behavior-preserving** refactor. Move and regroup code; do not change production control flow except to collapse provably-equivalent duplicates and to split oversized functions. Functions called from outside the module keep their signatures. Reference functions by name (line numbers drift as you go).

## 1. Establish the module directory

- [ ] 1.1 Convert `src/polling_loop.rs` into a directory module `src/polling_loop/` with `mod.rs` (or keep `polling_loop.rs` as the crate-facing module that `mod`-declares the submodules — match the crate's existing convention). Re-export from the module root every item that callers outside the module currently reference via `polling_loop::…`, so no call site outside the module changes.
- [ ] 1.2 Keep the orchestration core in the module root: `run`, `run_with_hooks`, `execute_one_pass`, `run_pass_through_commits`, `ChatOpsContext`, `RunHooks`, `IterationGuard`, the jitter/sleep helpers, and `test_hooks`. Target: the core stays under the file-size budget (~1,000 lines after the extractions below).

## 2. Extract responsibility submodules

Move the named functions into submodules by responsibility. Exact seams are a judgment call along these responsibility lines; the goal is each submodule under the file-size budget.

- [ ] 2.1 `alerts/` — the `maybe_post_*` / `post_*_alert` helpers and their formatting/truncation helpers (`maybe_post_revise_*`, `maybe_post_code_review_*`, `maybe_post_rereview_suggestion_alert`, `maybe_post_contradiction_findings_alert`, `maybe_post_unarchivable_deltas_alert`, `maybe_post_spec_revision_alert`, `post_perma_stuck_alert`, `post_stuck_alert`, `maybe_post_start_of_work`, `maybe_post_pr_opened`, `maybe_post_branch_pushed_no_pr`, `maybe_post_refork_notification`, the rebuild notifications, `truncate_reason`, `filter_alert_state_lines`, `compose_branch_url`).
- [ ] 2.2 `queue/` — `walk_queue`, `process_one_pending_change`, `escalate_to_chatops`, `ResumeDisposition`, `QueueStep` (walk concerns); `process_waiting_changes`, `process_one_waiting`, `run_due_audits_after_queue` (waiting concerns).
- [ ] 2.3 `preflight.rs` — `handle_archivability_preflight`, `build_unarchivable_revision_suggestion`, `handle_contradiction_preflight`, `build_contradiction_revision_suggestion`, `apply_archive_collision_preflight`, `handle_failure_counter`.
- [ ] 2.4 `review_context.rs` — `build_review_context`, `build_per_change_contexts`, `synthesize_per_change_report`, `verdict_label`, `worst_verdict`, the reviewer-revision comment helpers (`post_reviewer_revision_comments`, `partition_and_annotate_reviewer_revisions`, `annotate_dropped_*`, `concerns_to_owned`).
- [ ] 2.5 `pr_open.rs` — `open_pull_request`, `create_pull_request_via_hook`, `initial_revision_state_at_pr_open`, `open_rebuild_pull_request`, `open_triage_pull_request`, `open_pr_exists_for_agent_branch(_at)`. `pr_body.rs` — `build_pr_title`, `build_pr_body`, `build_rebuild_pr_body`, the audit-only PR title/body builders, the why-section readers (`read_change_why`, `read_proposal_why_from_archive`, `extract_why_section`), and the implementer-summary helpers. (Split across two files if either exceeds budget.)
- [ ] 2.6 `rebuild.rs` — `execute_rebuild_iteration` and its rename/abort formatting helpers.
- [ ] 2.7 `triage.rs` — `process_audit_triages`, `process_completed_triage`, the triage slug/hash/scope helpers, and the git-scrub helpers (`discard_non_spec_writes`, `remove_non_spec_path_from_disk`, `path_exists_in_head`, `run_git_revert`, `build_canonical_specs_index`, `mark_triage_failed`). (Split the git-scrub helpers into their own file if over budget.)
- [ ] 2.8 `proposals.rs` — `process_proposal_requests`, `process_completed_proposal`, `mark_proposal_failed`, `derive_unique_chat_request_slug`, `short_request_excerpt`.
- [ ] 2.9 `outcome.rs` — `handle_outcome`, `handle_iteration_requested`, `run_iteration_requested_steps`, the commit-subject builders, `is_lazy_archive`, `has_executor_changes`, `first_line_of_section`.
- [ ] 2.10 Keep visibility minimal: items used only within a submodule become private; items used across submodules become `pub(crate)` or `pub(in crate::polling_loop)` as needed. Do not widen anything to `pub` that wasn't already.

## 3. Collapse the near-identical alert families (behavior-preserving)

- [ ] 3.1 Collapse the per-comment-dedup family (`maybe_post_revise_picked_up/succeeded/failed`, `maybe_post_code_review_triggered/complete/failed`) into one parameterized helper taking the dedup key (comment_id + kind) and a body variant (inline vs. threaded-with-cap). Prove equivalence: the rendered text and dedup/save side-effects for each original call site are unchanged.
- [ ] 3.2 Collapse the throttle family (`maybe_post_contradiction_findings_alert`, `maybe_post_unarchivable_deltas_alert`, `maybe_post_spec_revision_alert`, `post_perma_stuck_alert`) into one parameterized helper taking the throttle key and a body closure.
- [ ] 3.3 Delete the now-duplicate constants and strings (the two byte-identical `35_000` thread-cap constants collapse to one; the duplicated journalctl truncation tail string collapses to one).

## 4. Split oversized functions

- [ ] 4.1 Split any function still over the function-size budget along its internal phases — in particular `run_with_hooks` (the main loop) into named phase functions — so no resulting function exceeds the budget. Behavior unchanged.

## 5. Relocate tests and prune wording-assertion tests

- [ ] 5.1 Move the inline `#[cfg(test)] mod tests` to a sibling `#[path]` test module (e.g. `#[path = "polling_loop_tests.rs"] mod tests;` declared from the module root, keeping `use super::*` so crate-private items and the `test_hooks` override still resolve). If the suite is large, split it into per-responsibility sibling test files. No test loses access to the items it exercises.
- [ ] 5.2 During the move, delete or rewrite every test that asserts a hand-authored substring of a shipped alert / notification / PR-body / marker message, per the `Tests assert behavior or derivation, never message wording` requirement. Rewrite to assert the behavioral property (presence of a derived value, ordering, char-boundary safety, dedup/throttle side-effects) using synthetic fixtures — never the rendered copy. Keep the genuine behavioral and boundary tests.
- [ ] 5.3 Where a collapsed alert helper (§3) replaces six call sites, replace the six per-variant wording tests with one behavioral test of the parameterized helper (asserts the dedup/throttle/save side-effects and that the correct body variant is selected, not the message text).

## 6. Acceptance gate

- [ ] 6.1 `cargo test` passes for the autocoder crate (surviving suite).
- [ ] 6.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [ ] 6.3 No file under `src/polling_loop/` exceeds the file-size budget; no function exceeds the function-size budget (i.e. an `architecture-brightline` run over the module produces no file/function size finding for it).
- [ ] 6.4 Behavior-preservation check: the diff relocates code and collapses provably-equivalent duplicates only — no production control-flow change — and every function called from outside the module keeps its signature.
- [ ] 6.5 `openspec validate a68-decompose-polling-loop --strict` passes.
