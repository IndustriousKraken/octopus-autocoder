## Why

An operator looking at an audit's threaded chatops post today has three options: ignore it, manually copy-paste findings into another LLM session for triage, or open a ticket / spec change by hand. The pattern an operator wants for production-prep iteration is "I see the findings, I trust the audit, just do it." The pieces that make that pattern feasible are already in flight: threaded audit delivery puts the findings in a stable, addressable thread; the PR-comment revision loop handles operator-initiated revisions on any PRs autocoder opens; the audits know how to invoke the executor.

The missing piece is a single chatops verb that takes "operator saw the audit, decided to act" as input AND produces the action as output. The `send it` verb (in an audit's thread, mentioning the bot) closes that loop.

When the verb fires, autocoder spawns an executor run with the audit's findings AND a triage prompt that first instructs the LLM to explore the codebase to understand context, THEN classify each finding as either a "quick fix" (apply directly to the code) or "spec-worthy" (create a new OpenSpec change proposal). The executor produces both kinds of output in one run. autocoder splits the produced changes into TWO PRs: one carrying the code fixes (mergeable immediately), one carrying the new spec proposal(s) (operator reviews the spec before merging; on merge, the next polling iteration implements the spec normally).

The "explore the codebase first" instruction in the prompt is the lever that controls quality. The LLM with project context can distinguish "this is a one-line guard the caller already implies" from "this is a contract change that ripples through three modules." Without that context, the classification degrades to surface-level pattern matching on the audit text.

The revision-loop plumbing absorbs misjudgment risk: if the LLM over-promotes findings to specs OR under-fixes by missing something obvious, the operator replies to either PR with `@<bot> revise ...` and the existing revision loop produces a corrected version. The system doesn't need to be perfect on first attempt; it needs to be self-correcting via channels operators already use.

## What Changes

**New chatops verb in audit threads: `send it` (mentioning the bot).** The trigger pattern is `@<bot> send it` posted as a reply within an audit's notification thread. The chatops listener already routes threaded mentions to the dispatcher (per `chatops-slack-inbound-listener`); this change extends the dispatcher with a `SendItOnAudit { thread_ts }` command that's recognized ONLY when the message's `thread_ts` matches a tracked audit thread. Outside an audit thread, `@<bot> send it` returns `?` reaction (unknown-verb fallback, unchanged).

**Audit-thread state tracking.** When a threaded audit notification posts (per `chatops-audit-findings-in-threads`), the audit scheduler records the resulting thread metadata in a state file at `<state_dir>/audit-threads/<thread_ts>.json`:

```json
{
  "thread_ts": "1748293445.001234",
  "channel": "C0OPS",
  "repo_url": "git@github.com:acme/myrepo.git",
  "audit_type": "architecture_brightline",
  "findings_excerpt": "...up to 35,000 chars of the threaded body...",
  "posted_at": "2026-05-26T12:34:05Z",
  "status": "open"
}
```

When the chatops listener sees `@<bot> send it` in a thread, it looks up the audit-thread state by `thread_ts`. If `status == "open"` AND the entry is fresh (≤7 days old), the dispatch proceeds. Older entries are stale; the bot replies `✗ This audit's findings are too old to act on (>7d). Re-run the audit via @<bot> audit <type> <repo>.` Newer entries with `status != "open"` (already acted on, or otherwise unavailable) get a similar polite refusal.

**Executor invocation in a new mode: triage.** The dispatcher submits a `trigger_audit_action` control-socket action. The polling task picks this up at the start of its next iteration, before the normal queue walk. It invokes the executor in a new "triage" mode with a `TriageContext`:

```rust
pub struct TriageContext {
    pub findings: String,
    pub audit_type: String,
    pub repo_url: String,
}
```

The triage-mode prompt template (new file `prompts/audit-triage.md`) instructs the LLM to:
1. **Explore the codebase first.** Read README, top-level source files, the directory structure. Build a mental model of the project's conventions and boundaries. Use `openspec` to read the canonical specs for context on what the project is supposed to do.
2. **Triage each finding** against that context. For each, decide: quick fix (the code change is small, localized, doesn't change the project's intended contract) OR spec-worthy (the finding implies a behavior change, a new boundary, or a cross-cutting refactor).
3. **Apply the quick fixes** to the working tree directly.
4. **Generate the spec change(s)** for the spec-worthy findings under `openspec/changes/<new-slug>/` with `proposal.md`, `tasks.md`, and the appropriate spec delta files. The slug derives from the audit type AND a short hash of the findings to avoid collisions across multiple `send it` runs.
5. **Report back** in a final summary that names what was fixed, what was specced, and what (if anything) was declined.

**Two-PR split.** After the triage-mode executor returns Completed, autocoder examines the produced diff:
- Files inside `openspec/changes/<new-slug>/` → spec PR.
- All other files → fixes PR.

Two `git checkout -B` cycles produce two branches off the same base. Each gets its own commit + push + PR via the existing PR-creation helpers. PR bodies cross-link each other ("This PR carries the code fixes from audit <type>; see <spec PR> for the new spec change.").

When the executor's diff has only fixes (no `openspec/changes/<new-slug>/`), only the fixes PR is opened. When it has only a spec (rare; the LLM decided everything needed specs), only the spec PR is opened. When neither (LLM decided nothing was actionable), no PR is opened — the bot replies in the audit thread with the LLM's stated reasoning.

**Audit-thread state transitions.** When `send it` is accepted and a triage run kicks off, the audit-thread state's `status` flips to `triage-pending`. When the triage run completes successfully (one or both PRs opened, or the no-action reply posted), the state flips to `acted`. If the triage run fails (executor returned Failed, network errors, etc.), the state flips to `triage-failed` with a `reason` field; the operator can retry by posting `@<bot> send it` again, which sees the failed state and starts a fresh triage attempt.

**Revision-loop coordination.** Both PRs are normal autocoder-opened PRs. They participate in the existing `a01-pr-comment-revision-loop` flow: operators replying `@<bot> revise <text>` on either PR get revisions through the standard channel. The audit-thread-spawned PRs aren't structurally different from polling-loop-spawned PRs in any way that matters to the revision dispatcher.

**`send it` is the only verb in this initial scope.** Variants like `send it but ignore finding 3` or `send it as fixes only, no specs` are explicit non-goals for v1. The all-or-nothing UX trades surgical control for simplicity; operators wanting per-finding selection iterate via revision comments on the resulting PRs.

## Impact

- **Affected specs:** `orchestrator-cli` — one ADDED requirement covering the verb, the thread-state tracking, the triage-mode executor invocation, the two-PR split, and the state-transition rules. No additions to `chatops-manager` are needed beyond what `chatops-audit-findings-in-threads` already provides.
- **Affected code:**
  - `autocoder/src/chatops/operator_commands.rs` — extend the parser with `OperatorCommand::SendItOnAudit { thread_ts }`. Recognition requires the message be in a thread AND match the bot mention + `send it` verb (case-insensitive).
  - `autocoder/src/audits/mod.rs` (or a new `autocoder/src/audits/threads.rs`) — add the audit-thread state-file IO (read/write/expire helpers).
  - `autocoder/src/audits/scheduler.rs` — when an audit's notification posts via the threaded path (from `chatops-audit-findings-in-threads`), the scheduler captures the resulting `thread_ts` (the new chatops backend method returns it; or the helper exposes it) AND writes the audit-thread state file with `status: open`.
  - `autocoder/src/control_socket.rs` — new `trigger_audit_action` action: enqueues a per-repo triage request keyed by `thread_ts`; the polling task drains it at iteration start.
  - `autocoder/src/polling_loop.rs` — at the start of each iteration, BEFORE the normal queue walk AND alongside the revision-request processing, check for triage-pending requests and execute them.
  - `autocoder/src/executor/claude_cli.rs` — extend the prompt-build path with a `TriageContext` variant. New template at `prompts/audit-triage.md`. Substitution variables include `{{findings}}`, `{{audit_type}}`, `{{repo_url}}`, and `{{canonical_specs_index}}` (a brief listing of what specs exist in `openspec/specs/` so the LLM can read selectively).
  - `autocoder/src/polling_loop.rs` — after the triage-mode executor returns Completed, the diff-split + two-PR logic. Add `pub fn split_diff_for_audit_triage(workspace, new_slug) -> (fixes_paths, spec_paths)` returning two sets of paths based on whether each touches `openspec/changes/<new_slug>/`.
  - `autocoder/src/github.rs` — no new helpers needed; the existing `create_pull_request` handles both PRs. PR bodies are templated by the audit-reply path.
  - New template file `prompts/audit-triage.md` with the four-step instruction.
  - Tests:
    - Parser: `@<bot> send it` in a non-empty thread parses; outside a thread, parses as unknown verb (`?` reaction path).
    - Audit-thread state: write + read round-trips; expire-on-stale; status transitions.
    - Triage dispatcher: with a stub executor returning a Completed with both kinds of content, the diff-split produces two non-empty path sets.
    - Triage dispatcher: with only code changes (no new openspec/changes dir), only fixes PR is created.
    - Triage dispatcher: with only spec changes (no code touches), only spec PR is created.
    - Triage dispatcher: with neither (empty diff), no PR is created AND the bot replies in the audit thread with the LLM's stated reasoning.
    - Cross-link in PR bodies: when both PRs are created, each body contains a link to the other.
    - Stale audit thread: `send it` against a 14-day-old audit thread is refused with the polite-too-old reply.
    - Revision-loop interop: both PRs are recognized by the existing PR-comment-revision-loop dispatcher (the same way polling-loop-spawned PRs are).

- **Operator-visible behavior:** new verb `@<bot> send it` in audit threads. One verb produces one or two PRs depending on what the LLM triage decides. PRs are normal autocoder-opened PRs that compose with the existing revision-loop. Audit threads gain state — operators see the bot's "triage pending → acted" progression in the thread's reply chain.
- **Breaking:** no. The new verb is additive. Audit threads that don't get `send it` replies behave exactly as today (per `chatops-audit-findings-in-threads`).
- **Acceptance:** `cargo test` passes (new + existing). An operator posts `@<bot> send it` as a reply in a tracked audit thread; the bot acks the trigger within seconds; within one polling iteration, two PRs appear (fixes + spec, when both are warranted); each PR body cross-links the other; the spec PR's `openspec/changes/<new-slug>/` is well-formed and validates via `openspec validate --strict`.
