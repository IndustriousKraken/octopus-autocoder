## 1. EventDedupCache module

- [x] 1.1 Create `autocoder/src/chatops/event_dedup.rs`. Public surface:
  ```rust
  pub struct EventDedupCache { /* internals */ }
  #[derive(Debug, Clone, Hash, Eq, PartialEq)]
  pub struct DedupKey {
      pub channel: String,
      pub ts: String,
      pub user: String,
  }
  #[derive(Debug)]
  pub enum CheckResult {
      Fresh,
      Duplicate { suppressed_count: u32 },
  }
  impl EventDedupCache {
      pub fn new(capacity: usize, ttl: Duration) -> Self;
      pub fn check_and_insert(&self, key: DedupKey) -> CheckResult;
  }
  ```
- [x] 1.2 Internals: a `Mutex<LruCache<DedupKey, DedupEntry>>` where `DedupEntry { inserted_at: DateTime<Utc>, suppressed_count: u32 }`. The LRU enforces the capacity bound; TTL is enforced at lookup time (a key whose `inserted_at` is older than `ttl` is treated as Fresh and re-inserted).
- [x] 1.3 Add the `lru` crate dependency to `Cargo.toml`. Check crates.io for the current version per `check-current-versions-not-training`. (Prefer the unsync `lru` crate wrapped in a `Mutex` over async LRUs to keep the dep tree small; the lookup is fast enough not to need async-aware locking.)
- [x] 1.4 `check_and_insert(key)` behavior:
  - Lock the LRU.
  - Look up `key`. If present AND its entry's `inserted_at + ttl >= now`: increment the entry's `suppressed_count`, return `Duplicate { suppressed_count }`.
  - If present AND its entry's `inserted_at + ttl < now`: remove it, insert a fresh entry, return `Fresh`.
  - If absent: insert with `suppressed_count: 0`, return `Fresh`.
- [x] 1.5 Tests:
  - Fresh key returns `Fresh`; insertion is observable on the next call.
  - Repeat call with same key returns `Duplicate { suppressed_count: 1 }`, then `2`, then `3`...
  - Key past TTL → treated as Fresh on next call; suppressed_count resets to 0.
  - Capacity-1 cache: insert two keys; the first is evicted; checking the first returns Fresh.
  - Different keys (varying channel, ts, or user) don't collide.

## 2. Listener integration

- [x] 2.1 In `autocoder/src/chatops/slack.rs`, extend `InboundListenerContext` with `pub dedup_cache: Arc<EventDedupCache>`. Constructed once at `start_inbound_listener` time AND shared across every reconnect cycle within the listener's lifetime.
- [x] 2.2 In `process_app_mention` (or wherever the dispatch decision lives), AFTER the drop-before-dispatch filters return Pass AND BEFORE invoking the dispatcher, compute `DedupKey { channel, ts, user }` from the event AND call `ctx.dedup_cache.check_and_insert(key.clone())`:
  - On `Fresh`: proceed to dispatch (existing code path).
  - On `Duplicate { suppressed_count }`: log INFO naming the dedup key + `suppressed_count`; return false (skip dispatch). The envelope ack was already sent earlier in the event loop, so Slack knows we received the event — we just don't dispatch the redundant work.
- [x] 2.3 The INFO log format:
  ```
  INFO slack inbound: deduplicated event channel=<channel> ts=<ts> user=<user> suppressed_count=<n>
  ```
  Same field naming as other slack-inbound log lines so operators grepping `slack inbound:` see dedup events alongside other listener activity.
- [x] 2.4 Tests:
  - Fixture event delivered once → dispatcher invoked once.
  - Fixture event delivered twice in succession → dispatcher invoked once; INFO log fires on second delivery.
  - Fixture events with different keys delivered in succession → dispatcher invoked for each.
  - Dedup cache persists across listener reconnect (simulated by stopping + restarting the event loop within the same listener task) → second delivery still recognized as duplicate.

## 3. Config + clamps

- [x] 3.1 In `autocoder/src/config.rs`, extend `ChatOpsSlackConfig`:
  ```rust
  #[serde(default = "default_dedup_cache_capacity")]
  pub dedup_cache_capacity: usize,
  #[serde(default = "default_dedup_cache_ttl_secs")]
  pub dedup_cache_ttl_secs: u64,
  ```
  Defaults: capacity 100, ttl 600 seconds.
- [x] 3.2 Clamps: capacity above 10000 → clamp to 10000 with WARN. TTL above 3600 → clamp to 3600 with WARN. Value 0 is permitted for capacity (disables dedup; every event is Fresh) but not for TTL (TTL 0 is functionally equivalent to capacity 0; clamp to 1 with WARN to keep the semantics clear).
- [x] 3.3 Tests: default config parses with the documented defaults; explicit values within bounds pass through; out-of-bounds values are clamped with WARNs; capacity 0 parses without WARN AND disables dedup behaviorally.

## 4. Listener constructs the cache at startup

- [x] 4.1 In `start_inbound_listener`, after the existing setup, construct `let dedup_cache = Arc::new(EventDedupCache::new(config.dedup_cache_capacity, Duration::from_secs(config.dedup_cache_ttl_secs)));` AND store it in the `InboundListenerContext`.
- [x] 4.2 The cache is shared across the outer reconnect loop AND every inner event-loop cycle. Reconnects do NOT clear the cache. Listener task exit (daemon shutdown) drops the cache via Arc Drop.
- [x] 4.3 Tests:
  - Cache constructed with configured capacity and ttl.
  - Cache outlives a simulated reconnect cycle (same `Arc<EventDedupCache>` reference before and after the reconnect-and-resume helper runs).

## 5. README + docs updates

- [x] 5.1 In `docs/CHATOPS.md`, add a paragraph in the listener section describing the new dedup behavior, the cache config fields, AND when operators might want to tune them.
- [x] 5.2 In `docs/TROUBLESHOOTING.md`, add an entry: "Bot replied multiple times to a single message — this is Slack's at-least-once delivery, and the dedup cache prevents it. If you're still seeing duplicates, check `journalctl -u autocoder | grep 'deduplicated event'` to confirm the dedup is firing; if not, your `dedup_cache_capacity` may be 0 (disabled) or too small for your traffic volume."
- [x] 5.3 In `docs/CONFIG.md`, document the new `chatops.slack.dedup_cache_capacity` and `chatops.slack.dedup_cache_ttl_secs` fields.

## 6. Spec delta

- [x] 6.1 The ADDED requirement in `openspec/changes/chatops-slack-event-dedup/specs/chatops-manager/spec.md` codifies: the dedup cache and its keying, the cache-hit suppression behavior, the persistence across reconnect, the configuration fields with their bounds, and the per-suppression INFO log format.

## 7. Verification

- [x] 7.1 `cargo test` passes (new + existing).
- [x] 7.2 `openspec validate chatops-slack-event-dedup --strict` passes.
- [x] 7.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
