## Why

Triage flows — `send it` on an audit thread AND `propose` from chatops — currently run an LLM executor that writes BOTH spec changes (under `openspec/changes/<derived-slug>/`) AND code fixes (everywhere else) in one pass. The diff-split helper partitions the changed paths into `spec_paths` AND `fixes_paths`, AND the daemon opens two cross-linked PRs: `audit-triage fixes` AND `audit-triage spec` (OR the equivalent for `propose`). Both PRs land simultaneously; the operator reviewing the spec PR is reviewing code that's ALREADY been written AND pushed in a sibling PR.

Every OTHER spec-producing flow in autocoder follows a different pattern:

| Flow | Spec PR shape | Implementation timing |
| --- | --- | --- |
| Periodic `missing_tests_audit` / `security_bug_audit` | Spec-only PR (audit runs after queue walk; next iteration implements) | After operator merges spec PR |
| `brownfield` chatops verb | Spec-only PR (capability spec only) | N/A — pure spec drafting |
| `spec-it` chatops verb (inside scout thread) | Spec-only commit (next iteration's `list_pending` picks it up) | After operator merges |
| `send it` / `propose` (TODAY) | **TWO PRs simultaneously** — spec + fixes | **Concurrent — fixes ship without operator approval of the spec** |

The inconsistency means the operator's review power is unevenly distributed. For periodic spec-writing audits AND `brownfield` AND `spec-it`, the operator sees a spec proposal AND chooses whether to accept it before any code gets written. For `send it` AND `propose`, the operator sees a spec AND code at the same time, with code already committed AND pushed. Revising the spec requires also undoing the fixes; rejecting the spec means closing both PRs.

The fix is to collapse the two-PR shape down to one (spec-only) AND route implementation through the standard pipeline. The triage agent's role narrows from "draft a spec AND fix the code" to "draft a spec." After the operator merges the spec PR, the next polling iteration's `list_pending` picks up the new change AND the implementer writes the code fixes through the same path that every other spec change uses. Same revision-loop mechanics, same review surface, same audit trail.

This also simplifies a class of operator confusion: today the cross-linked fixes PR's body says "the spec PR carries the new spec; this PR carries the code fixes" — a structure that implies both are equally provisional, when actually the code fixes are NOT revisable without reverting the merged PR. The new shape eliminates that asymmetry.

## What Changes

**Triage executor runs (`send it`, `propose`) SHALL write spec content only.** The triage prompt-builder SHALL instruct the executor explicitly: the agent's writes are restricted to `openspec/changes/<new-slug>/` (proposal.md, tasks.md, specs/<capability>/spec.md). Any writes outside that subtree SHALL be dropped before commit — the diff-split helper keeps `spec_paths` AND discards `fixes_paths` rather than partitioning them into a second PR.

**The two-PR shape collapses to one spec-only PR.** The polling-loop's audit-triage completion handler AND the chat-triage completion handler each open ONE PR (the spec PR) when `spec_paths` is non-empty. The `fixes_paths` branch is removed entirely. PR-body text loses the cross-link clause (there's no other PR to link to). The PR's title becomes `audit-triage spec proposal: <slug>` (OR `chat-triage spec proposal: <slug>`) — no `fixes` variant exists.

