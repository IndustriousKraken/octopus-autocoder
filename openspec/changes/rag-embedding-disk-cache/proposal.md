## Why

The canonical-RAG store is in-memory only and the re-embed cadence requirement pins "Daemon restart re-embeds from scratch" — every restart re-chunks and re-embeds the full canonical corpus of every configured workspace through the embedding provider. The corpus barely changes between restarts, so nearly all of that work recomputes vectors for byte-identical chunks: startup latency (documented at ~30 seconds per workspace on CPU — times the whole fleet) and, for hosted providers, per-token cost, paid on every restart, upgrade, and crash recovery for embeddings the daemon already computed.

## What Changes

- A disk-backed embedding cache under the daemon's cache directory: vectors keyed by a content hash of (embedding provider, model, chunk text). Workspace-init and post-archive rebuilds consult it first and call the provider only for misses, writing new vectors through.
- Restart cost becomes proportional to what actually changed since the last run — typically zero provider calls — instead of the corpus size.
- The cache is exactly cache-category data: re-creatable, safe to delete, pruned of dead entries on each write (only hashes seen in the current corpus are kept), and invalidated naturally by its key (a model or provider change misses everything; a chunk edit misses that chunk).
- Fail-open on cache trouble: an unreadable or corrupt cache file logs a WARN and degrades to today's full re-embed, then overwrites the cache.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `orchestrator-cli`: the "RAG re-embed cadence (workspace init and post-archive)" requirement gains the disk cache (the restart-re-embeds-from-scratch scenario becomes restart-rebuilds-from-cache), and the "Canonical-spec RAG configuration and pipeline" requirement's persist-nothing clause is scoped to STORE state, explicitly carving out the embedding-vector cache.

## Impact

- `autocoder/src/rag/` (store build + `rebuild_capabilities`): consult/populate the cache around the existing `embed_batch` calls.
- Cache file per workspace under `<cache_dir>/rag-embeddings/<workspace-basename>.json`, written atomically (tempfile + rename, the daemon's standard pattern).
- No config changes; no behavior change when the cache is absent (first run after upgrade is one full embed that seeds it).
