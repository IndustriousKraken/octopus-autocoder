## Why

Audit chatops output today is dumped as a single multi-line message into the configured channel. The `architecture_brightline` audit in particular can produce findings that dwarf a screen: every file over the line threshold gets a line, every duplicate signature detected gets a paragraph. A repo with several violations produces hundreds of lines in one message. When multiple audits run in the same iteration, their messages run together with no visual separator — the bottom of one audit's wall of text touches the top of the next audit's. Operators copying a specific finding to share or to investigate must scroll past adjacent audits, with no clear boundary between them.

The Slack-native solution is threading: a one-line top-level summary in the channel, with the wall-of-text findings posted as a threaded reply to that summary. Operators scanning the channel see a clean list of per-audit summary lines; clicking into a thread surfaces the full findings only when wanted. The channel stops looking like a log dump and starts looking like a navigable feed.

This pattern composes with the existing audit notifications without disrupting them. The proposal-creating audits (`missing_tests_audit`, `security_bug_audit`, `architecture_consultative`) already post one-line `🔍 created proposal` notifications via `a02-audit-proposal-created-notification` — those are already concise. The new threading helps the audits that DON'T create proposals: `architecture_brightline` (pure-data file/signature scan) and `drift_audit` (spec/code divergences). It also helps the `❌` failure notification from `a01-audit-proposal-self-validation` when the openspec validation error is multi-line.

## What Changes

**New backend method: `post_notification_with_thread`.** The `ChatOpsBackend` trait gains:

```rust
async fn post_notification_with_thread(
    &self,
    channel: &str,
    top_line: &str,
    thread_body: &str,
) -> Result<()>;
```

A default implementation concatenates the two strings with a separator and calls the existing `post_notification` — graceful degradation for any backend that hasn't implemented native threading. The Slack backend overrides with real threading: posts `top_line` via `chat.postMessage` (captures the `ts` from the response), then posts `thread_body` via a second `chat.postMessage` with `thread_ts` set to that `ts`. Experimental backends (Discord, Teams, Mattermost, Matrix) keep the default implementation in v1; future per-backend overrides can use each platform's native threading model.

**Per-audit-type emoji and top-line shape.** Audit notifications use a consistent shape so operators can scan the channel:

- 📐 `architecture_brightline on <repo>: <N> file(s) over line threshold; <M> duplicate signature(s)`
- 🧭 `drift_audit on <repo>: <N> spec/code divergence(s) detected`
- The proposal-creating audits continue to use `🔍 created proposal ...` from `a02-audit-proposal-created-notification` (unchanged).
- The validation-failure notification (`a01-audit-proposal-self-validation`) keeps its `❌` emoji but uses the threaded shape when the validation error is multi-line: top-line summary, threaded reply with the full error.

The top-line summary names the audit type, the repo, AND the most useful single-clause summary of what was found. The summary clause is audit-specific (file counts for brightline, divergence counts for drift). When the audit has no findings AND `notify_on_clean=true`, the top-line reads `✅ <audit_type> on <repo>: no findings` and no thread is posted.

**Threshold for when to thread.** Threading only fires when the body would actually benefit. The rule: thread when the findings body exceeds 3 lines OR 300 characters. Below that, the findings inline into the top-line message (no thread, since a thread for a one-line finding is more friction than value). Above that, the top-line is the summary and the thread contains the full body.

**Length cap with truncation pointer.** Slack's per-message limit is 40,000 characters. The threaded reply truncates at 35,000 characters when the body exceeds it (preserving 5,000 chars of safety margin for formatting), with the truncated body ending in:

```
… [truncated; full findings at journalctl -u autocoder | grep audit_id=<id>]
```

The `<audit_id>` is a deterministic identifier the audit-runner stamps into its log entries — e.g. `<repo-sanitized>:<audit-type>:<timestamp>`. Operators with very large finding bodies grep the daemon log for the full text.

**Audit scheduler routes through the new method.** Today's scheduler-side notification posting (the wall-of-text `post_notification` call) is replaced with `post_notification_with_thread` calls that split the body. The split logic lives at the scheduler so per-audit-type body construction is consistent.

**Existing audit output formats unchanged.** This change is about transport, not content. The brightline audit's "file X has Y lines, threshold is Z" lines, the drift audit's "spec X says A but code says B" entries — both produce the same text as today; the text just lives in a thread now instead of inline in the channel.

## Impact

- **Affected specs:** `chatops-manager` — one ADDED requirement covering the new trait method, the default-impl graceful degradation, the Slack override, the per-audit-type top-line shape, the threading threshold, and the length-cap truncation.
- **Affected code:**
  - `autocoder/src/chatops/mod.rs` — add `post_notification_with_thread` to the `ChatOpsBackend` trait with a default impl that concatenates and calls `post_notification`.
  - `autocoder/src/chatops/slack.rs` — override `post_notification_with_thread` with real Slack threading: two `chat.postMessage` calls, the second with `thread_ts` set to the first's response `ts`. Tests via mockito.
  - `autocoder/src/audits/scheduler.rs` (and any audit-result-posting code paths) — replace the existing wall-of-text `post_notification` call with a call to `format_audit_notification(audit_type, repo, findings) -> (top_line, thread_body)` and then route through `post_notification_with_thread`. The `format_audit_notification` helper centralizes the per-audit-type top-line construction.
  - New helper `fn format_audit_notification(audit_type, repo, findings) -> AuditNotification` returning `{ top_line: String, thread_body: String, inline_when_short: bool }`. The audit scheduler decides whether to thread based on the threshold check; the helper just builds the strings.
  - Tests:
    - `format_audit_notification` per audit type: brightline with 7 files over threshold + 3 dupes produces the documented top-line; drift with 2 divergences produces its documented top-line; empty findings produces the `✅ no findings` form.
    - Threshold test: body ≤3 lines AND ≤300 chars renders inline (no `post_notification_with_thread` thread split — body in top-line); body exceeding either threshold renders threaded.
    - Length-cap test: body of 50,000 chars produces a thread reply of ≤35,000 chars ending in the truncation pointer.
    - Default-impl test (against a non-Slack backend stub): `post_notification_with_thread` concatenates top_line + thread_body and calls `post_notification` with the result.
    - Slack-override test (mockito): `post_notification_with_thread` issues two `chat.postMessage` calls in order; the second carries `thread_ts` matching the first's `ts`.

- **Operator-visible behavior:** audit chatops messages become navigable. The channel shows one summary line per audit; clicking a thread surfaces the findings. Multi-audit iterations no longer have their walls of text running together. The `ValidationExhausted` failure notifications are also cleaner when the openspec error is multi-line.
- **Breaking:** no for operators using Slack. Operators on experimental backends see the existing concat-into-one-message behavior (the default impl). The new method is additive — existing `post_notification` callers are unchanged.
- **Acceptance:** `cargo test` passes (new + existing). A live brightline audit run against a fixture repo with many findings produces ONE top-level Slack message with the summary line AND ONE threaded reply with the full findings. The next audit's notification appears below the first audit's top-line in the channel as a separate item, not concatenated with it.
