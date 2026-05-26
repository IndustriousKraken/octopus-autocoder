## 1. `ChatOpsBackend` trait extension

- [ ] 1.1 In `autocoder/src/chatops/mod.rs`, add to the `ChatOpsBackend` trait:
  ```rust
  /// Post a notification whose body might be long enough to warrant
  /// threading. Backends that support native threading (Slack) post the
  /// `top_line` as the top-level message and `thread_body` as a threaded
  /// reply. Backends without threading (default impl) concatenate the two
  /// with a separator and post as a single message via `post_notification`.
  async fn post_notification_with_thread(
      &self,
      channel: &str,
      top_line: &str,
      thread_body: &str,
  ) -> Result<()> {
      // Default impl: concatenate and degrade to post_notification
      let combined = format!("{top_line}\n\n{thread_body}");
      self.post_notification(channel, &combined).await
  }
  ```
- [ ] 1.2 Tests:
  - Default impl: a fake backend that captures `post_notification` calls; assert `post_notification_with_thread` issues one `post_notification` call whose body contains both `top_line` AND `thread_body` separated by a blank line.

## 2. Slack backend override

- [ ] 2.1 In `autocoder/src/chatops/slack.rs`, override `post_notification_with_thread`:
  - First call: `chat.postMessage` with `{"channel": <channel>, "text": <top_line>}`. Parse the response for `ts`. On Err: bubble up; the threaded reply is not attempted (the top-line failed; no point trying to thread to nothing).
  - Second call: `chat.postMessage` with `{"channel": <channel>, "text": <thread_body>, "thread_ts": <captured_ts>}`. On Err: log WARN naming the missing thread reply; do NOT bubble up. The top-line already landed; operators see the summary even if the thread detail post fails.
- [ ] 2.2 Tests (mockito):
  - Happy path: both Slack calls succeed; assert the second carries `thread_ts` matching the first's response `ts`; assert the function returns Ok.
  - Top-line post fails: assert the function returns Err with the underlying Slack error; assert no second call was made.
  - Thread reply post fails (top-line succeeded): assert the function returns Ok (the top-line is the user-visible signal); assert a WARN log fires.

## 3. Audit notification formatter

- [ ] 3.1 Add `pub fn format_audit_notification(audit_type: &str, repo_url: &str, findings: &AuditFindings) -> AuditNotification` in `autocoder/src/audits/mod.rs` (or a new sibling module). Returns:
  ```rust
  pub struct AuditNotification {
      pub top_line: String,
      pub thread_body: String,
      pub should_thread: bool,  // false when the body is short enough to inline
  }
  ```
- [ ] 3.2 Per-audit-type top-line formatting:
  - `architecture_brightline`: `📐 architecture_brightline on <repo>: <N> file(s) over line threshold; <M> duplicate signature(s)`. When N == 0 AND M == 0 AND `notify_on_clean=true`: `✅ architecture_brightline on <repo>: no findings`.
  - `drift_audit`: `🧭 drift_audit on <repo>: <N> spec/code divergence(s) detected`. When N == 0 AND `notify_on_clean=true`: `✅ drift_audit on <repo>: no findings`.
  - The proposal-creating audits (`missing_tests_audit`, `security_bug_audit`, `architecture_consultative`) use the existing `🔍 created proposal` format from `a02-audit-proposal-created-notification` — this helper does not change their notification path.
- [ ] 3.3 Threading threshold: `should_thread = thread_body.lines().count() > 3 || thread_body.chars().count() > 300`. Below the threshold, the body inlines into the top-line message (the scheduler posts via `post_notification` directly rather than `post_notification_with_thread`).
- [ ] 3.4 Length cap on the thread body: when `thread_body.chars().count() > 35_000`, truncate to the first 35,000 characters AND append:
  ```
  
  … [truncated; full findings at journalctl -u autocoder | grep audit_id=<audit_id>]
  ```
  where `<audit_id>` is `format!("{repo_sanitized}:{audit_type}:{utc_timestamp}")`. The audit-runner is responsible for stamping this same `audit_id` into the daemon log entries for the run so the operator's grep returns the full content.
