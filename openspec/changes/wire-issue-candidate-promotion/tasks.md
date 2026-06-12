# Tasks

## 1. Capture the candidate's thread at post time

- [ ] 1.1 Add `#[serde(default)] thread_ts: Option<String>` AND `#[serde(default)] channel: Option<String>` to `CandidateState` in `lanes/ingestion.rs`. Confirm existing candidate JSON files still deserialize (defaults fill the absent fields).
- [ ] 1.2 Change `post_candidate` to post the candidate via the thread-returning notification path (the `post_notification_with_thread` API on the chatops backend) instead of fire-and-forget `shared::notify`, and record the returned `thread_ts` (and the channel) on the written `CandidateState`. When the post degrades and returns no `thread_ts`, persist the candidate with `thread_ts: None` and log at warn — the candidate is posted but not reply-matchable.
- [ ] 1.3 Update the notification text to instruct `@<bot> send it` (not bare `send it`), keeping the "nothing is queued until you do" clause.
- [ ] 1.4 Add a lookup that returns the posted candidate whose recorded `thread_ts` equals a given `thread_ts` by scanning `candidates_dir(state_root)` (mirror the survey scan; `Posted` status only is the promotable case). Return the parsed `CandidateState`.

## 2. Promotion control-socket action

- [ ] 2.1 Add a `promote_issue_candidate` action handler in `control_socket.rs` that requires `url`, `candidate_id`, `channel`, and `thread_ts`; resolves the repo and its workspace (`workspace::resolve_path(&state.paths, &repo)`); loads the candidate via `read_candidate(&state.paths.state, candidate_id)`; and dispatches on its status.
- [ ] 2.2 On a posted candidate, call `promote_candidate(&workspace, &state.paths.state, &candidate)` and return `{ ok: true, slug, path }`. Map the already-exists / write errors from `promote_candidate` to `{ ok: false, error }`.
- [ ] 2.3 On an already-promoted candidate, return a structured `{ ok: true, already_promoted: true, slug }` so the dispatcher can word its reply without re-writing.
- [ ] 2.4 Register the action string in the control-socket dispatch match alongside `trigger_audit_action` / `queue_brownfield_batch_request`.

## 3. Dispatcher: third `send it` context

- [ ] 3.1 In `dispatch_send_it_on_audit` (the shared `send it` handler), after the audit-thread read misses AND `try_send_it_on_survey` returns `None`, call a new `try_send_it_on_issue_candidate(thread_ts, repositories, submitter)` before returning `SEND_IT_REFUSE_UNTRACKED`.
- [ ] 3.2 Implement `try_send_it_on_issue_candidate`: scan `candidates_dir` under the dispatcher's state root (the same root used for audit-thread reads) for a candidate whose `thread_ts` matches; return `None` when none matches (so the caller falls through to the refusal).
- [ ] 3.3 On a posted match, resolve the candidate's repo url, submit `promote_issue_candidate { url, candidate_id, channel, thread_ts }`, and return the success reply naming the written `issues/<slug>/` and the queue. On a handler error, return `✗ send it: could not promote candidate: <error>`.
- [ ] 3.4 On an already-promoted match, return the already-promoted reply and submit nothing.
- [ ] 3.5 Update `SEND_IT_REFUSE_UNTRACKED` to name all three valid contexts (audit thread, brownfield-survey thread, issue-candidate thread).

## 4. Reachability cleanup

- [ ] 4.1 Remove the `#[allow(dead_code)]` from `read_candidate` and `promote_candidate` once they are reached from the live promotion path; remove the "wired by a follow-up" notes.

## 5. Tests

- [ ] 5.1 `post_candidate` persists the captured `thread_ts`/`channel` on the candidate state, and the notification text contains `@<bot> send it`. Derive the assertion from behavior (state round-trip), not by asserting the full notification string.
- [ ] 5.2 The thread-match lookup returns the posted candidate for a matching `thread_ts` and `None` for a non-matching one; an already-promoted candidate is reported as such, not promoted again.
- [ ] 5.3 `promote_issue_candidate` handler: a posted candidate is written to `issues/<slug>/` (public-origin includes `report-body.md`) and flipped to promoted; a second invocation is idempotent (no re-write, reports already-promoted); a missing candidate returns an error.
- [ ] 5.4 Dispatcher: a `send it` whose `thread_ts` matches a posted candidate submits `promote_issue_candidate`; a `send it` matching an already-promoted candidate submits nothing; a `send it` matching no record returns the three-context refusal. Audit-thread and survey matches still take precedence (regression).

## 6. Docs

- [ ] 6.1 Update the operator-facing chatops documentation to list the issue-candidate thread as a third `send it` context, mirroring how the audit and brownfield-survey contexts are documented.
