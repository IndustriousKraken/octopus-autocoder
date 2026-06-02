# orchestrator-cli — delta for a43-triage-flows-spec-only-pr

## MODIFIED Requirements

### Requirement: Completed triage splits into one or two PRs by content path
After the triage executor returns `Completed`, the daemon SHALL inspect the working tree's changed paths AND keep ONLY paths inside `openspec/changes/<derived-slug>/`. Paths outside that subtree (code fixes, doc edits, ANY non-spec content) SHALL be discarded via `git restore --source=HEAD --staged --worktree` (reverting both the index AND the worktree for tracked modifications, so a code edit the executor staged with `git add` cannot survive) AND `std::fs::remove_file` / `remove_dir_all` (for untracked additions) BEFORE the spec-PR commit. If any non-spec write cannot be reverted or removed, the daemon SHALL abort before the spec-PR commit rather than allow the write to leak into the spec PR. At most ONE PR is created per triage run — the spec PR. The fixes-PR path is removed entirely; code fixes flow through the standard implementer pipeline on a subsequent polling iteration after the operator merges the spec PR.

When the discard step drops non-empty paths (the agent wrote code despite the prompt's restriction), the daemon SHALL emit a WARN log naming the dropped paths AND post a chatops reply in the audit-thread naming the dropped paths AND directing the operator to capture the dropped fixes as `tasks.md` items in the spec if they were load-bearing.

When the discard step leaves NO spec content in `openspec/changes/<derived-slug>/` (the agent wrote only code AND no spec), NO PR is created AND the daemon posts a chatops reply in the audit-thread naming `no spec content produced; retry with a clearer directive`. The audit-thread's `status` flips to `TriageFailed`.

When the discard step leaves spec content, the daemon SHALL create the spec branch off the same base, commit the spec paths with subject `audit-triage spec proposal from <audit_type>`, push the branch, AND open the spec PR via the existing PR-creation helpers. PR-body text describes the spec content AND does NOT cross-link to any fixes PR (there is no fixes PR).

#### Scenario: Mixed diff produces one spec PR; code paths are discarded with chatops warning
- **GIVEN** the triage executor's Completed working tree contains BOTH new files in `openspec/changes/audit-fix-x/` AND modifications to `src/foo.rs`
- **WHEN** the audit-triage completion handler runs
- **THEN** `git restore --source=HEAD --staged --worktree -- src/foo.rs` (OR equivalent removal for untracked additions) executes BEFORE the commit, reverting both the index AND the worktree so a staged code edit cannot survive into the spec commit
- **AND** the working tree's `src/foo.rs` reverts to the base-branch state
- **AND** a WARN log fires naming the audit type, the derived slug, AND `src/foo.rs` as the dropped path
- **AND** the daemon creates a spec branch + PR with ONLY `openspec/changes/audit-fix-x/` paths
- **AND** the PR body does NOT mention a companion fixes PR
- **AND** the daemon posts a chatops reply in the audit-thread naming `src/foo.rs` as dropped AND explaining `Per a43, code fixes go through the standard implementer pipeline. The spec PR has been opened; if the dropped fixes were load-bearing, revise the spec to capture them as tasks.md items.`
- **AND** the audit-thread's `status` flips to `Acted`

#### Scenario: Spec-only triage produces one spec PR with no warning
- **GIVEN** the triage executor's Completed working tree contains ONLY new files in `openspec/changes/audit-fix-x/`
- **WHEN** the audit-triage completion handler runs
- **THEN** the discard step finds no paths to drop AND emits NO WARN log
- **AND** the spec branch + PR is created with the spec content
- **AND** NO chatops warning is posted (the agent followed the restriction)
- **AND** the audit-thread's `status` flips to `Acted`

#### Scenario: Code-only triage produces NO PR; chatops reply explains no spec content
- **GIVEN** the triage executor's Completed working tree contains ONLY modifications to `src/foo.rs` (no `openspec/changes/<derived-slug>/` content)
- **WHEN** the audit-triage completion handler runs
- **THEN** the discard step restores `src/foo.rs` to the base-branch state
- **AND** no spec branch is created AND no PR is opened
- **AND** the daemon posts a chatops reply in the audit-thread naming `no spec content produced; retry with a clearer directive`
- **AND** the audit-thread's `status` flips to `TriageFailed`

#### Scenario: Empty-diff triage posts a no-action reply
- **GIVEN** the triage executor returns `Completed` but the working tree's diff is empty (the LLM decided nothing was actionable)
- **WHEN** the audit-triage completion handler runs
- **THEN** no PRs are created
- **AND** the bot posts a reply in the audit thread containing the LLM's final-summary text explaining the decision
- **AND** the audit-thread's `status` flips to `Acted`

#### Scenario: Slug collision is suffixed
- **GIVEN** the derived slug `<audit-type>-<hash>` already exists as `openspec/changes/<slug>/`
- **WHEN** the audit-triage completion handler builds the spec dir
- **THEN** the daemon increments a suffix (`-2`, `-3`, ...) until it finds a free path
- **AND** the resulting spec directory uses the suffixed slug

### Requirement: Directive triage uses the existing two-PR mechanic; PRs participate in the revision-loop
When the executor returns `Completed` without a `.chat-reply.md` marker, the polling iteration SHALL discard non-spec writes from the working tree (via the same helper used by the audit-triage path) AND open AT MOST ONE PR — the spec PR — when spec content exists. Code-path writes are dropped before commit; a WARN log AND a chatops reply name the dropped paths when applicable. The two-PR shape from prior canonical text is removed; implementation flows through the standard implementer pipeline on a subsequent polling iteration after the operator merges the spec PR. Operators commenting `@<bot> revise <text>` on the spec PR continue to get revisions through `a01-pr-comment-revision-loop` per the unchanged revision-loop semantics.

#### Scenario: Mixed-diff directive produces one spec PR; code paths discarded with chatops warning
- **GIVEN** the directive's executor returns `Completed` with BOTH code changes in `src/foo.rs` AND new files in `openspec/changes/<chat-derived-slug>/`
- **WHEN** the chat-triage completion handler runs
- **THEN** the discard step restores `src/foo.rs`
- **AND** the daemon creates a spec branch + PR with ONLY the openspec paths
- **AND** the PR body does NOT mention a companion fixes PR
- **AND** the daemon posts a chatops reply in the proposal-thread naming `src/foo.rs` as dropped
- **AND** the proposal-request state's `status` flips to `Acted`

#### Scenario: Spec-only directive produces one spec PR
- **GIVEN** the directive's diff has only new `openspec/changes/<chat-derived-slug>/` paths
- **WHEN** the chat-triage completion handler runs
- **THEN** the spec PR is created
- **AND** no chatops warning is posted
- **AND** the proposal-request state's `status` flips to `Acted`

#### Scenario: Code-only directive produces NO PR
- **GIVEN** the directive's diff has only code paths (no new `openspec/changes/<chat-derived-slug>/`)
- **WHEN** the chat-triage completion handler runs
- **THEN** the discard step restores the code paths
- **AND** no PR is opened
- **AND** the daemon posts a chatops reply in the proposal-thread naming `no spec content produced; retry with a clearer directive`
- **AND** the proposal-request state's `status` flips to `TriageFailed`

#### Scenario: Empty-diff directive posts a no-action reply
- **GIVEN** the directive's executor returns `Completed` with an empty diff AND no `.chat-reply.md`
- **WHEN** the chat-triage completion handler runs
- **THEN** no PRs are created
- **AND** the bot posts a reply in the request's thread explaining no action was taken
- **AND** the proposal-request state's `status` flips to `Acted`

#### Scenario: Revision comments on a triage PR are processed normally
- **GIVEN** a chat-request-spawned PR has an operator comment `@<bot> revise <text>`
- **WHEN** the revision-loop dispatcher polls for new PR comments
- **THEN** the existing dispatcher (per `a01-pr-comment-revision-loop`) picks up the comment AND processes the revision against the PR's branch
- **AND** the proposal-request state file is not consulted (the revision is its own scope)
- **AND** the revision agent's writes remain scoped to the PR's diff (which by construction now contains only spec files)
