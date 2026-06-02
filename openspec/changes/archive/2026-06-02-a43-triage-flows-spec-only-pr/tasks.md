# Implementation tasks

## 1. Shared helper — discard non-spec writes from the working tree

- [x] 1.1 Add `pub fn discard_non_spec_writes(workspace: &Path, spec_slug: &str) -> Result<Vec<String>>` to `autocoder/src/polling_loop.rs` (OR a sibling module if the polling_loop file is unwieldy). The helper:
  1. Reads `git status --porcelain=v1 --untracked-files=all` to enumerate modified, added, AND untracked paths.
  2. Partitions paths: those starting with `openspec/changes/<spec_slug>/` are "kept"; everything else is "discarded."
  3. For each "discarded" path: if it's modified-AND-tracked, runs `git restore -- <path>`; if it's untracked, removes it via `std::fs::remove_file` (OR `remove_dir_all` for directories).
  4. Returns the sorted list of discarded paths so the caller can log AND surface to chatops.
- [x] 1.2 Unit tests for the helper covering: spec-only diff (returns empty list, no restores), code-only diff (returns code paths, restores them), mixed diff (returns only code paths, leaves spec paths alone), untracked-files mixed with modified-files, AND the no-op case when the working tree is clean.

## 2. Audit-triage completion handler

- [x] 2.1 In `autocoder/src/polling_loop.rs::process_completed_audit_triage` (~line 5994), replace the `(fixes_paths, spec_paths)` partition + two-PR creation logic with:
  1. Call `discard_non_spec_writes(workspace, &derived_slug)`. Bind the result as `discarded_paths`.
  2. If `discarded_paths` is non-empty: emit a `tracing::warn!` (carrying `url = %repo.url` per `a42` if landed) naming the audit type, the derived slug, AND the dropped paths. Then post a chatops threaded reply in the audit-thread naming the dropped paths AND the explanation `Per a43, code fixes go through the standard implementer pipeline. The spec PR has been opened; if the dropped fixes were load-bearing, revise the spec to capture them as tasks.md items.`
  3. After the discard, snapshot the changes-dir delta to determine whether ANY spec content exists at `openspec/changes/<derived_slug>/`.
  4. If no spec content: skip PR creation; post the chatops reply in the audit thread (still threaded) naming "no spec content produced; retry with a clearer directive." Flip the audit-thread `status` to `TriageFailed` (per existing patterns).
  5. If spec content exists: stage the spec dir, commit with the existing subject pattern (`audit-triage spec proposal from <audit_type>`), push the spec branch, open the spec PR via `open_triage_pull_request`. Flip `status` to `Acted`.
- [x] 2.2 Remove the `fixes_pr_url` Option variable AND its associated branch entirely. The function returns a `Result<()>` after at most one PR.
- [x] 2.3 The PR-body text for the spec PR drops the "see #<other_pr> for companion fixes PR" clause AND becomes: `This PR carries the new spec change(s) from the \`<audit_type>\` audit on \`<repo_url>\`. After merge, the next polling iteration's implementer will produce the code fixes through the standard pipeline.`
- [x] 2.4 The lifecycle-thread summary reply (lines ~6125-6140) drops the `Fixes PR: <url>` line; the reply now contains only `Spec PR: <url>` (or nothing if no PR opened, with the explanation already posted earlier in the handler).

## 3. Chat-triage completion handler