**Discarded out-of-scope writes are logged AND surfaced.** When the diff-split helper finds `fixes_paths` non-empty (the agent wrote code despite the prompt's restriction), the daemon SHALL:

1. Log a WARN at the `audits` / `chat_triage` module level naming the dropped paths AND the audit/request context.
2. Post a chatops reply in the triage's lifecycle thread (the audit-thread for `send it`, the proposal-thread for `propose`) naming what was dropped, e.g., `⚠️ The triage agent attempted to write 3 code path(s) outside openspec/changes/. Per a43, code fixes go through the standard implementer pipeline. The spec PR has been opened normally; if the dropped fixes were load-bearing, revise the spec to capture them as `tasks.md` items.`
3. Reset the working tree's code-path changes via `git restore -- <fixes_paths>` BEFORE the spec-PR commit, so the spec PR's diff is genuinely spec-only.

**Implementation flows through the existing implementer pipeline.** After the operator merges the spec PR, the next polling iteration of the affected repo runs `list_pending` AND picks up `openspec/changes/<new-slug>/`. The implementer writes the code fixes against `tasks.md` per the standard contract; the PR opened is the standard implementer PR (subject to the standard code-reviewer flow, the standard revision-loop, etc.). No new pipeline. No special-casing.

**Existing revision-loop semantics on the spec PR.** Operators commenting `@<bot> revise <text>` on the spec PR continue to work per `a01-pr-comment-revision-loop`. The revision agent's prompt is unchanged — it operates on the spec PR's diff, which by construction now contains only spec files, so revisions stay scoped to spec content.

**Triage prompts updated.** `prompts/audit-triage.md` AND `prompts/chat-request-triage.md` gain a one-paragraph restriction near the prompt's "what you do" framing:

> Your writes are restricted to `openspec/changes/<new-slug>/`. Do NOT edit code outside that subtree. The implementer will pick up your spec on a subsequent iteration AND write the code through the standard pipeline. If the operator's request includes specific code-level changes, capture them as concrete `tasks.md` items so the implementer knows exactly what to do.

The restriction is hard — the daemon enforces it by discarding out-of-scope writes. The prompt is the soft contract; the diff-split + restore is the hard contract.

**No new control-socket actions.** The action surface is unchanged (`audit_triage_action`, `chat_triage_action` continue to exist, fire from the same chatops verbs, AND drive the same executor invocations). The behavior change is in what the executor's output gets committed.

**No config knob.** The behavior is unconditional. Operators who relied on the old two-PR shape can still get equivalent results — they merge the spec PR AND the implementer's PR follows on the next iteration. No opt-out.

## Impact

- **Affected specs:**
  - `orchestrator-cli` — MODIFIED two existing requirements:
    - `Completed triage splits into one or two PRs by content path` — re-titled in canonical to `Completed triage produces a spec-only PR; code-path writes are discarded`. The four existing scenarios (mixed-diff-two-PRs, code-only-fixes-only, spec-only-spec-only, empty-diff-no-action) become five new scenarios reflecting the new shape (mixed-diff drops fixes + opens spec PR + posts chatops warning; spec-only opens spec PR; code-only opens NO PR + posts chatops "no spec content produced, retry with clearer directive"; empty diff posts no-action reply; slug collision is suffixed — unchanged).
    - `Directive triage uses the existing two-PR mechanic; PRs participate in the revision-loop` — re-titled to `Directive triage produces a spec-only PR; PRs participate in the revision-loop`. Scenarios update to match.
  - The `Triage-created PRs participate in the existing PR-comment-revision-loop` requirement is preserved verbatim (still applies — PRs are still PRs).
- **Affected code:**
  - `autocoder/src/polling_loop.rs` — the audit-triage completion handler (`process_completed_audit_triage` at ~line 5994) AND the chat-triage completion handler (~line 6632) drop their `fixes_paths` branch. Both now: partition diff, log + chatops-warn the `fixes_paths` if non-empty, `git restore` the code paths, commit spec paths only, open the spec PR.
  - The `open_triage_pull_request` helper retains its signature but the two-call shape (one for fixes, one for spec) collapses to one call.
  - `prompts/audit-triage.md` AND `prompts/chat-request-triage.md` gain the restriction paragraph.
  - A new shared helper `discard_non_spec_writes(workspace, spec_slug) -> Result<Vec<String>>` that runs `git restore` against the non-openspec paths AND returns the list of restored paths for logging. Reusable between the two triage paths.
- **Operator-visible behavior:**
  - `send it` AND `propose` open at most one PR (the spec PR). If the triage agent writes only code AND no spec, the operator sees a chatops reply explaining no PR was opened AND why.
  - The fixes-PR title (`audit-triage fixes (<audit_type>)`) AND the cross-link prose stop appearing.
  - The lifecycle thread's summary reply contains one PR URL (the spec PR) instead of two, OR a chatops-warning naming dropped paths if the agent wrote code outside the spec subtree.
  - After the operator merges the spec PR, the next polling iteration runs the implementer on the new change — same as periodic audit-produced proposals AND brownfield AND spec-it.
- **Backward compatibility:** the chatops verbs (`send it`, `propose`), control-socket actions, AND operator-facing prompts (where to type, what to say) are unchanged. The change is in the daemon's response shape, NOT in how operators interact. Existing audit-thread state files AND proposal-request state files are unchanged on disk.
- **Dependencies:** none. `a42` (audit-logs-carry-repo-url) is independent AND can land in any order. The new chatops WARN in this change benefits from `a42`'s `url` field convention, so landing `a42` first gives the new warnings free repo attribution — but `a43` is not blocked on `a42`.
- **Acceptance:** `cargo test` passes; `openspec validate a43-triage-flows-spec-only-pr --strict` passes. Tests:
  - Audit-triage completion: a fixture where the triage executor writes BOTH `openspec/changes/audit-fix-x/proposal.md` AND `src/foo.rs` produces ONE PR (the spec PR), restores `src/foo.rs` BEFORE the commit, logs a WARN naming `src/foo.rs`, AND posts a chatops reply in the audit thread naming the dropped path.
  - Audit-triage completion: a fixture where the triage executor writes ONLY `openspec/changes/audit-fix-x/proposal.md` produces ONE PR (the spec PR) AND no chatops warning fires.
  - Audit-triage completion: a fixture where the triage executor writes ONLY `src/foo.rs` (no spec) produces NO PR AND a chatops reply naming "no spec content produced; retry with a clearer directive." `git restore` removes the unintended write.
  - Chat-triage completion: same three cases.
  - `discard_non_spec_writes` helper: with a workspace containing `openspec/changes/foo/proposal.md` AND `src/bar.rs` modified, calling the helper with spec_slug `foo` restores `src/bar.rs` AND returns `vec!["src/bar.rs"]`. The `openspec/changes/foo/` content is untouched.
  - Existing revision-loop test (`revision comment on a triage PR is processed normally`) continues to pass — the new PRs are still PRs from the dispatcher's perspective.
