## Why

The `@<bot> revise <text>` flow on PR comments has been semantically broken since the day it was specced — not as a regression, as a construction defect. The revision prompt builder calls `openspec instructions apply --change <X>` against the workspace's current state to load "the original change material." But the workspace's current state, when the revise dispatcher runs at the top of every polling iteration, is the AGENT BRANCH'S tip — which always contains `openspec archive`'s move of `<X>` from `openspec/changes/<X>/` to `openspec/changes/archive/<date>-<X>/`. The implementer prompt enforces this: *"Do not archive the change yourself; `openspec archive` is denied in this sandbox. Leave the working tree dirty — autocoder will commit your diff and archive on success."* The archive-before-PR rule is **the** rule, not an edge case.

So `openspec instructions apply --change <X>` ALWAYS fails for ANY change that has ever been in a PR. The code then "falls back to a placeholder" — substituting a stub string in the prompt where the original change body would go:

```rust
"_(original change material unavailable — `openspec instructions apply` failed; rely on the PR diff and the revision request below.)_"
```

100% of revise attempts hit the placeholder path. Every operator who has ever run `@<bot> revise` on an autocoder-opened PR has received a degraded prompt without that being signalled anywhere in the chat or PR surface (only a WARN in journalctl that operators don't tail). The reply they see (`✅ Revision applied: ...`) is indistinguishable from a fully-loaded revision, so quality drops are silent.

The "best-effort" code comment that justifies this (`Revision mode is best-effort about loading the original change body...`) is a violation of the project's `no_stubs_in_changes` rule: every change ships fully runnable; no degraded-input path is permitted unless the spec explicitly defers AND no production path reaches the stub. This stub IS the production path.

The architectural mistake is reading change material from a local filesystem path that, by construction, cannot contain it. The PR itself is the canonical record of every piece of context the revision needs:

- **Spec deltas**: contained in the PR's diff as the `archive/<date>-<X>/proposal.md`, `tasks.md`, `specs/<cap>/spec.md` files.
- **Agent's original work narrative**: the `## Agent implementation notes` issue-comment that the canonical "Implementer-summary PR comment" requirement mandates be posted after every PR opens. One per change in multi-change passes.
- **Code review concerns**: contained in the PR body's `## Code Review` section (per the canonical "Monolithic PR at end of pass" requirement) when the reviewer is enabled.
- **Operator's revision text**: the triggering comment's text (already extracted).
- **Multi-change resolution**: the PR body's "Changes implemented in this pass" list names every change. The LLM picks which one(s) the operator's revision request targets.

The PR is the source of truth. It survives log retention (logs are pruned after 30 days of archive-state per `a20a2`'s pair-retention; PR comments persist forever). It's identical to what the human reviewer sees, which gives the LLM and operator a shared frame of reference.

## What Changes

**Remove the placeholder fallback in `build_revision_prompt`.** The `_(original change material unavailable — ...)_` string SHALL be removed from the codebase entirely. The `openspec instructions apply` call SHALL also be removed — it was the wrong source for revision-mode regardless of the placeholder issue.

**Replace it with PR-sourced material assembly.** The revision prompt SHALL be built from five PR-derived inputs:

1. **PR diff** — already gathered as `revision_context.pr_diff`. Contains the spec deltas via the archive moves. No change.
2. **PR body** — fetched once via `GET /repos/{owner}/{repo}/pulls/{n}`. Passed to the LLM in full (no parsing for sub-sections). Contains the code review section AND the changes-list section AND any other context.
3. **Per-change `Agent implementation notes` comments** — fetched via `list_issue_comments_since` (existing helper, with `since=null` to get all comments) filtered to comments whose body starts with the canonical marker `## Agent implementation notes`. Each captured comment's body becomes a section in the prompt. For multi-change passes, multiple notes are concatenated in PR-body order.
4. **Operator's revision text** — already gathered. No change.
5. **List of changes in the PR** — already extracted via `extract_change_list_from_pr_body`. Passed explicitly so the LLM can name which change(s) it's revising.

The dispatcher SHALL fetch (2) and (3) once per revise invocation. (2) and (3) are read-only; failures fetching them result in a hard refusal (`✗ Cannot revise: failed to fetch PR context: <error>. Daemon will retry on the next iteration.`) AND no advance of the comment-seen marker, so transient API errors don't lose the comment. Persistent failures over multiple iterations remain visible to the operator via repeated comments.

**Revision prompt template (`prompts/implementer-revision.md`) gains new placeholders.** Existing: `{{change_body}}` (placeholder format — removed), `{{revision_diff}}`, `{{revision_request}}`. New format:

- `{{pr_body}}` — the PR's body as-is.
- `{{pr_change_list}}` — newline-separated change slugs from the PR.
- `{{agent_implementation_notes}}` — concatenated `## Agent implementation notes` comment bodies, separated by `---` between entries.
- `{{revision_diff}}` — unchanged.
- `{{revision_request}}` — unchanged.

The template's prose SHALL instruct the LLM to identify which change(s) the operator's revision targets — by name match, by context cue, or by applying to all listed changes if the request is generic. This decision is in scope for the LLM; the daemon does not pre-filter.

**The dispatcher SHALL NOT call the executor when context assembly fails.** Any failure path (PR body fetch error, comments fetch error, etc.) SHALL produce a clear PR comment naming the failure AND skip the executor call. No degraded-prompt path exists.

**Canonical invariant: prompt construction SHALL receive complete material; no degraded-prompt path is permitted.** A new requirement in `executor` codifies: prompt-construction code SHALL receive every input the canonical template requires, OR the caller SHALL refuse to invoke the executor for that work item. No prompt builder SHALL silently substitute placeholder text for a missing input. The construction-site discipline mandates that every call to `build_X_prompt` is gated by an explicit availability check at the call site; missing-input cases are handled by the caller, not by the builder. Future prompt builders (the chat-triage builder, brownfield draft builder, scout builder, sentinel handlers, etc.) inherit the invariant.

## Impact

- **Affected specs:**
  - `executor` — ADDED requirement: `Revision prompt is constructed from PR-sourced material; no degraded-prompt fallback is permitted`. Covers the template placeholders, the assembly steps, AND the no-fallback invariant.
  - `orchestrator-cli` — ADDED requirement: `Revise dispatcher refuses to invoke the executor when PR-context assembly fails`. Covers the polite-refusal path AND the comment-seen-marker non-advance on transient API errors.
- **Affected code:**
  - `autocoder/src/executor/claude_cli.rs::build_revision_prompt` — replaces the `openspec instructions apply` call AND the placeholder fallback with a PR-context-assembly invocation. Template substitution adopts the new placeholder set.
  - `prompts/implementer-revision.md` — updated to reference the new placeholders (`{{pr_body}}`, `{{pr_change_list}}`, `{{agent_implementation_notes}}`) AND instruct the LLM on the multi-change targeting decision.
  - `autocoder/src/revisions.rs::process_one_pr` — assembles `RevisionContext` with the new fields (PR body, agent notes block, change list). Adds the failure-refusal path for context-assembly failures.
  - `autocoder/src/github.rs` — may need a `get_pr_body` helper (or reuse existing PR-fetch path) AND ensure `list_issue_comments_since` supports `since=null` for "all comments."
  - `autocoder/src/revisions.rs::RevisionContext` — adds `pr_body`, `pr_change_list`, `agent_implementation_notes` fields.
- **Operator-visible behavior:**
  - Revise on autocoder-opened PRs works for the first time. The agent receives spec deltas (via diff), the original implementation notes, the code review section, AND the revision request — full context.
  - When PR-context assembly fails (transient or permanent), the daemon posts a clear `✗ Cannot revise: <reason>` comment. The comment-seen marker is NOT advanced on transient failures (so the next iteration retries); it IS advanced on terminal failures with operator-actionable causes.
  - The `_(original change material unavailable...)_` placeholder string disappears from every revision context for every PR. Operators inspecting revision-mode logs see complete prompt input.
  - No new config knobs.
- **Breaking:** no for operators. Internal: `build_revision_prompt` signature evolves; the revision template's placeholders evolve. Operators with custom revision templates need to migrate their placeholder names (documented in `docs/CONFIG.md`'s prompt-overrides table per `a24`).
- **Acceptance:** `cargo test` passes (new tests + existing tests after template migration); `openspec validate a20a5-revision-prompt-built-from-pr --strict` passes; `cargo clippy --bin autocoder` produces no new warnings in touched files. Manual verification: on an open PR (any autocoder-opened PR, fork-PR mode or not, post-`a20a4`), comment `@<bot> revise add a small clarifying sentence to the proposal`. Within one polling cycle, the agent's reply contains a concrete edit that references the spec material AND the operator's text. Verify the per-change run log for the revision pass shows a populated `{{agent_implementation_notes}}` section in the rendered prompt (not the pre-spec placeholder string).
