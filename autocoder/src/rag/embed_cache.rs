//! Disk-backed embedding cache for the canonical-spec RAG pipeline.
//!
//! Both embed events (workspace-init full build, post-archive partial
//! rebuild) consult this cache before calling the embedding provider, so
//! a daemon restart re-pays provider cost only for chunks that actually
//! changed — typically nothing.
//!
//! - **Key** = SHA-256 over `provider \0 model \0 chunk-text`. Everything
//!   that determines a vector's validity is in the key, so invalidation is
//!   structural: edit a chunk → new key; switch model or provider → all-new
//!   keys. No versioning or invalidation logic to maintain.
//! - **File** = one JSON map (`{ hex-key: [f32, …] }`) per workspace
//!   basename under `<cache_dir>/rag-embeddings/`, written atomically
//!   (tempfile + rename) — the daemon's standard state-write pattern.
//! - **Prune-on-write**: [`write_through`] serializes exactly the keys of
//!   the just-built store, so entries for deleted or edited chunks fall out
//!   on the next write instead of accumulating.
//! - **Fail-open**: a missing file is an empty cache (cold start, no WARN);
//!   an unreadable or corrupt file logs ONE WARN, is treated as empty (full
//!   provider embed), and is overwritten by the subsequent write-through.
//!   The cache never fails a rebuild the provider could serve.

use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

/// The on-disk cache shape: hex-SHA-256 key → embedding vector.
pub type CacheMap = HashMap<String, Vec<f32>>;

/// Compute the content-hash key for one chunk. `provider` is the stable
/// operator-facing provider string (e.g. `ollama`), `model` the configured
/// model name. NUL separators keep the three fields unambiguous so no
/// pair of distinct `(provider, model, text)` triples can collide by
/// concatenation.
pub fn cache_key(provider: &str, model: &str, chunk_text: &str) -> String {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    ctx.update(provider.as_bytes());
    ctx.update(&[0]);
    ctx.update(model.as_bytes());
    ctx.update(&[0]);
    ctx.update(chunk_text.as_bytes());
    let digest = ctx.finish();
    let mut hex = String::with_capacity(digest.as_ref().len() * 2);
    for byte in digest.as_ref() {
        // Infallible: writing to a String never errors.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Load the cache at `path`. Fail-open: a missing file is an empty cache
/// (normal cold start, silent); an unreadable or corrupt file logs ONE
/// WARN and is likewise treated as empty so the caller does a full embed
/// and overwrites the bad file on write-through.
pub fn load(path: &Path) -> CacheMap {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<CacheMap>(&bytes) {
            Ok(map) => map,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    "canonical RAG embedding cache is corrupt ({e}); treating as empty \
                     and rewriting on this build's write-through"
                );
                CacheMap::new()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => CacheMap::new(),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                "canonical RAG embedding cache unreadable ({e}); treating as empty \
                 and rewriting on this build's write-through"
            );
            CacheMap::new()
        }
    }
}

/// Atomically write `entries` to `path` (tempfile + rename), creating the
/// parent directory lazily. `entries` is exactly the current store's keys,
/// so this both prunes dead entries and retains every live one.
pub fn write_through(path: &Path, entries: &CacheMap) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("rag embedding cache path {} has no parent", path.display()))?;
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating rag-embeddings dir {}", dir.display()))?;
    let tmp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("creating tempfile in {}", dir.display()))?;
    serde_json::to_writer(&tmp, entries)
        .with_context(|| format!("serializing rag embedding cache {}", path.display()))?;
    tmp.persist(path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn key_is_stable_and_field_sensitive() {
        let k = cache_key("ollama", "nomic-embed-text", "hello");
        assert_eq!(k.len(), 64, "sha-256 hex is 64 chars");
        assert!(k.chars().all(|c| c.is_ascii_hexdigit()));
        // Deterministic.
        assert_eq!(k, cache_key("ollama", "nomic-embed-text", "hello"));
        // Any field change → different key.
        assert_ne!(k, cache_key("openai_compatible", "nomic-embed-text", "hello"));
        assert_ne!(k, cache_key("ollama", "other-model", "hello"));
        assert_ne!(k, cache_key("ollama", "nomic-embed-text", "hell"));
    }

    #[test]
    fn separator_prevents_field_boundary_collision() {
        // Without the NUL separators, ("a","bc",...) and ("ab","c",...)
        // would hash the same concatenation.
        assert_ne!(cache_key("a", "bc", "x"), cache_key("ab", "c", "x"));
    }

    #[test]
    fn missing_file_is_empty_and_silent() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nope.json");
        assert!(load(&path).is_empty());
    }

    #[test]
    fn roundtrip_write_then_load() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("sub/ws.json");
        let mut map = CacheMap::new();
        map.insert("k1".into(), vec![1.0, 2.5, -3.0]);
        write_through(&path, &map).unwrap();
        assert!(path.exists(), "write_through creates the file + parent dir");
        let got = load(&path);
        assert_eq!(got, map);
    }

    #[test]
    fn corrupt_file_loads_empty() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("ws.json");
        std::fs::write(&path, b"{not valid json").unwrap();
        assert!(load(&path).is_empty());
        // And a subsequent write overwrites it with something valid.
        let mut map = CacheMap::new();
        map.insert("k".into(), vec![0.0]);
        write_through(&path, &map).unwrap();
        assert_eq!(load(&path), map);
    }
}