- [x] 3.1 In `autocoder/src/polling_loop.rs::process_completed_chat_triage` (~line 6632), apply the same restructure as task 2: discard non-spec writes, log + chatops-warn dropped paths, open at most one spec PR.
- [x] 3.2 The chat-triage spec PR's body text uses the analogous wording: `This PR carries the new spec change(s) from the \`propose\` request on \`<repo_url>\`. After merge, the next polling iteration's implementer will produce the code fixes through the standard pipeline.`
- [x] 3.3 The lifecycle-thread summary reply drops the `Fixes PR: <url>` line.

## 4. Prompt updates

- [x] 4.1 `prompts/audit-triage.md` — insert near the prompt's existing "what you do" framing (probably near the top, after the role line):
  ```
  Your writes are restricted to `openspec/changes/<new-slug>/`. Do NOT edit code outside that subtree. The daemon enforces this restriction by discarding any code-path writes BEFORE the spec PR commits, AND it posts a chatops warning naming what was dropped. After the operator merges the spec PR, the next polling iteration's implementer will pick up the new change AND write the code fixes through the standard pipeline. If the audit findings imply specific code-level fixes, capture them as concrete `tasks.md` items so the implementer knows exactly what to do; do NOT attempt the fixes yourself.
  ```
- [x] 4.2 `prompts/chat-request-triage.md` — insert the equivalent restriction. The wording mirrors task 4.1 but substitutes "the operator's request" for "the audit findings."
- [x] 4.3 Existing prompt content that describes the two-PR shape (any sentence mentioning "fixes PR" OR "two PRs" in either prompt) SHALL be removed.

## 5. PR-body composition

- [x] 5.1 In `polling_loop.rs::open_triage_pull_request` (the helper), the call sites are simplified by tasks 2 AND 3; the helper itself stays unchanged. Verify it still compiles AND tests pass.
- [x] 5.2 If any inline PR-body string literal still references "fixes PR" OR "companion" cross-link wording (a leftover from the old shape), edit it out.

## 6. Spec deltas + scenarios

- [x] 6.1 The two MODIFIED requirements in `specs/orchestrator-cli/spec.md` (a43's delta file) replace the existing canonical scenarios with the new ones per the spec delta. Verify that all canonical scenarios from the existing requirements appear in the MODIFIED body either (a) updated to reflect the new behavior OR (b) removed with explicit justification in the spec body (e.g., "the mixed-diff case is replaced by..."). Per the project memory rule `[[openspec-modified-requirements-preserve-canonical]]`, the MODIFIED header MUST match canonical exactly AND the body MUST contain every existing canonical scenario OR an explicit replacement.

## 7. Integration tests

- [x] 7.1 `autocoder/tests/` — add a test (OR extend an existing audit-triage test file) covering the full audit-triage flow with a mixed-diff executor outcome. Assert: one PR opened, the discarded paths chatops reply posted, the dropped paths restored on the working tree, the spec PR's diff contains only `openspec/changes/<slug>/` paths.
- [x] 7.2 Same shape test for chat-triage.
- [x] 7.3 Spec-only outcome test for both flows: no chatops warning, no restores, one PR with the spec content.
- [x] 7.4 Code-only outcome test for both flows: no PR opened, chatops reply explains "no spec content produced," working tree clean after the handler returns.
- [x] 7.5 Existing revision-loop test for triage PRs continues to pass (the PR is still a normal PR from the dispatcher's perspective).

## 8. Documentation updates

- [x] 8.1 `docs/CHATOPS.md` — the `send it` AND `propose` sections currently describe the two-PR shape; update to describe the spec-only-PR shape AND the "implementer follows on next iteration" sequence. Add a one-paragraph "what changed in a43" historical note if the existing wording is firmly bedded in operator memory.
- [x] 8.2 `docs/OPERATIONS.md` — if any operating note describes the audit-triage two-PR flow OR the cross-link between fixes AND spec PRs, update it to match the new shape.
- [x] 8.3 README.md — line referencing `send it`'s shape ("same two-PR shape as `propose`") in the verb table needs updating. Make the verb-table cell describe spec-only-PR behavior.

## 9. Acceptance gate

- [x] 9.1 `cargo test` passes for the autocoder crate, including the new integration tests.
- [x] 9.2 `openspec validate a43-triage-flows-spec-only-pr --strict` passes.
- [ ] 9.3 Manual end-to-end: drive a `send it` against a test repo's audit thread where the audit findings are clearly actionable. Verify exactly one spec PR opens, the working tree on the daemon side is clean after, the chatops reply names the spec PR URL, AND the next polling iteration picks up the new change AND opens an implementer PR through the standard flow. (NOT performed inside the autocoder sandbox — requires a live deployed daemon, a real chatops backend, and a configured test repo. This is a post-deploy operator verification step; the runtime behavior it checks is covered by the §7 integration tests: spec-only PR, clean working tree, dropped-paths/spec-PR chatops replies, and TriageFailed/Acted state transitions.)
