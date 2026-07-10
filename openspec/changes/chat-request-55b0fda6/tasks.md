# Tasks

OpenSpec: implements the ADDED requirement in `specs/orchestrator-cli/spec.md`.

## 1. Stop clearing the marker at revision-PR-open

- [ ] 1.1 In `autocoder/src/polling/revision_session.rs`, in `run_revision_execute`'s
  clean-re-gate success path (after the PR is opened, ~lines 1499–1515), DELETE the
  block that clears the marker: the comment starting `// Clear the .needs-spec-revision.json
  marker now that a revision PR is open.` AND the `match crate::queue::remove_revision_marker_idempotent(workspace, change_slug) { ... }`
  that follows it. Leave everything else on that path unchanged (the
  consecutive-failure reset at ~:1492, the `Acted` status flip at ~:1520, the
  `restore_base` at ~:1541, and the success thread reply). The marker MUST survive
  the clean-PR path so it keeps holding the change out of `list_pending`.
- [ ] 1.2 Do NOT touch the `@<bot> revise` marker-clear at
  `autocoder/src/revisions/process_pr.rs:1281` — that path is correct (its PR is
  on the agent branch, which the open-PR check parks). Only the spec-revision
  executor path in `revision_session.rs` changes.

## 2. Tell the operator the post-merge clear step

- [ ] 2.1 In the same success path, update the success thread reply (the
  `format!("✅ Revision PR opened for `{change_slug}`: {pr_url}\nReview + merge it
  to apply the revision.")` at ~line 1546) to also tell the operator that after
  merging they should run `@<bot> clear-revision <repo> <change>` to release the
  held change. Keep it to one added sentence; do not restructure the reply.

## 3. Tests

- [ ] 3.1 Invert the marker assertion in
  `clean_regate_resets_consecutive_failure_count`
  (`autocoder/src/polling/revision_session.rs`, ~lines 2887–2888): after a clean
  re-gate opens the PR, assert `crate::spec_revision::read_marker(&ws, "c1").unwrap().is_some()`
  with a message like `"a clean re-gate (PR opened) RETAINS the .needs-spec-revision.json
  marker so the change stays held out of list_pending until the revision lands"`.
  (The rest of that test — the consecutive-failure-count reset — is unchanged.)
- [ ] 3.2 Add a focused regression test in the same `mod tests` (drive
  `run_revision_execute` via the existing injected `ExecutorDeps` / `CannedReGate(ReGateOutcome::Clean)`
  / `FakePr` seams, no real subprocess or GitHub): a clean re-gate that opens the
  revision PR LEAVES the change's `.needs-spec-revision.json` marker present
  (`read_marker(...).unwrap().is_some()`) AND still opens the PR once AND flips the
  thread state to `Acted`. Assert the filesystem/marker state, not message wording.
- [ ] 3.3 Confirm `executor_clean_regate_opens_pr_and_flips_status` (~:2063) and
  `failed_round_keeps_marker_and_never_commits_to_base` (~:3355) still pass
  unchanged (the failed-round test already asserts the marker remains).

## 4. Validation

- [ ] 4.1 `cd autocoder && cargo test --bin autocoder revision_session` (the suite
  is known-flaky under parallel load — re-run / isolate any failure before treating
  it as real).
- [ ] 4.2 `openspec validate chat-request-55b0fda6 --strict` from the repo root.
