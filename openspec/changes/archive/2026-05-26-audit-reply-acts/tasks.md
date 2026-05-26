## 1. Parser: recognise `send it` in audit threads

- [x] 1.1 Extend `OperatorCommand` in `autocoder/src/chatops/operator_commands.rs` with `SendItOnAudit { thread_ts: String }`. Parser recognises `@<bot> send it` (case-insensitive on `send it`, single space between tokens) AS such only when the inbound message arrives with a non-empty `thread_ts`. Messages outside a thread that match the same text pattern parse as unknown verb (existing `?`-reaction fallback).
- [x] 1.2 The `thread_ts` is carried through from the inbound message envelope; the parser doesn't see it directly, but the chatops listener stamps it onto the parsed command before dispatch.
- [x] 1.3 Tests:
  - `@<bot> send it` in a thread parses as `SendItOnAudit { thread_ts }`.
  - `@<bot> SEND IT` in a thread parses identically (case-insensitive).
  - `@<bot> send it` outside any thread parses as the unknown-verb fallback.
  - `@<bot> send` (no `it`) parses as unknown verb.
  - `@<bot> send it now` (extra args) parses as unknown verb (`send it` must be the entire verb, no trailing args).

## 2. Audit-thread state IO

- [x] 2.1 Create `autocoder/src/audits/threads.rs`. Public surface:
  ```rust
  pub struct AuditThreadState {
      pub thread_ts: String,
      pub channel: String,
      pub repo_url: String,
      pub audit_type: String,
      pub findings_excerpt: String,
      pub posted_at: DateTime<Utc>,
      pub status: AuditThreadStatus,  // open | triage-pending | acted | triage-failed
      pub reason: Option<String>,     // populated for triage-failed
  }
  pub enum AuditThreadStatus { Open, TriagePending, Acted, TriageFailed }
  pub fn state_path(state_dir: &Path, thread_ts: &str) -> PathBuf;
  pub fn write_state(state_dir: &Path, state: &AuditThreadState) -> Result<()>;
  pub fn read_state(state_dir: &Path, thread_ts: &str) -> Result<Option<AuditThreadState>>;
  pub fn prune_stale_entries(state_dir: &Path, max_age: Duration) -> Result<usize>;
  ```
  Path convention: `<state_dir>/audit-threads/<thread_ts>.json`. Atomic writes via temp-file-then-rename.
- [x] 2.2 The findings excerpt is capped at 35,000 chars (same as the threaded-notification body cap from `chatops-audit-findings-in-threads`). Stored verbatim so the triage prompt can ship the full content the operator saw.
- [x] 2.3 Tests:
  - Read missing state file returns `Ok(None)`.
  - Write then read round-trips every field including `status` and `reason`.
  - Status transitions preserve other fields (e.g. setting `status: TriagePending` keeps `findings_excerpt`).
  - Stale-pruning removes entries older than the configured max age; younger entries stay.

## 3. Stamp audit-thread state on threaded notification post

- [x] 3.1 In `autocoder/src/audits/scheduler.rs`, when an audit's findings post via the threaded path (per `chatops-audit-findings-in-threads`), capture the returned `thread_ts` from the chatops backend's threaded-post call. Today's `post_notification_with_thread` returns `Result<()>` — extend it to return `Result<Option<String>>` where `Some(thread_ts)` names the top-level message's id. For backends that don't support threading (default impl), return `Ok(None)` (no thread; nothing to track).
- [x] 3.2 When the backend returns `Some(thread_ts)`, the scheduler writes an `AuditThreadState` with `status: Open` to the audit-threads directory.
- [x] 3.3 Tests:
  - Slack threaded post returns `Some(thread_ts)`; scheduler writes the state file.
  - Default-impl (non-threading) backend returns `Ok(None)`; scheduler writes no state file.

## 4. Dispatcher: send-it routes to triage

- [x] 4.1 In `OperatorCommandDispatcher::handle_message`, add a match arm for `SendItOnAudit { thread_ts }`:
  1. Read the `AuditThreadState` for `thread_ts`. If `Ok(None)` or read-fails: reply `✗ This reply is in a thread autocoder is not tracking. The `send it` verb only acts in audit-notification threads.`
  2. If `state.posted_at` is older than 7 days: reply `✗ This audit's findings are too old to act on (>7d). Re-run the audit via @<bot> audit <type> <repo>.`
  3. If `status != Open AND status != TriageFailed`: reply `✗ This audit thread is already <status>. No new action taken.`
  4. Otherwise: submit `trigger_audit_action` via the control socket with the `thread_ts`; the action returns the resolved repo URL + audit type. Reply with `✓ Triage scheduled for <audit_type> on <repo_url>. The next polling iteration will run it (~Nm).`
  5. Update the state file's `status` to `TriagePending`.
- [x] 4.2 Tests:
  - Reply in untracked thread → polite-refusal reply.
  - Reply in stale thread (>7 days) → polite-too-old reply.
  - Reply in tracked-and-fresh thread → triage scheduled + state status flipped to TriagePending.
  - Reply in TriageFailed state → triage re-scheduled (fresh attempt).

## 5. Triage-mode executor invocation

- [x] 5.1 Create new template `prompts/audit-triage.md`. Required substitutions: `{{findings}}`, `{{audit_type}}`, `{{repo_url}}`, `{{canonical_specs_index}}` (a brief listing of which specs exist in `openspec/specs/`). Template instructions:
  - "First, explore the codebase. Read README, top-level source, the directory structure. Use openspec to read canonical specs you'll need."
  - "Then, classify each finding: quick fix (small, localized, no contract change) OR spec-worthy (behavior change, new boundary, cross-cutting). State your classification reasoning briefly."
  - "Apply quick fixes directly to the working tree."
  - "For spec-worthy findings, create `openspec/changes/<derived-slug>/` with proposal.md, tasks.md, and spec delta files. The slug derives from the audit type + a short hash of the findings."
  - "Final output: a summary naming what was fixed, what was specced, what was declined."
