## 1. Parser: `propose` verb

- [x] 1.1 Extend `OperatorCommand` in `autocoder/src/chatops/operator_commands.rs` with `ProposeRequest { repo_substring: String, request_text: String }`. Parser recognises `@<bot> propose <repo-substring> <free-form text>` with case-insensitive verb matching. The repo-substring is the first whitespace-separated token after `propose`; the request_text is everything after that with leading/trailing whitespace trimmed.
- [x] 1.2 Argument validation: repo-substring must pass the existing repo-substring regex (`^[a-zA-Z0-9._/-]{1,128}$`). The request_text has no character restrictions but is capped at 10,000 characters total (operators typing more than that should put it in an issue or doc and reference it).
- [x] 1.3 Tests:
  - `@<bot> propose myrepo add a healthz endpoint` parses as `ProposeRequest { repo_substring: "myrepo", request_text: "add a healthz endpoint" }`.
  - `@<bot> PROPOSE myrepo add X` parses identically (case-insensitive verb).
  - `@<bot> propose myrepo` returns "missing request text" error.
  - `@<bot> propose` returns "missing repo-substring" error.
  - Multi-line request text: preserved verbatim (line breaks kept).
  - Request text over 10,000 chars: rejected with a clear error.

## 2. Per-repo proposal-request queue + state

- [x] 2.1 Extend `RepoTaskHandle` in `autocoder/src/control_socket.rs` with `pub pending_proposal_requests: Arc<Mutex<Vec<ProposalRequest>>>` (default empty Vec). Define:
  ```rust
  pub struct ProposalRequest {
      pub request_id: String,
      pub channel: String,
      pub thread_ts: String,         // bot's ack-message ts; the request's lifecycle thread
      pub operator_user: String,
      pub request_text: String,
      pub submitted_at: DateTime<Utc>,
  }
  ```
- [x] 2.2 Create `autocoder/src/proposal_requests.rs` (or extend the audit-threads module from `a01-audit-reply-acts`). Public surface mirrors that module:
  ```rust
  pub struct ProposalRequestState {
      pub request_id: String,
      pub repo_url: String,
      pub channel: String,
      pub thread_ts: String,
      pub ack_message_ts: String,
      pub operator_user: String,
      pub request_text: String,
      pub submitted_at: DateTime<Utc>,
      pub status: ProposalRequestStatus,
      pub reason: Option<String>,
  }
  pub enum ProposalRequestStatus { Pending, TriagePending, Acted, Discussed, TriageFailed }
  pub fn state_path(state_dir: &Path, request_id: &str) -> PathBuf;
  pub fn write_state / read_state / remove_state / prune_stale_entries;
  ```
- [x] 2.3 Tests:
  - State round-trip: write → read returns identical content.
  - Status transitions preserve other fields.
  - Stale-pruning at 7 days same as audit-thread state.

## 3. Dispatcher integration

- [x] 3.1 In `OperatorCommandDispatcher::handle_message`, add a match arm for `ProposeRequest`:
  1. Resolve the repo via `match_repo`.
  2. Generate `request_id` = `uuid::Uuid::new_v4().to_string()`.
  3. Post the ack message via `post_notification`:
     ```
     ✓ Queued proposal request for <repo_url>. The next polling iteration will run it (~Nm). Follow along in this thread.
     ```
     Capture the ack message's `ts` (the new threading-aware backend method must return it per `chatops-audit-findings-in-threads`'s extension).
  4. Write the proposal-request state file with `status: Pending` AND `thread_ts: <ack_ts>`.
  5. Submit a `queue_proposal_request` control-socket action with the request_id.
- [x] 3.2 New `queue_proposal_request` control-socket action: looks up the repo's handle by URL, appends a `ProposalRequest` (assembled from the state file) to `pending_proposal_requests`. Returns Ok or an error if the repo can't be found.
- [x] 3.3 Tests:
  - Happy path: dispatcher submits the action; state file is written; queue is populated.
  - Repo substring resolves to Multiple/None: existing "be more specific" / "no repo matched" responses fire; no state file written.

## 4. Triage prompt template

- [x] 4.1 Create `prompts/chat-request-triage.md`. Required substitutions:
  - `{{request_text}}` — the operator's free-form input
  - `{{repo_url}}` — for context
  - `{{canonical_specs_index}}` — a brief listing of `openspec/specs/` so the LLM can read selectively
- [x] 4.2 Template content (the four-step instruction):
  1. **Classify the request.** DIRECTIVE / QUESTION / AMBIGUOUS. For QUESTION: write your response to `<workspace>/.chat-reply.md` and finish (no source changes). For AMBIGUOUS: use the `ask_user` MCP tool to ask for clarification. For DIRECTIVE: proceed to step 2.
  2. **Explore the codebase.** Read the README, top-level source files, the directory structure. Use `openspec` to read relevant canonical specs.
  3. **Triage findings.** For each thing the directive asks for: decide quick fix (small, localized, no contract change) vs spec-worthy (behavior change, new boundary, cross-cutting).
  4. **Apply** the quick fixes directly to the working tree. **Create** new `openspec/changes/<derived-slug>/` for spec-worthy items with proposal.md, tasks.md, and spec deltas.
  5. **Report back** with a final summary naming what was done.
