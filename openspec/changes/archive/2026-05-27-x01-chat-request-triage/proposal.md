## Why

The `audit-reply-acts` spec gives operators a verb to act on an audit's findings: `send it` in the audit's thread → triage-mode executor → fixes PR + spec PR. The plumbing it establishes — the triage prompt that explores the codebase first, the diff-split into code-vs-spec PRs, the revision-loop interop for corrections — is general. The audit's findings are just one possible input to it.

The natural next step is to expose the same flow as a chat verb that accepts the operator's free-form input instead of audit findings. The operator types `@<bot> propose <repo> add a /healthz endpoint that returns 200 OK with daemon version and uptime`; autocoder runs triage against the request; produces a fixes PR (the endpoint implementation), a spec PR (the new requirement), or both. Same explore-first behavior, same revision plumbing, same operator-visible PR output. The chat surface becomes the entry point for any operator-initiated work that doesn't already fit a verb like `audit` or `rebuild-specs`.

One UX refinement makes the verb materially more usable: operators don't always know what they want. Sometimes the message is a question or an exploration prompt ("what would it take to add a healthz endpoint?", "should we extract the auth logic into a separate module?", "is this audit finding worth a spec?"). For those, jumping straight to code is exactly the wrong response. The triage prompt SHALL classify the input first: question/discussion → reply in the thread with the LLM's thoughts, no code changes; clear directive → proceed with the existing explore + classify + fix/spec flow. Truly ambiguous inputs fall through to the existing AskUser escalation, which posts a clarifying question in the thread and waits for the operator's reply.

This composes cleanly with everything in the pipeline. The plumbing reuse is the point: `audit-reply-acts` defines the triage-mode executor and the two-PR mechanic; the existing `chatops-slack-inbound-listener` defines the chatops listener; `a01-pr-comment-revision-loop` defines how operators iterate on resulting PRs; `chatops-audit-findings-in-threads` defines the threaded-reply convention. This spec wires those together with one new verb and one new prompt-template step (the question-vs-directive classifier).

## What Changes

**New chatops verb: `propose`.** Syntax: `@<bot> propose <repo-substring> <free-form text>`. The repo-substring uses the same case-insensitive substring-matching rule every other verb does. The free-form text is everything after the substring; no validation of content (the LLM handles interpretation).

Examples:
- `@<bot> propose myrepo add a /healthz endpoint that returns 200 OK with the daemon's version and uptime` → directive; triage proceeds to code + maybe a spec
- `@<bot> propose myrepo what would it take to extract the auth logic into a separate module?` → question; triage replies in thread, no PR
- `@<bot> propose myrepo something something handler logic` → ambiguous; triage emits AskUser, the standard chatops escalation fires, the operator clarifies, the executor resumes

**Bot ack creates a thread.** The bot's response to `@<bot> propose ...` is a top-level message in the channel:

```
✓ Queued proposal request for <repo_url>. The next polling iteration will run it (~Nm). Follow along in this thread.
```

The ack's `thread_ts` becomes the thread for the request's lifecycle. Subsequent status updates, the LLM's discussion reply (when the input is a question), and any AskUser escalations all post into this thread.

**Per-repo proposal-request queue.** Each `RepoTaskHandle` gains a `pending_proposal_requests: Arc<Mutex<Vec<ProposalRequest>>>` field. The dispatcher's `propose` verb appends to it; the polling iteration drains it at iteration start (alongside the existing revision-request queue and the on-demand audit queue from `chatops-on-demand-audit-trigger`).

State per request lives at `<state_dir>/proposal-requests/<repo-sanitized>/<request-id>.json`:

```json
{
  "request_id": "<uuid-v4>",
  "repo_url": "...",
  "channel": "C0OPS",
  "thread_ts": "1748399999.001234",
  "ack_message_ts": "1748399999.001234",
  "operator_user": "U0RAB",
  "request_text": "...",
  "submitted_at": "2026-05-27T14:00:00Z",
  "status": "Pending"
}
```

`status` transitions through `Pending → TriagePending → (Acted | Discussed | TriageFailed)`. The `Discussed` terminal state is new in this spec: it means the triage classified the input as a question and posted a thread reply, no PRs.

**Triage prompt extension: question-vs-directive classifier.** A new template `prompts/chat-request-triage.md` parallel to `prompts/audit-triage.md` but with one critical step added BEFORE the explore-codebase step:

```
Step 0: Classify the operator's request.

Read the operator's text. Decide:

- DIRECTIVE: a specific action the operator wants taken. Examples: "add X", 
  "fix Y", "refactor Z to do W". The directive is clear enough that a 
  reasonable engineer would know what to build. Proceed to step 1 (explore + 
  classify + fix/spec).

- QUESTION: the operator is asking for your opinion, an analysis, or an 
  exploration of options. Examples: "what would it take to do X?", 
  "should we Y?", "is Z worth doing?". DO NOT modify any source files. 
  Write your response to <workspace>/.chat-reply.md and finish. The daemon 
  will post your response in the request's thread.

- AMBIGUOUS: the request might be a directive but you cannot pin down what 
  exactly to build. Use the ask_user MCP tool to ask the operator for 
  clarification. The daemon will escalate to the chatops thread and resume 
  you with the answer.
```

The rest of the template (explore-codebase, classify-findings, apply-fixes, create-spec-proposal) is unchanged from `audit-triage.md`.

