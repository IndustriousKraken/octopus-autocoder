## Context

The RAG store (`autocoder/src/rag/`) is in-memory; canon's re-embed cadence requirement explicitly pins "no on-disk persistence" and a full re-embed per restart, costed at ~30s/workspace on CPU plus per-token fees on hosted providers. Across a fleet (11 repos in the reference deployment) every restart, upgrade, and crash recovery re-pays that for a corpus that is byte-identical to the last run except where an archive folded a delta. First flagged as `roadmap/investigate-rag-reembedding` (2026-07-13); investigation conclusion: the recomputation is real waste with a natural fix, so this change replaces the roadmap item.

## Goals / Non-Goals

**Goals:**
- Restart/init embedding cost proportional to corpus *changes*, not corpus size.
- Zero new operator surface: no config, no commands, no migration.

**Non-Goals:**
- Persisting the in-memory *store* (index structure) itself — rebuild-from-cache is cheap once vectors are free; persisting the store would couple its layout to a file format for no measurable gain.
- Cross-workspace sharing or dedup — workspaces have distinct corpora; a shared cache saves little and adds locking.
- Cache size caps or age-based eviction — the write-through retains only currently-live keys, so the file is bounded by corpus size by construction.

## Decisions

- **Key = hash(provider | model | chunk text).** Everything that determines a vector's validity is in the key, so invalidation is structural: edit a chunk → new key; switch models → all-new keys. No versioning or invalidation logic to maintain. Include the embedding dimension implicitly via model name.
- **One JSON file per workspace basename under `<cache_dir>/rag-embeddings/`.** The cache category ("re-creatable but kept") is exactly this data's contract, and per-workspace files keep writes small and independent. Atomic tempfile+rename matches every other daemon state write. A binary format was considered and rejected: corpus-sized JSON is megabytes at worst, and debuggability wins.
- **Prune-on-write (keep only keys seen in the current build).** Makes the file self-cleaning without a separate GC pass; superseded chunks disappear on the next write-through.
- **Fail-open everywhere.** Cache-read trouble degrades to today's behavior (full embed) and self-heals via write-through; cache-write trouble logs a WARN and costs nothing but the next run's misses. The cache must never make a rebuild fail that the provider could serve — mirroring the requirement's existing fail-open posture for re-embed failures.
- **Post-archive partial rebuilds use the same path.** `rebuild_capabilities` consults and writes the same cache; its misses are exactly the folded capability's changed chunks.

## Risks / Trade-offs

- [Hash collision returns a wrong vector] → Use a cryptographic hash (SHA-256); collision probability is negligible against corpus sizes measured in thousands of chunks.
- [Stale cache after a provider-side model update under the same model name] → Same exposure exists today across post-archive partial rebuilds (mixed-vintage vectors in one store); hosted embedding models are version-pinned by name in practice. Deleting the cache directory is the documented, always-safe reset.
- [Concurrent writes from parallel workspace tasks] → Files are per-workspace and each workspace has one polling task (the existing serial-per-repo invariant); no cross-task sharing exists to race.
