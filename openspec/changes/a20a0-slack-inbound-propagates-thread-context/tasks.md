## 1. Capture thread_ts from the Slack inbound envelope

- [ ] 1.1 In `autocoder/src/chatops/slack.rs`, extend `AppMentionEvent` (around line 631) with:
  ```rust
  #[serde(default)]
  pub thread_ts: Option<String>,
  ```
  The `#[serde(default)]` annotation handles top-level mentions, where Slack does NOT include `thread_ts` in the payload — they deserialize with `None`.
- [ ] 1.2 Verify (or add) a unit test that deserializes a fixture Slack `app_mention` payload with `thread_ts: "9999.1234"` AND asserts `event.thread_ts == Some("9999.1234".to_string())`. Also test the top-level-mention case (no `thread_ts` field present) deserializes to `event.thread_ts.is_none()`.

## 2. Swap the production dispatch call to forward context

- [ ] 2.1 At `autocoder/src/chatops/slack.rs:1184` (the `ctx.dispatcher.handle_message(...)` call), replace the call with `handle_message_with_context(...)`. The new arguments:
  ```rust
  let reply = ctx.dispatcher.handle_message_with_context(
      &normalized_text,
      &event.channel,
      event.thread_ts.as_deref(),
      event.user.as_deref(),
      &bot_mention,
      &repos,
      &submitter,
  ).await;
  ```
- [ ] 2.2 Verify there are no other production call sites of `handle_message` in `autocoder/src/`. Search: `grep -rn "\.handle_message(" autocoder/src/`. Any non-test hit needs the same swap.

## 3. Prevent regression: mark `handle_message` test-only OR delete it

Pick ONE of the two options based on test-suite impact:

- [ ] 3.1 **Option A — Delete `handle_message`.** Remove the function in `autocoder/src/chatops/operator_commands.rs` (line 1845-1863). Update every test caller to use `handle_message_with_context(text, channel, None, None, bot_mention, repos, submitter)` directly. Remove `#[allow(dead_code)]` from `handle_message_in_thread` (line 1869) since that helper is now the closer-to-production entry point.
- [ ] 3.2 **Option B — Annotate `handle_message` `#[cfg(test)]`.** Change `pub async fn handle_message(...)` to `#[cfg(test)] pub async fn handle_message(...)` so the production build cannot link against it. Test callers continue to compile. Remove `#[allow(dead_code)]` from `handle_message_in_thread` AND `handle_message` becomes naturally non-dead under `cfg(test)`.
- [ ] 3.3 EITHER option closes the regression vector: any future production code that tries to call the no-context entry point fails to compile, surfacing the wiring contract at compile time.
- [ ] 3.4 Verify: `cargo build --release` succeeds; `cargo test` succeeds.

## 4. Regression-prevention integration test

- [ ] 4.1 Add a test (in `autocoder/src/chatops/slack.rs`'s test module OR a new `autocoder/tests/slack_inbound_send_it_threads.rs` integration test) that:
  - Constructs an `AppMentionEvent` with `text: "<@BOT> send it"`, `channel: "C0"`, `ts: "1.0"`, `user: Some("U_RAB")`, `thread_ts: Some("9999.1234")`.
  - Constructs the inbound dispatch context with a mock `ActionSubmitter` that captures whatever action it receives.
  - Pre-stamps an `AuditThreadState` for `thread_ts: "9999.1234"` so the read path succeeds.
  - Drives the inbound handler (the function at `slack.rs:1184` region).
  - Asserts: the mock `ActionSubmitter` received a `trigger_audit_action` with `thread_ts: "9999.1234"` (NOT that the listener returned `?` reaction).
- [ ] 4.2 The test SHALL fail against the pre-spec code (the dispatch call returns None, no `trigger_audit_action` is submitted). The test SHALL pass after the fix.
- [ ] 4.3 Add a parallel negative test: same setup but with `thread_ts: None` (top-level mention). Assert the parser returns `ParseOutcome::None` AND the listener applies the `?` reaction — confirming `send it` is correctly REFUSED at top level (the desired pre-spec behaviour preserved for non-threaded invocations).

## 5. Spec deltas

- [ ] 5.1 `openspec/changes/a31-slack-inbound-propagates-thread-context/specs/chatops-manager/spec.md` ADDs the listener-propagation requirement covering AppMentionEvent shape, the production call-site contract, AND the regression-test invariant.

## 6. Verification

- [ ] 6.1 `cargo test` passes — new tests + existing tests, including the existing parser-level tests in `operator_commands.rs` (those use `handle_message_in_thread` already AND should pass unchanged).
- [ ] 6.2 `openspec validate a31-slack-inbound-propagates-thread-context --strict` passes.
- [ ] 6.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
- [ ] 6.4 Manual verification on the live daemon (after the fix lands AND the daemon is updated):
  - Trigger any audit on a managed repo (`@<bot> audit drift <repo>`, etc.). Wait for the audit's threaded notification to post.
  - Reply `@<bot> send it` inside that thread.
  - Expected: bot replies with one of the documented send-it outcomes (`✓ acted on audit findings; triage queued (~Nm).`, `✗ thread stale`, `✗ thread already acted on`, OR `✗ no such audit thread`). NOT the `?` reaction.
  - Also verify on a NEW iteration: `@<bot> propose <repo> <text>` AND inspect the resulting `ProposalRequestState` file's `operator_user` field. Expected: contains the actual Slack user id (e.g. `U_RAB`), not the empty-string default.
