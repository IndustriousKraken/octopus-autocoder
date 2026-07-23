## 1. Cache module

- [x] 1.1 Add a cache helper in `autocoder/src/rag/` (load, lookup by key, write-through with prune-to-live-keys): keys are SHA-256 over provider, model, and chunk text; file at `<cache_dir>/rag-embeddings/<workspace-basename>.json`, written via tempfile + rename; unreadable/corrupt files are treated as empty with one WARN.
- [x] 1.2 Resolve the cache directory through `DaemonPaths` (cache category) and create it lazily on first write.

## 2. Wire into both embed events

- [x] 2.1 In the workspace-init full-corpus build, look up each chunk before batching provider calls; embed only the misses; write the cache after the store is built, logging the hit/miss counts.
- [x] 2.2 In `rebuild_capabilities` (post-archive partial rebuild), use the same lookup/write-through path for the affected capabilities' chunks.

## 3. Tests

- [x] 3.1 Unit tests with a counting fake embed client: second build of an unchanged corpus makes zero provider calls; an edited chunk causes exactly one; a provider/model change misses everything.
- [x] 3.2 Unit tests for the file lifecycle: write-through retains only live keys (a removed chunk's entry disappears); corrupt file → WARN + full embed + healthy rewrite; missing directory is created.
- [x] 3.3 Run the full `cargo test` suite; confirm existing RAG pipeline and re-embed cadence tests pass unchanged.

## 4. Housekeeping

- [x] 4.1 Remove `roadmap/investigate-rag-reembedding.md` — this change is the investigation's outcome, superseding the roadmap item.