**`.chat-reply.md` marker file.** When triage classifies the input as a question, the LLM writes its response to `<workspace>/.chat-reply.md`. The polling iteration after the executor returns checks for this file BEFORE running the diff-split:

- File present AND non-empty: read its contents, post to the request's thread, delete the file, set state status to `Discussed`. No PRs are created. If `git status --porcelain` shows any OTHER modifications, log WARN naming them; revert via `git reset --hard HEAD` so the workspace is clean for the next iteration.
- File absent: proceed with the existing diff-split + two-PR creation from `audit-reply-acts`.

The post-reply truncates the file's contents at 35,000 characters (Slack's threaded-reply length budget; matches the `chatops-audit-findings-in-threads` cap) with a pointer to the daemon log when over.

**`a01-audit-reply-acts` is the dependency.** The triage executor (`run_triage`), the diff-split helper (`split_diff_for_audit_triage` — generalize the name to `split_diff_by_spec_dir` so it serves both audit-reply and chat-request flows), the two-PR creation mechanic, and the slug-collision-suffixing all come from that spec. This spec adds:

- The `propose` verb + parser entry
- The `pending_proposal_requests` queue + dispatcher integration
- The proposal-request state file + transitions
- The `.chat-reply.md` marker + thread-reply path
- The new `Discussed` terminal status
- The chat-request-triage prompt template with the question/directive classifier

**Revision-loop interop is unchanged.** PRs spawned from `propose` are structurally identical to PRs spawned from `send it` and from polling-loop changes. Operators commenting `@<bot> revise <text>` on either the fixes PR or the spec PR get revisions through `a01-pr-comment-revision-loop` the same way they would for any other autocoder-opened PR.

## Impact

- **Affected specs:** `orchestrator-cli` — one ADDED requirement covering the new verb, the proposal-request queue + state, the question/directive classifier in the triage prompt, the `.chat-reply.md` marker path, the `Discussed` terminal status, and the dependency on `audit-reply-acts`'s shared triage plumbing.
- **Affected code:**
  - `autocoder/src/chatops/operator_commands.rs` — extend the parser with `OperatorCommand::ProposeRequest { repo_substring, request_text }`. Extend the dispatcher to resolve the repo, generate a request_id, write the initial state file, post the bot ack (capture its `thread_ts`), submit a `queue_proposal_request` control-socket action.
  - `autocoder/src/control_socket.rs` — new `queue_proposal_request` action; extend `RepoTaskHandle` with `pending_proposal_requests`.
  - `autocoder/src/polling_loop.rs` — at iteration start, drain `pending_proposal_requests` alongside the existing audit and revision queues. For each request, build a `TriageContext` from the request_text + repo_url + canonical_specs_index, invoke `executor.run_triage`. Handle the outcome: check for `.chat-reply.md` first; if present, post to thread + clean up + Discussed status; if absent, run the existing diff-split + two-PR creation from `a01-audit-reply-acts`.
  - `autocoder/src/audits/threads.rs` (or wherever `a01-audit-reply-acts` places its triage helpers) — generalize the diff-split helper to take an "expected spec subdir" parameter so the same code serves both audit-reply (`openspec/changes/<audit-derived-slug>/`) and chat-request (`openspec/changes/<chat-derived-slug>/`) flows.
  - `autocoder/src/executor/claude_cli.rs` — extend the triage-mode template loader to support the new `prompts/chat-request-triage.md` template path; the substitution variables are mostly the same as audit-triage (request_text replaces findings; otherwise identical).
  - New file `prompts/chat-request-triage.md` with the four-step instruction (classify → explore → fix → spec).
  - `autocoder/src/audits/threads.rs` (or proposal-requests sibling module) — proposal-request state file IO + status transitions + stale pruning (same 7-day rule as audit-thread state from `a01-audit-reply-acts`).
  - Tests:
    - Parser: `@<bot> propose myrepo add a healthz` parses; `@<bot> propose myrepo` (no text) is invalid; `@<bot> propose` (no args) is invalid.
    - Dispatcher: happy path creates the state file, posts ack, submits action.
    - Triage classification (stub executor returning a `.chat-reply.md`-shaped output): bot posts the reply to the thread, no PR is created, status becomes Discussed.
    - Triage classification (stub executor returning a code diff): two-PR split fires; status becomes Acted.
    - Triage classification (stub executor returning AskUser): existing chatops escalation fires; status stays TriagePending.
    - `.chat-reply.md` with extra fs mods: WARN logged, mods reverted, only the reply is posted.
    - Length cap on `.chat-reply.md`: contents over 35,000 chars are truncated with the documented pointer.
    - State-file pruning: 8-day-old requests are removed by the prune; 5-day-old preserved.

- **Operator-visible behavior:** new chatops verb for general-purpose chat-driven code/spec work. The bot can either answer your question or ship code — the LLM decides based on whether your text is a directive or a question. PRs that result from directives go through the standard revision-loop for corrections.
- **Breaking:** no. Pure addition. Operators who don't use `propose` see no change.
- **Acceptance:** `cargo test` passes (new + existing). An operator types `@<bot> propose myrepo add a /healthz endpoint`; the bot acks within seconds and creates a thread; the next polling iteration runs triage; the executor classifies as a directive, explores, implements; two PRs land (a fixes PR with the endpoint, a spec PR with the new requirement). Separately: the operator types `@<bot> propose myrepo what would it take to refactor X?`; the bot acks; the next iteration runs triage; the executor classifies as a question, writes `.chat-reply.md`; the bot posts the LLM's response in the thread; no PR is created.
