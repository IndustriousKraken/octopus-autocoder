//! In-memory dedup cache for Slack inbound events.
//!
//! Slack's Socket Mode delivery contract is explicitly at-least-once: a
//! single underlying event can be redelivered multiple times across
//! reconnect cycles when the WebSocket ack for an envelope doesn't reach
//! Slack. Without application-level dedup, each redelivery flows through
//! the full listener pipeline (filters → dispatcher → reply post),
//! producing duplicate bot replies.
//!
//! `EventDedupCache` is a `(channel, ts, user)`-keyed LRU with a
//! per-entry TTL. The first `check_and_insert` for a given key returns
//! `Fresh` and records the key; subsequent calls within the TTL window
//! return `Duplicate { suppressed_count }` so the listener can skip the
//! redundant dispatch while still acking the envelope.

use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use lru::LruCache;

/// The unique identifier for a Slack `app_mention` event regardless of
/// how many times Slack delivers it across envelopes or reconnects.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct DedupKey {
    pub channel: String,
    pub ts: String,
    pub user: String,
}

/// Outcome of looking up a key in the cache.
#[derive(Debug)]
pub enum CheckResult {
    /// The key was newly inserted; the caller SHOULD proceed with
    /// dispatch.
    Fresh,
    /// The key was already present (and within the TTL window); the
    /// caller SHOULD skip dispatch. `suppressed_count` is the running
    /// count of how many times this key has been suppressed since the
    /// cache window for it opened.
    Duplicate { suppressed_count: u32 },
}

#[derive(Debug, Clone)]
struct DedupEntry {
    inserted_at: DateTime<Utc>,
    suppressed_count: u32,
}

/// LRU-bounded, TTL-respecting cache keyed by [`DedupKey`].
///
/// Capacity `0` is a valid configuration that disables dedup entirely:
/// every `check_and_insert` returns `Fresh` and the cache holds no
/// entries.
pub struct EventDedupCache {
    capacity: usize,
    ttl: Duration,
    entries: Mutex<Option<LruCache<DedupKey, DedupEntry>>>,
}

impl EventDedupCache {
    /// Construct a cache with the given capacity and per-entry TTL.
    /// `capacity == 0` disables dedup (every event is Fresh).
    pub fn new(capacity: usize, ttl: Duration) -> Self {
        let inner = NonZeroUsize::new(capacity).map(LruCache::new);
        Self {
            capacity,
            ttl,
            entries: Mutex::new(inner),
        }
    }

    /// Look the key up in the cache. On miss (or stale entry), insert
    /// and return `Fresh`. On hit, increment the entry's
    /// `suppressed_count` and return `Duplicate`.
    ///
    /// Cache TTL is enforced at lookup time: a present-but-stale entry
    /// (`inserted_at + ttl < now`) is treated as not-present and
    /// replaced with a fresh entry.
    pub fn check_and_insert(&self, key: DedupKey) -> CheckResult {
        if self.capacity == 0 {
            return CheckResult::Fresh;
        }
        let mut guard = self.entries.lock().expect("dedup cache mutex poisoned");
        let cache = guard.as_mut().expect("capacity>0 ⇒ Some(LruCache)");
        let now = Utc::now();
        if let Some(entry) = cache.get_mut(&key) {
            let age_ok = now
                .signed_duration_since(entry.inserted_at)
                .to_std()
                .map(|d| d <= self.ttl)
                .unwrap_or(false);
            if age_ok {
                entry.suppressed_count = entry.suppressed_count.saturating_add(1);
                let count = entry.suppressed_count;
                return CheckResult::Duplicate {
                    suppressed_count: count,
                };
            }
            // Stale: drop the entry and fall through to fresh insert.
            cache.pop(&key);
        }
        cache.put(
            key,
            DedupEntry {
                inserted_at: now,
                suppressed_count: 0,
            },
        );
        CheckResult::Fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(channel: &str, ts: &str, user: &str) -> DedupKey {
        DedupKey {
            channel: channel.into(),
            ts: ts.into(),
            user: user.into(),
        }
    }

    #[test]
    fn fresh_key_returns_fresh_and_records() {
        let cache = EventDedupCache::new(8, Duration::from_secs(60));
        let r = cache.check_and_insert(k("C", "1.0", "U"));
        assert!(matches!(r, CheckResult::Fresh), "first call must be Fresh");
        let r2 = cache.check_and_insert(k("C", "1.0", "U"));
        assert!(
            matches!(r2, CheckResult::Duplicate { suppressed_count: 1 }),
            "second call must be Duplicate {{ suppressed_count: 1 }}, got {r2:?}"
        );
    }

    #[test]
    fn repeated_calls_increment_suppressed_count() {
        let cache = EventDedupCache::new(4, Duration::from_secs(60));
        let key = k("C", "1.0", "U");
        assert!(matches!(cache.check_and_insert(key.clone()), CheckResult::Fresh));
        for expected in 1..=5u32 {
            match cache.check_and_insert(key.clone()) {
                CheckResult::Duplicate { suppressed_count } => {
                    assert_eq!(suppressed_count, expected);
                }
                CheckResult::Fresh => panic!("expected Duplicate, got Fresh"),
            }
        }
    }

    #[test]
    fn key_past_ttl_is_treated_as_fresh_and_resets_count() {
        // A zero TTL guarantees any prior entry is stale on the next
        // lookup, so we can verify the stale-replace path without
        // sleeping.
        let cache = EventDedupCache::new(4, Duration::from_secs(0));
        let key = k("C", "1.0", "U");
        assert!(matches!(cache.check_and_insert(key.clone()), CheckResult::Fresh));
        // Even though the key is present, the TTL has elapsed, so the
        // next call must treat it as Fresh and reset the count.
        let r = cache.check_and_insert(key.clone());
        assert!(
            matches!(r, CheckResult::Fresh),
            "stale key must be treated as Fresh, got {r:?}"
        );
    }

    #[test]
    fn capacity_one_evicts_first_on_second_insert() {
        let cache = EventDedupCache::new(1, Duration::from_secs(60));
        let a = k("C", "1.0", "Ua");
        let b = k("C", "2.0", "Ub");
        assert!(matches!(cache.check_and_insert(a.clone()), CheckResult::Fresh));
        // Inserting b evicts a (LRU, capacity = 1).
        assert!(matches!(cache.check_and_insert(b.clone()), CheckResult::Fresh));
        // a is no longer in the cache → Fresh.
        let r = cache.check_and_insert(a);
        assert!(
            matches!(r, CheckResult::Fresh),
            "evicted key must be Fresh on re-lookup, got {r:?}"
        );
    }

    #[test]
    fn different_keys_do_not_collide() {
        let cache = EventDedupCache::new(16, Duration::from_secs(60));
        let keys = [
            k("C1", "1.0", "Ua"),
            k("C2", "1.0", "Ua"), // different channel
            k("C1", "2.0", "Ua"), // different ts
            k("C1", "1.0", "Ub"), // different user
        ];
        for key in &keys {
            let r = cache.check_and_insert(key.clone());
            assert!(
                matches!(r, CheckResult::Fresh),
                "distinct key {key:?} must be Fresh, got {r:?}"
            );
        }
    }

    #[test]
    fn capacity_zero_disables_dedup() {
        let cache = EventDedupCache::new(0, Duration::from_secs(60));
        let key = k("C", "1.0", "U");
        // Every call returns Fresh; no suppression.
        for _ in 0..5 {
            let r = cache.check_and_insert(key.clone());
            assert!(matches!(r, CheckResult::Fresh), "capacity 0 must be Fresh");
        }
    }
}