- [ ] 3.5 Tests:
  - `format_audit_notification` for brightline with 7 files + 3 dupes: top_line contains both counts; thread_body contains the per-file + per-dupe detail; `should_thread: true` when the body exceeds threshold.
  - Empty-findings brightline with `notify_on_clean=true`: top_line is the `✅` form; thread_body is empty; `should_thread: false`.
  - Single-line drift finding: thread_body is short; `should_thread: false`; the body inlines.
  - 50,000-char body: thread_body is truncated at 35,000 chars; the truncation pointer is appended.

## 4. Wire scheduler through the new helper + method

- [ ] 4.1 In `autocoder/src/audits/scheduler.rs`, replace the existing chatops-post call sites for audit findings with the new flow:
  ```rust
  let notification = format_audit_notification(audit_type, &repo_url, &findings);
  if notification.should_thread {
      backend.post_notification_with_thread(
          channel, &notification.top_line, &notification.thread_body
      ).await?;
  } else {
      backend.post_notification(channel, &notification.top_line).await?;
  }
  ```
- [ ] 4.2 The `notify_on_clean` gate remains as today: when `false`, empty-findings audits skip the entire notification block (no top-line, no thread).
- [ ] 4.3 Tests:
  - Brightline with many findings + chatops configured: assert exactly one `post_notification_with_thread` call with the documented top-line + thread-body.
  - Brightline with no findings + `notify_on_clean=true`: assert one `post_notification_with_thread` call with the `✅` top-line and an empty thread body — wait, actually with `should_thread: false`, this should route through `post_notification` instead (no thread for empty findings). Update the assertion accordingly: one `post_notification` call with the `✅` top-line.
  - Brightline with no findings + `notify_on_clean=false`: assert no chatops call at all.
  - Drift with short findings (1 line): assert one `post_notification` call with the inline form.
  - Drift with long findings (5 lines): assert one `post_notification_with_thread` call.

## 5. ValidationExhausted notification updates

- [ ] 5.1 In `autocoder/src/audits/scheduler.rs` (or wherever `a01-audit-proposal-self-validation`'s `ValidationExhausted` notification fires), update to use `post_notification_with_thread` when the validation error is multi-line OR exceeds 300 chars:
  - Top-line: `❌ <repo>: <audit_type> produced an invalid proposal that failed openspec validation after <retries_attempted> retries.`
  - Thread body: the captured validation error excerpt (the existing payload that's currently dumped into the single message).
- [ ] 5.2 Tests:
  - ValidationExhausted with a short error: routes through `post_notification` (inline).
  - ValidationExhausted with a multi-line error: routes through `post_notification_with_thread`; top-line names the audit + repo + retry count; thread body contains the full error.

## 6. README + docs updates

- [ ] 6.1 In `docs/CHATOPS.md`, add a paragraph describing the new threaded notification pattern for audit findings: top-line summary in the channel, full findings in the thread, per-audit-type emoji conventions.
- [ ] 6.2 In `docs/CHATOPS.md`'s experimental-backends section, note that non-Slack backends fall back to the concatenated single-message form via the default trait impl; per-backend native-threading overrides may be added in future changes.

## 7. Spec delta

- [ ] 7.1 The ADDED requirement in `openspec/changes/chatops-audit-findings-in-threads/specs/chatops-manager/spec.md` codifies: the new trait method, the default-impl graceful-degradation contract, the Slack-override two-call sequence, the per-audit-type top-line formatting, the threading threshold, the length-cap with truncation pointer, and the no-thread-when-empty rule.

## 8. Verification

- [ ] 8.1 `cargo test` passes (new + existing).
- [ ] 8.2 `openspec validate chatops-audit-findings-in-threads --strict` passes.
- [ ] 8.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
