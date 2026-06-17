# Tasks

Cross-cutting constraints for every task below:

- **Behavior-preserving only.** No change to any observable contract —
  control-socket request/response JSON, chatops reply text, reviewer
  Markdown output, PR outcomes, or the CLI surface. This is a code
  reorganization, not a feature change.
- Keep public call sites compiling unchanged by re-exporting moved
  items (`pub(crate) use`) from their original module path where
  callers reference them.
- Preserve existing tests; when moving code that carries unit tests,
  place them in a sibling test module rather than a fresh inline
  `#[cfg(test)] mod tests` block in the new source file.
- Match the surrounding hand-formatting. Do **not** run `cargo fmt`
  (this crate is intentionally not rustfmt-clean; a format pass would
  produce a huge unrelated diff).
- Do not author or restate any size/duplication threshold as a spec
  requirement — these line counts are audit selectors, not contracts.

## 1. control_socket.rs — de-duplicate enqueue handlers + dispatch table

- [ ] Factor the shared enqueue body out of the `handle_queue_*`
      handlers (`handle_queue_proposal_request`,
      `handle_queue_changelog_request`, `handle_queue_brownfield_request`,
      `handle_queue_scout_request`, `handle_queue_spec_it_request`,
      `handle_queue_sync_upstream_request`,
      `handle_queue_brownfield_survey_request`,
      `handle_queue_brownfield_batch_request`, and the matching
      clear/revision handlers in `autocoder/src/control_socket.rs:1605-2470`)
      into one generic helper that performs: `require_str` for `url` +
      `request_id`, `find_repo`, look up the live `RepoTaskHandle`'s
      pending queue, de-dup by `request_id`, push, and return the
      `{ok, url, request_id, poll_interval_sec}` JSON. Each handler should
      then supply only its queue selector and its request-record
      constructor.
- [ ] Replace the hand-maintained `match action.as_str()` arm list in
      `dispatch_request` (`autocoder/src/control_socket.rs:612-643`) with a
      table-driven dispatch (action string → handler) so adding an action
      no longer means extending a giant match.
- [ ] Move the accreted handler bodies behind that dispatch table into a
      submodule (e.g. a `control_socket/` directory module or
      `control_socket/handlers.rs`) to bring the file under the size
      budget, keeping `dispatch_request` and `ControlState` reachable at
      their current paths.

## 2. operator_commands.rs — extract the `send it` cascade

- [ ] Extract the `send it` cascade and its audit/survey state-machine
      logic (`dispatch_send_it_on_audit`, `try_send_it_on_survey`,
      `try_send_it_on_issue_candidate`, `try_send_it_on_revision`, and the
      audit-thread lookup at
      `autocoder/src/chatops/operator_commands.rs:3491-4047`) into its own
      module, out of `operator_commands.rs`.
- [ ] Preserve the canon-mandated context resolution order exactly:
      audit → survey → issue-candidate → revision → canonical refusal.
      The extraction must not reorder or drop any context.

## 3. code_reviewer.rs — extract the agentic reviewer transport

- [ ] Move the self-contained "Agentic reviewer transport (a58)" block
      (`autocoder/src/code_reviewer.rs:882-1588`: the role/tooling consts
      and `agentic_review_allowed_tools`, `RawReviewConcern` /
      `RawReviewSubmission`, `payload_to_review_result`,
      `render_review_submission_markdown`, `render_agentic_review_prompt`,
      the `ReviewSessionRunner` trait, `CliReviewSessionRunner`, the
      `run_agentic_review_*` orchestration, and `resolve_reviewer_strategy`)
      into its own module (e.g. `code_reviewer/agentic.rs` or
      `code_reviewer_agentic.rs`).
- [ ] Re-export the items that callers outside the reviewer reference so
      existing paths keep compiling.

## 4. revisions.rs — collapse the duplicated outcome arms

- [ ] In `process_one_pr` (`autocoder/src/revisions.rs:624-1539`), factor
      the repeated post-processing shared across the executor-outcome
      arms (`Completed`, `AskUser`, `Failed`, `PreconditionUnmet`,
      `SpecNeedsRevision`, `IterationRequested`, `Aborted`) into a helper
      so each arm carries only its unique logic.
- [ ] Split the remaining body along its internal phases into smaller
      functions so the orchestration is no longer one ~915-line function,
      keeping the outcome semantics identical.

## 5. cli/install.rs — extract the --reconfigure subsystem

- [ ] Move the `--reconfigure` subsystem
      (`autocoder/src/cli/install.rs:2126-2607`:
      `resolve_existing_config_path`, `execute_reconfigure`,
      `section_label`, `print_restart_guidance`, `reconfigure_audits`,
      `reconfigure_reviewer`, `reconfigure_chatops`,
      `apply_in_place_patch`, `prior_file_mode`, and the
      `ReconfigureSection` plumbing) into a new
      `autocoder/src/cli/reconfigure.rs` module; register it in
      `autocoder/src/cli/mod.rs`.
- [ ] Update the `execute_inner` call site in `install.rs` to call into
      the new module. The `--reconfigure <audits|reviewer|chatops>` flag,
      its three-section allowlist, and all printed guidance must stay
      byte-for-byte unchanged.

## 6. Verify

- [ ] `cargo build` succeeds and the existing test suite passes (the two
      `sandbox::tests` that fail only for environmental reasons are not
      regressions and may be ignored). No `cargo fmt`.