- [x] 4.3 The `<derived-slug>` is `chat-request-<short-hash-of-request-text>` to avoid collisions across multiple `propose` calls.
- [x] 4.4 Tests:
  - Substitution: rendered prompt contains all three substitution payloads.
  - Template integrity: the four-step instruction is present in the rendered prompt.

## 5. Triage executor invocation

- [x] 5.1 Reuse the `TriageContext` from `a01-audit-reply-acts` but extend (or shadow) with a `ChatTriageContext`:
  ```rust
  pub struct ChatTriageContext {
      pub request_text: String,
      pub repo_url: String,
      pub canonical_specs_index: String,
  }
  ```
  OR — cleaner — generalize `TriageContext` to take a `description: String` field that's the operator's request OR the audit's findings depending on the call site. The new spec uses the same struct with a different value.
- [x] 5.2 The executor's `run_triage` method (from `a01-audit-reply-acts`) accepts the generalized context. Same prompt-build path; the template selection is based on which mode the iteration is running.
- [x] 5.3 Tests: stub executor returning Completed (in each of the three flavors: discussion reply, code diff, AskUser) for a `ChatTriageContext` — the dispatcher's downstream handling is correctly routed based on output.

## 6. Polling-loop integration

- [x] 6.1 In `autocoder/src/polling_loop.rs::run`, at iteration start AFTER the revision-loop processing AND the on-demand audit processing AND BEFORE the pending-change walk, drain `pending_proposal_requests`. For each request:
  1. Load the `ProposalRequestState`.
  2. Build `ChatTriageContext`.
  3. Invoke `executor.run_triage(workspace, ctx)`.
  4. Handle the outcome per task 6.2.
- [x] 6.2 Outcome handling:
  - **`Completed` AND `<workspace>/.chat-reply.md` exists AND is non-empty**:
    - Read the file contents.
    - Truncate at 35,000 characters with the documented pointer suffix if over.
    - Post to the request's thread via `post_threaded_reply`.
    - Delete `.chat-reply.md`.
    - If `git status --porcelain` reports any OTHER modifications, log WARN naming them AND run `git reset --hard HEAD; git clean -fd` to revert (the LLM was supposed to modify nothing else; clean up its mess).
    - Set state status to `Discussed`.
  - **`Completed` AND no `.chat-reply.md`**:
    - Run the diff-split + two-PR creation from `a01-audit-reply-acts` (generalize the helper name to `split_diff_by_spec_dir`).
    - Set state status to `Acted`.
  - **`AskUser`**: existing chatops escalation fires; state stays `TriagePending`. The escalation posts in the request's thread (the same thread that holds the bot's ack).
  - **`Failed { reason }`**: post a failure reply in the thread; set state status to `TriageFailed`.
- [x] 6.3 Tests:
  - Stub executor returns Completed + .chat-reply.md present + other source files clean → reply posted, state Discussed, no PRs.
  - Stub executor returns Completed + .chat-reply.md present + extra source modifications → reply posted, state Discussed, no PRs, WARN logged, modifications reverted.
  - Stub executor returns Completed + no .chat-reply.md → diff-split + PRs, state Acted.
  - Stub executor returns AskUser → escalation fires, state stays TriagePending.
  - Stub executor returns Failed → failure reply in thread, state TriageFailed.

## 7. Length cap on .chat-reply.md

- [x] 7.1 When the reply contents exceed 35,000 characters, truncate to 35,000 chars AND append:
  ```
  
  … [truncated; full reply at journalctl -u autocoder | grep request_id=<request_id>]
  ```
- [x] 7.2 Tests:
  - 50,000-char reply → posted reply is ~35,000 chars with the truncation pointer.
  - 1,000-char reply → posted reply is exactly the file contents.

## 8. Stale-entry pruning

- [x] 8.1 At each iteration's start (or in the existing periodic-housekeeping pass), call `proposal_requests::prune_stale_entries(state_dir, Duration::from_days(7))`. Removes state files whose `submitted_at` is older than 7 days regardless of status.
- [x] 8.2 Tests:
  - 8-day-old entry → removed.
  - 5-day-old entry → preserved.

## 9. README + docs updates

- [x] 9.1 In `docs/CHATOPS.md`, add a section "Chat-driven proposals: `propose`" describing the verb, the question-vs-directive classification, the two output paths (thread reply for questions, PRs for directives), the 7-day staleness rule, and the revision-loop interop for resulting PRs.
- [x] 9.2 In `docs/OPERATIONS.md`, cross-reference the `propose` verb in the audit/triage workflow section. Operators understand the symmetry: `audit-reply-acts` is "act on what the audit found"; this is "act on what I'm asking for."
- [x] 9.3 In `docs/TROUBLESHOOTING.md`, add entries for the polite-refusal cases (untracked thread, stale request, status conflict).

## 10. Spec delta

- [x] 10.1 The ADDED requirement in `openspec/changes/x01-chat-request-triage/specs/orchestrator-cli/spec.md` codifies: the `propose` verb shape and argument rules, the proposal-request state file shape, the question-vs-directive classifier in the triage prompt, the `.chat-reply.md` marker path and length cap, the new `Discussed` terminal status, the revision-loop interop, and the 7-day pruning rule.

## 11. Verification

- [x] 11.1 `cargo test` passes (new + existing).
- [x] 11.2 `openspec validate x01-chat-request-triage --strict` passes.
- [x] 11.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
