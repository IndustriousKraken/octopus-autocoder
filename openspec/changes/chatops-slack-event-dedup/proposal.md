## Why

Slack's Socket Mode delivery contract is explicitly **at-least-once**. If autocoder's WebSocket ack for an envelope doesn't reach Slack — because the connection dropped before Slack confirmed receipt, because of a transient network blip, because of any reconnect-driven race — Slack redelivers the same underlying event on the next connection. Each redelivery flows through the listener's full dispatch path: parse, drop-before-dispatch filters, dispatcher, action submission, reply post. The operator sees the same reply two or three times.

A real incident: an operator posted `@<bot> rebuild-specs coterie` once. The bot acked the verb three times within ~60 seconds:

```
12:14 ✓ rebuild scheduled for coterie — will run within ~300s (current iteration must finish first)
12:14 ✓ rebuild scheduled for coterie — will run within ~300s (current iteration must finish first)
12:15 ✓ rebuild scheduled for coterie — will run within ~300s (current iteration must finish first)
```

The listener's ack is already sent BEFORE dispatch (per `chatops-slack-inbound-listener`'s implementation), so the ack ordering isn't the problem. The problem is Slack's at-least-once contract: even a perfectly-ordered ack can be lost in transit, especially across the reconnect cycles the same spec's exponential-backoff reconnect logic explicitly handles. Idempotency at the application level — the listener processing each unique event AT MOST ONCE regardless of how many times Slack delivers it — is the contract-compliant defense.

A secondary motivation: the rebuild-specs flag the user's incident triggered is itself idempotent (queuing the same rebuild twice is a no-op), so the WORK was duplicated harmlessly. But not every chatops verb is idempotent. Posting three triage runs for a `@<bot> propose` or three revisions on a PR via `@<bot> revise` would actually do the work multiple times — wasting LLM tokens, polluting the agent branch with redundant commits, and confusing operators. Application-level dedup protects every verb uniformly.

## What Changes