- [x] 5.2 Add `TriageContext` to the executor's prompt-build path:
  ```rust
  pub struct TriageContext {
      pub findings: String,
      pub audit_type: String,
      pub repo_url: String,
      pub canonical_specs_index: String,
  }
  ```
- [x] 5.3 New executor method `run_triage(workspace, ctx) -> Result<ExecutorOutcome>`. Substitutes the template's variables, spawns the wrapped CLI, returns the outcome.
- [x] 5.4 Tests:
  - `TriageContext` substitution: rendered prompt contains all four substitution payloads.
  - Stub executor returning `Completed` for a triage context: the method returns the same outcome shape.

## 6. Polling-loop integration + diff split

- [x] 6.1 In `autocoder/src/polling_loop.rs::run`, at the start of each iteration AFTER the revision-loop processing AND BEFORE the pending-change walk, drain the per-repo triage queue. For each pending `thread_ts`:
  1. Load the `AuditThreadState`.
  2. Build `TriageContext` (findings from the state, repo url from state, audit_type from state, canonical_specs_index from `<workspace>/openspec/specs/` directory listing).
  3. Invoke `executor.run_triage(workspace, ctx)`.
  4. On `Completed`: run the diff-split + two-PR creation (next task).
  5. On `Failed`: flip state's `status` to `TriageFailed` with the reason; post a reply in the audit thread naming the failure.
  6. On `AskUser`: existing chatops escalation path fires; the triage is in-progress; status stays `TriagePending`.
- [x] 6.2 Diff split + PR creation. New `pub fn process_completed_triage(workspace, audit_thread_state, ctx, github_cfg) -> Result<TriageOutcome>`:
  1. `git status --porcelain` to enumerate changed paths.
  2. Partition paths: paths inside `openspec/changes/<new_slug>/` → `spec_paths`; all others → `fixes_paths`.
  3. The new_slug is derived deterministically: `<audit-type>-<short-hash-of-findings>`. Test for uniqueness; if a collision is detected, suffix `-2`, `-3`, etc.
  4. If `fixes_paths.is_empty() && spec_paths.is_empty()`: post a thread reply with the LLM's summary (no PR created); set state to `Acted`.
  5. If `fixes_paths.is_empty()` AND `spec_paths.non_empty()`: create the spec PR only; set state to `Acted`.
  6. If `fixes_paths.non_empty()` AND `spec_paths.is_empty()`: create the fixes PR only; set state to `Acted`.
  7. Both non-empty: create both PRs.
- [x] 6.3 Two-PR mechanic:
  - Create the fixes branch: `git checkout -B <agent_branch>-fixes`, `git add <fixes_paths>`, `git commit -m "audit-triage fixes from <audit_type>"`, push, open PR.
  - Reset to base: `git checkout <agent_branch>`, then `git checkout -B <agent_branch>-spec`, `git add <spec_paths>`, `git commit -m "audit-triage spec proposal from <audit_type>"`, push, open PR.
  - Each PR's body cross-links the other ("This PR carries the code fixes; see #<other-pr> for the new spec change.")
  - State's `status` flips to `Acted` after both PRs land.
- [x] 6.4 Tests:
  - Stub executor returning Completed with code-only diff → only fixes PR is created.
  - Stub executor returning Completed with spec-only diff → only spec PR is created.
  - Stub executor returning Completed with both → both PRs are created; bodies cross-link.
  - Stub executor returning Completed with empty diff → no PRs; thread reply with LLM summary; state set to Acted.
  - Stub executor returning Failed → state flips to TriageFailed; thread reply names the reason; no PRs.
  - Slug collision: when `openspec/changes/<derived-slug>/` already exists, the suffix `-2` is applied.

## 7. Stale-entry pruning

- [x] 7.1 At each iteration's start (or once per day; pick whichever fits the existing periodic-housekeeping pattern), call `prune_stale_entries(state_dir, Duration::from_days(7))`. Removes audit-thread state files older than 7 days regardless of status.
- [x] 7.2 Tests:
  - State file 8 days old → removed by prune.
  - State file 5 days old → preserved.

## 8. README + docs updates

- [x] 8.1 In `docs/CHATOPS.md`, add a section "Acting on an audit's findings: `send it`" describing the verb, the audit-thread tracking, the two-PR output shape, and the 7-day staleness rule.
- [x] 8.2 In `docs/OPERATIONS.md`'s audits section, add a paragraph describing the audit → review → `send it` → fixes-PR + spec-PR → revise loop. Cross-reference `a01-pr-comment-revision-loop`'s revision-comment workflow for correcting either PR.
- [x] 8.3 In `docs/TROUBLESHOOTING.md`, add entries for the polite-refusal cases (untracked thread; stale thread; already-acted thread) so operators reading the bot's `✗` replies know what each means.

## 9. Spec delta

- [x] 9.1 The ADDED requirement in `openspec/changes/audit-reply-acts/specs/orchestrator-cli/spec.md` codifies: the `send it` verb in audit threads, the audit-thread state tracking, the triage-mode executor invocation, the diff-split + two-PR shape, the staleness rule, and the state-transition diagram.

## 10. Verification

- [x] 10.1 `cargo test` passes (new + existing).
- [x] 10.2 `openspec validate audit-reply-acts --strict` passes.
- [x] 10.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
