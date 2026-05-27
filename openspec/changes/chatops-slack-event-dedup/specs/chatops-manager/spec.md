## ADDED Requirements

### Requirement: Slack inbound listener deduplicates redelivered events
The Slack inbound listener SHALL maintain an in-memory cache of recently-processed `app_mention` events keyed by `(channel, ts, user)` — the tuple that uniquely identifies a Slack message regardless of how many times Slack delivers it across envelopes or reconnects. Before dispatching an event (after the drop-before-dispatch filters return Pass), the listener SHALL look up the event's key in the cache. A cache hit SHALL skip the dispatch entirely; the listener still sends the envelope ack (so Slack stops redelivering) but does NOT post a reply, submit a control-socket action, or otherwise execute the operator's intent a second time.

#### Scenario: First delivery dispatches normally; subsequent redeliveries are suppressed
- **WHEN** the listener receives an `app_mention` event with `(channel=C, ts=T, user=U)` for the first time AND the event passes all drop-before-dispatch filters
- **THEN** the dedup cache returns `Fresh`
- **AND** the dispatcher is invoked
- **AND** the cache records the key
- **WHEN** the listener receives the same event again (Slack redelivery)
- **THEN** the dedup cache returns `Duplicate { suppressed_count: 1 }`
- **AND** the dispatcher is NOT invoked
- **AND** no chatops reply is posted
- **AND** an INFO log records the suppression

#### Scenario: Multiple redeliveries increment the suppressed count
- **WHEN** the same event is redelivered three times after the initial delivery
- **THEN** the cache returns `Duplicate { suppressed_count: 1 }`, then `2`, then `3` for each subsequent call
- **AND** the dispatcher is invoked exactly once (for the initial delivery)
- **AND** three INFO logs are emitted (one per suppression) with monotonically increasing suppressed_count values

#### Scenario: Different events do not collide
- **WHEN** the listener receives events with different `(channel, ts, user)` tuples
- **THEN** each event's first delivery returns `Fresh`
- **AND** the dispatcher is invoked for each
- **AND** no suppression occurs

#### Scenario: Cache persists across listener reconnect cycles
- **WHEN** the listener processes an event AND then the WebSocket disconnects AND the listener reconnects
- **AND** Slack redelivers the same event on the new connection
- **THEN** the dedup cache returns `Duplicate { suppressed_count: 1 }` (the cache persists across reconnect; the dedup decision is preserved)
- **AND** the dispatcher is NOT invoked

### Requirement: Dedup cache has bounded capacity and TTL with operator-configurable knobs
The dedup cache SHALL enforce both a maximum capacity (LRU eviction past the cap) AND a per-entry TTL (entries older than TTL are treated as Fresh on next lookup). Default capacity is `100`; default TTL is `600` seconds (10 minutes). Configurable via `chatops.slack.dedup_cache_capacity` (max `10000` with WARN-and-clamp) AND `chatops.slack.dedup_cache_ttl_secs` (max `3600` with WARN-and-clamp). Capacity `0` is permitted AND disables dedup behaviorally (every event is treated as Fresh; legacy pre-this-spec behavior).

#### Scenario: Capacity bound enforced via LRU eviction
- **WHEN** the cache has capacity `2` AND three distinct keys are inserted in succession
- **THEN** the first-inserted key is evicted to make room for the third
- **AND** a subsequent lookup of the first key returns `Fresh` (it's no longer in the cache)

#### Scenario: TTL bound treats stale entries as fresh
- **WHEN** a key is inserted into the cache AND `ttl_secs + 1` seconds pass AND the same key is looked up again
- **THEN** the cache returns `Fresh` (the stale entry is treated as not-present)
- **AND** the entry is replaced with a new insertion

#### Scenario: Capacity 0 disables dedup
- **WHEN** `chatops.slack.dedup_cache_capacity` is set to `0`
- **THEN** every `check_and_insert` call returns `Fresh`
- **AND** the dispatcher is invoked for every event regardless of past deliveries (today's pre-spec behavior is preserved verbatim)

#### Scenario: Out-of-bounds config values are clamped with WARN
- **WHEN** `chatops.slack.dedup_cache_capacity` is set to `50000`
- **THEN** the resolved capacity is `10000`
- **AND** a WARN log fires at startup naming both the requested and clamped values
- **WHEN** `chatops.slack.dedup_cache_ttl_secs` is set to `7200`
- **THEN** the resolved TTL is `3600`
- **AND** a WARN log fires at startup

### Requirement: Dedup suppression is logged at INFO with the key and suppressed count
Each cache hit SHALL emit a single INFO log line naming the dedup key fields AND the running suppressed-count for that key. Operators investigating "the bot didn't reply to my message" can grep journalctl for `deduplicated event` lines AND confirm whether their message was received-and-suppressed (vs not received at all OR dropped by a different filter).

#### Scenario: Suppression log format includes the dedup key and count
- **WHEN** a duplicate event is suppressed by the dedup cache
- **THEN** an INFO log fires with text containing `deduplicated event`, the channel ID, the ts, the user ID, AND the `suppressed_count` value
- **AND** the log uses the field naming convention `channel=<channel> ts=<ts> user=<user> suppressed_count=<n>` consistent with other slack-inbound listener log lines