**Track recently-processed Slack events in an in-memory LRU.** A new `EventDedupCache` keyed by `(channel, ts, user)` — the tuple that uniquely identifies a Slack message regardless of how many times Slack delivers it across envelopes or reconnects. Default capacity is 100 entries; eviction is LRU. Entries older than 10 minutes are pruned in addition to LRU pressure (Slack's redelivery window is typically minutes, not hours).

**Listener checks the cache before dispatching.** When an `app_mention` event arrives AND passes the existing drop-before-dispatch filters (channel allowlist, self-author, bot-author, leading mention), the listener computes the dedup key from the event AND looks it up in the cache:

- **Cache miss**: insert into the cache, proceed to dispatch normally.
- **Cache hit**: skip dispatch entirely. Send the envelope ack as usual (Slack still gets the ack signal). Log INFO naming the deduplicated event so operators can correlate with the per-message redelivery storm.

The ack-before-dispatch ordering from the previous spec stays in place; dedup happens between the ack and the dispatch, AFTER the filters but BEFORE the dispatcher round-trip.

**The cache lives on the listener task, not in process-wide state.** Each Slack listener instance owns its own cache. When the listener reconnects (after a disconnect), the cache PERSISTS across the reconnect — that's the key property; otherwise we'd lose the dedup signal exactly when we need it most. Listener restart (daemon shutdown + start) clears the cache; that's acceptable since the redelivery window is short relative to restart frequency.

**Configurable cap and TTL.** A new `chatops.slack.dedup_cache_capacity: u32` (default `100`, max `10000` with WARN-and-clamp) AND `chatops.slack.dedup_cache_ttl_secs: u64` (default `600` for 10 minutes, max `3600` for 1 hour). Operators with high-traffic channels can raise both; the defaults are sized for normal operator-command traffic where redelivery windows are short and unique-message volume is low.

**Dedup applies to `app_mention` events only.** Other event types (today: ignored after ack; future: any other events autocoder learns to handle) take their own dedup decision per type. The keying tuple may differ — e.g., `member_joined_channel` events use a different identifier — so the cache is currently typed-narrow to `app_mention`. The trait surface allows extension if needed.

**Visibility into dedup activity.** Each cache hit logs at INFO with the dedup key + a count of how many times this event has been suppressed in the current cache window. Operators investigating "why isn't the bot responding to my message" can grep the journal for the dedup INFO lines to confirm the message was received but skipped as a duplicate (vs not received at all).

## Impact

- **Affected specs:** `chatops-manager` — one ADDED requirement covering the event dedup cache, its keying, the ack-but-skip-dispatch behavior on cache hit, the capacity + TTL configuration, and the per-cache-hit logging.
- **Affected code:**
  - New module `autocoder/src/chatops/event_dedup.rs`:
    ```rust
    pub struct EventDedupCache {
        capacity: usize,
        ttl: Duration,
        entries: Mutex<LruCache<DedupKey, DateTime<Utc>>>,
    }
    pub struct DedupKey {
        pub channel: String,
        pub ts: String,
        pub user: String,
    }
    impl EventDedupCache {
        pub fn new(capacity: usize, ttl: Duration) -> Self;
        // Returns true if the key was already present (suppress dispatch);
        // false if the key was newly inserted (proceed to dispatch).
        pub fn check_and_insert(&self, key: DedupKey) -> CheckResult;
    }
    pub enum CheckResult {
        Fresh,                          // newly inserted; proceed
        Duplicate { suppressed_count: u32 },  // already present; skip
    }
    ```
    Uses the `lru` crate for the underlying LRU; add to Cargo.toml after verifying the current version per `check-current-versions-not-training`.
  - `autocoder/src/chatops/slack.rs` — extend `process_app_mention` (or the calling site in the listener's event loop):
    - After the drop-before-dispatch filters return Pass, compute `DedupKey { channel, ts, user }` from the event.
    - Call `cache.check_and_insert(key)`.
    - On `Duplicate { suppressed_count }`: log INFO naming the key + count, return without dispatching.
    - On `Fresh`: proceed to the existing dispatch path.
  - `autocoder/src/chatops/slack.rs` — `start_inbound_listener` constructs an `EventDedupCache` once at listener startup (capacity + ttl from config) AND passes it to the event-loop via the `InboundListenerContext`. The cache outlives reconnect cycles.
  - `autocoder/src/config.rs` — add `chatops.slack.dedup_cache_capacity` (default 100, max 10000 with WARN-and-clamp) AND `chatops.slack.dedup_cache_ttl_secs` (default 600, max 3600 with WARN-and-clamp).
  - Tests:
    - LRU + TTL behavior: insert N entries; assert eviction at capacity; entries past TTL age out.
    - `check_and_insert` on a fresh key returns `Fresh`; subsequent call with the same key returns `Duplicate { suppressed_count: 1 }`; third call returns `Duplicate { suppressed_count: 2 }`.
    - Listener integration: fixture event delivered twice (simulating Slack redelivery) → dispatch fires once; second delivery returns Duplicate; INFO log captured naming the suppression.
    - Listener integration across reconnect: fixture event delivered on connection A; reconnect; same event redelivered on connection B → second delivery returns Duplicate (cache persists across reconnect).
    - Different events do not collide: events with different `(channel, ts, user)` tuples all process; no false-positive suppression.
    - Config opt-out: setting `dedup_cache_capacity: 0` disables dedup entirely (every event processes; legacy behavior).

- **Operator-visible behavior:** Slack redelivery storms no longer produce duplicate bot replies. The first delivery is processed normally; subsequent redeliveries of the same event are suppressed AND logged. Operators investigating "why does the bot reply twice sometimes" can grep journalctl for the dedup INFO lines AND see the suppressed-count to understand Slack's redelivery behavior.
- **Breaking:** no. The dedup is purely additive; first-delivery dispatch is unchanged. Operators who somehow want the legacy behavior set `dedup_cache_capacity: 0`.
- **Acceptance:** `cargo test` passes (new + existing). A fixture that delivers the same `app_mention` event three times in succession (simulating Slack's at-least-once redelivery) results in exactly one dispatcher invocation; three INFO log lines record the deduplications; the operator sees exactly one bot reply in the channel.
