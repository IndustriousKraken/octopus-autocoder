## ADDED Requirements

### Requirement: ChatOpsBackend exposes a threaded-notification method with graceful degradation
The `ChatOpsBackend` trait SHALL expose `async fn post_notification_with_thread(&self, channel: &str, top_line: &str, thread_body: &str) -> Result<()>`. The trait's default implementation SHALL concatenate `top_line` + a blank-line separator + `thread_body` AND post the result via `post_notification` (no native threading). Backends with native threading support SHALL override the method with platform-appropriate threading. Backends without native threading continue working unchanged via the default impl.

#### Scenario: Default implementation concatenates for non-threading backends
- **WHEN** a backend that has not overridden `post_notification_with_thread` is asked to post one
- **THEN** the call results in exactly one `post_notification` invocation whose body contains `top_line`, then a blank line, then `thread_body`
- **AND** no platform-specific threading metadata is involved

#### Scenario: Slack override uses chat.postMessage + thread_ts
- **WHEN** `SlackBackend::post_notification_with_thread` is called with non-empty `top_line` and `thread_body`
- **THEN** the backend issues two HTTP POSTs to `chat.postMessage`
- **AND** the first POST's body is `{"channel": <channel>, "text": <top_line>}` and the response is parsed for the `ts` field
- **AND** the second POST's body is `{"channel": <channel>, "text": <thread_body>, "thread_ts": <captured_ts>}`
- **AND** the call returns Ok when both POSTs succeed

#### Scenario: Slack top-line failure aborts before threading
- **WHEN** the first `chat.postMessage` (the top-line) fails
- **THEN** the second POST is NOT attempted
- **AND** the function returns Err with the Slack error from the first call

#### Scenario: Slack thread-reply failure does not bubble up
- **WHEN** the first POST succeeds AND the second POST (the thread reply) fails
- **THEN** the function returns Ok (the top-line is the user-visible signal)
- **AND** a WARN log is emitted naming the missed thread reply

### Requirement: Audit findings post via the threaded-notification path when long enough to benefit
The audit scheduler SHALL route findings notifications through `post_notification_with_thread` when the body would benefit from threading: body line count > 3 OR body character count > 300. Below the threshold, findings inline into a single-message `post_notification` call. Empty findings posted under `notify_on_clean=true` use the inline path (`✅ <audit> on <repo>: no findings`); empty findings under `notify_on_clean=false` produce no notification at all (existing behaviour).

#### Scenario: Long findings post to a thread
- **WHEN** an audit produces findings whose body exceeds 3 lines OR 300 characters
- **THEN** the scheduler calls `post_notification_with_thread` with the audit-type's top-line summary AND the full findings body
- **AND** no separate `post_notification` call is made

#### Scenario: Short findings inline into the top-line
- **WHEN** an audit produces findings whose body is ≤3 lines AND ≤300 characters
- **THEN** the scheduler calls `post_notification` with the combined top-line + inline-body text
- **AND** no thread is created

#### Scenario: Empty findings with notify_on_clean=true posts the `✅` form inline
- **WHEN** an audit produces zero findings AND its `notify_on_clean` setting is `true`
- **THEN** the scheduler calls `post_notification` with the `✅ <audit_type> on <repo>: no findings` text
- **AND** no threaded reply is created (the body is empty; nothing to thread)

#### Scenario: Empty findings with notify_on_clean=false posts nothing
- **WHEN** an audit produces zero findings AND its `notify_on_clean` setting is `false`
- **THEN** no chatops call is made (existing behaviour preserved)

### Requirement: Audit top-line uses per-type emoji and audit-specific summary
The top-line of each audit notification SHALL be formatted per audit type so operators can scan the channel and immediately recognize the audit producing each message:

- `architecture_brightline`: `📐 architecture_brightline on <repo>: <N> file(s) over line threshold; <M> duplicate signature(s)`
- `drift_audit`: `🧭 drift_audit on <repo>: <N> spec/code divergence(s) detected`
- The proposal-creating audits (`missing_tests_audit`, `security_bug_audit`, `architecture_consultative`) use the `🔍 created proposal` form from `a02-audit-proposal-created-notification` (unchanged by this requirement; their notifications are already concise and do not need threading).

When an audit has zero findings AND `notify_on_clean=true`, the top-line is `✅ <audit_type> on <repo>: no findings` (uniform across audit types).

#### Scenario: Brightline summary names both counts
- **WHEN** an `architecture_brightline` notification fires with 7 files over threshold AND 3 duplicate signatures
- **THEN** the top-line is `📐 architecture_brightline on <repo>: 7 file(s) over line threshold; 3 duplicate signature(s)`

#### Scenario: Drift summary names the divergence count
- **WHEN** a `drift_audit` notification fires with 2 divergences detected
- **THEN** the top-line is `🧭 drift_audit on <repo>: 2 spec/code divergence(s) detected`

#### Scenario: No-findings top-line uses the `✅` form uniformly
- **WHEN** any audit fires with zero findings AND `notify_on_clean=true`
- **THEN** the top-line is `✅ <audit_type> on <repo>: no findings` regardless of audit type

### Requirement: Thread body truncates at 35,000 characters with a pointer to the daemon log
When the thread body would exceed 35,000 characters, it SHALL be truncated to 35,000 characters AND end with a marker pointing at the daemon log so operators can grep the full content. The 35,000 cap leaves a 5,000-character safety margin under Slack's per-message limit of 40,000.

#### Scenario: Body over 35k is truncated with the documented pointer
- **WHEN** the thread body would be 50,000 characters
- **THEN** the actual thread body posted is exactly 35,000 characters (or close to it; the truncation point is text-aware where reasonable) AND ends with `\n\n… [truncated; full findings at journalctl -u autocoder | grep audit_id=<audit_id>]`
- **AND** the `<audit_id>` is a deterministic identifier of the form `<repo-sanitized>:<audit-type>:<utc-timestamp>` that the audit-runner has stamped into its daemon-log entries for the same run

#### Scenario: Body under 35k is posted in full
- **WHEN** the thread body is 1,000 characters
- **THEN** the thread body is posted as-is with no truncation pointer

### Requirement: ValidationExhausted notifications use threading when the error is long
The `❌ <audit_type> produced an invalid proposal` notification from `a01-audit-proposal-self-validation` SHALL use the threaded-notification path when the validation error excerpt exceeds the threading threshold (>3 lines or >300 characters). The top-line names the audit, the repo, and the retry count; the thread body contains the full validation error. Short errors continue to inline into a single message.

#### Scenario: ValidationExhausted with multi-line error uses threading
- **WHEN** an audit returns `ValidationExhausted` with a `final_error` body exceeding the threading threshold
- **THEN** the scheduler routes the notification through `post_notification_with_thread`
- **AND** the top-line is `❌ <repo>: <audit_type> produced an invalid proposal that failed openspec validation after <retries_attempted> retries.`
- **AND** the thread body contains the full validation error excerpt

#### Scenario: ValidationExhausted with short error inlines
- **WHEN** an audit returns `ValidationExhausted` with a `final_error` body within the threading threshold
- **THEN** the scheduler routes the notification through `post_notification` (the existing inline path)
