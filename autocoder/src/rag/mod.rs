//! Canonical-spec RAG (retrieval-augmented context) for the
//! per-execution implementer (a21).
//!
//! ## Daemon plumbing
//!
//! The daemon's `cli/run.rs` startup registers a single
//! [`CanonicalRagRegistry`] AND exposes it via the
//! [`shared_registry`]/[`set_shared_registry`] process-global. The
//! polling loop's workspace-init AND post-archive hooks read this
//! global to register/rebuild stores. The control-socket handler
//! ([`crate::control_socket::handle_query_canonical_specs`]) reads it
//! to look up the right store for the calling workspace.
//!
//! Using a process-global is the pragmatic alternative to threading the
//! registry through every polling-loop entry point; the registry is
//! conceptually a singleton per-daemon, and `cli/run.rs` constructs it
//! once at startup AND publishes it before the polling tasks spawn.
//!
//! Design summary:
//! - Embeds every `openspec/specs/<capability>/spec.md` chunk at
//!   workspace init.
//! - Re-embeds affected capabilities after archives that touch
//!   canonical specs.
//! - The vector STORE is in-memory only and rebuilt on workspace-init;
//!   a disk-backed embedding cache ([`embed_cache`]) keyed by
//!   `hash(provider, model, chunk-text)` under `<cache_dir>/rag-embeddings/`
//!   lets a restart rebuild from cached vectors with zero provider calls
//!   when the corpus is unchanged. The cache is CACHE-category data —
//!   safe to delete; its absence just restores the full-embed cost.
//! - Per-workspace store registry, keyed by sanitized basename, lives
//!   in the daemon (`CanonicalRagRegistry`); the control socket relays
//!   `query_canonical_specs` requests from per-execution MCP children
//!   to the right store.

pub mod chunking;
pub mod embed_cache;
pub mod embedding;

use std::sync::OnceLock;

/// Process-global registry handle. Set once at daemon startup by
/// `cli/run.rs`; read by the polling loop's RAG hooks AND the control
/// socket's `query_canonical_specs` handler.
static SHARED_REGISTRY: OnceLock<CanonicalRagRegistry> = OnceLock::new();
/// Process-global snapshot of the active `CanonicalRagConfig`. Set
/// alongside [`SHARED_REGISTRY`] at startup; consulted by the polling
/// loop's RAG hooks to decide whether to build/rebuild stores.
static SHARED_CONFIG: OnceLock<crate::config::CanonicalRagConfig> = OnceLock::new();

/// Set the process-global registry + config. Called once by `cli/run.rs`
/// after parsing the config; idempotent only in the sense that
/// `OnceLock::set` returns `Err` on the second call (silently ignored).
pub fn set_shared(registry: CanonicalRagRegistry, config: crate::config::CanonicalRagConfig) {
    let _ = SHARED_REGISTRY.set(registry);
    let _ = SHARED_CONFIG.set(config);
}

/// Read the process-global registry handle, if set.
pub fn shared_registry() -> Option<&'static CanonicalRagRegistry> {
    SHARED_REGISTRY.get()
}

/// Read the process-global config snapshot, if set.
pub fn shared_config() -> Option<&'static crate::config::CanonicalRagConfig> {
    SHARED_CONFIG.get()
}

/// Workspace-init RAG hook. Called once per workspace, on the first
/// iteration after daemon startup. Builds + embeds the canonical
/// corpus and registers the store under the workspace's sanitized
/// basename. Fail-open: any error logs WARN and the store is omitted
/// from the registry (subsequent queries return empty Vec).
pub async fn workspace_init_hook(paths: &crate::paths::DaemonPaths, workspace: &std::path::Path) {
    let Some(registry) = shared_registry() else {
        return;
    };
    let Some(config) = shared_config() else {
        return;
    };
    if !config.is_active() {
        return;
    }
    let basename = sanitize_workspace_basename(workspace);
    if registry.contains(&basename).await {
        return; // Already initialized for this workspace.
    }
    // Resolve the per-workspace embedding-cache file through DaemonPaths
    // (cache category). The store carries this path so the post-archive
    // partial rebuild consults + writes the same cache.
    let cache_path = Some(paths.rag_embeddings_cache_path(&basename));
    match CanonicalRagStore::rebuild_for_workspace(workspace, config.clone(), cache_path).await {
        Ok(store) => {
            let count = store.entry_count().await;
            registry
                .register(basename.clone(), std::sync::Arc::new(store))
                .await;
            tracing::info!(
                workspace_basename = %basename,
                "canonical RAG embedded {count} chunks for workspace `{basename}`"
            );
        }
        Err(e) => {
            tracing::warn!(
                workspace_basename = %basename,
                "canonical RAG workspace-init failed: {e:#}; \
                 query_canonical_specs will return empty Vec"
            );
        }
    }
}

/// Post-archive RAG hook. Given the workspace path AND the list of
/// canonical-spec capability slugs that the just-landed archive
/// touched, re-embed those capabilities in the store. Fail-open: any
/// error logs WARN and the prior embeds are retained.
pub async fn post_archive_hook(
    workspace: &std::path::Path,
    affected_capabilities: &[String],
) {
    if affected_capabilities.is_empty() {
        return;
    }
    let Some(registry) = shared_registry() else {
        return;
    };
    let Some(config) = shared_config() else {
        return;
    };
    if !config.is_active() || !config.reembed_on_archive {
        return;
    }
    let basename = sanitize_workspace_basename(workspace);
    let Some(store) = registry.get(&basename).await else {
        return;
    };
    match store
        .rebuild_capabilities(workspace, affected_capabilities)
        .await
    {
        Ok(()) => {
            tracing::info!(
                workspace_basename = %basename,
                "canonical RAG re-embedded {} capabilities after archive: {:?}",
                affected_capabilities.len(),
                affected_capabilities
            );
        }
        Err(e) => {
            tracing::warn!(
                workspace_basename = %basename,
                "canonical RAG post-archive re-embed failed: {e:#}; prior embeds retained"
            );
        }
    }
}

/// Inspect a git diff between two refs in `workspace` and return the
/// set of capability slugs whose `openspec/specs/<cap>/spec.md` was
/// touched. Used by the polling loop's post-archive hook to drive
/// [`post_archive_hook`].
pub fn capabilities_touched_between(
    workspace: &std::path::Path,
    range: &str,
) -> Vec<String> {
    let output = match std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["diff", "--name-only", range, "--", "openspec/specs/"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let mut caps = std::collections::HashSet::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split('/').collect();
        if parts.len() >= 4
            && parts[0] == "openspec"
            && parts[1] == "specs"
            && parts.last().map(|p| *p == "spec.md").unwrap_or(false)
        {
            caps.insert(parts[2].to_string());
        }
    }
    let mut out: Vec<String> = caps.into_iter().collect();
    out.sort();
    out
}

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::{CanonicalRagConfig, ChunkStrategy};

pub use chunking::{ChunkInput, chunk_canonical_spec};
pub use embedding::{EmbedClient, build_client};

/// A single retrieved chunk + its similarity score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagHit {
    pub capability: String,
    pub requirement_title: String,
    pub requirement_body: String,
    pub scenario_titles: Vec<String>,
    pub relevance_score: f32,
}

struct StoreEntry {
    input: ChunkInput,
    embedding: Vec<f32>,
}

/// In-memory canonical-spec store for one workspace.
pub struct CanonicalRagStore {
    #[allow(dead_code)]
    workspace_basename: String,
    provider: Arc<dyn EmbedClient>,
    config: CanonicalRagConfig,
    entries: RwLock<Vec<StoreEntry>>,
    /// Per-workspace embedding-cache file (`<cache_dir>/rag-embeddings/
    /// <basename>.json`), resolved via `DaemonPaths`. `None` disables the
    /// disk cache (every embed goes to the provider) — used by tests and
    /// any caller that omits a cache path.
    cache_path: Option<PathBuf>,
}

impl CanonicalRagStore {
    /// Build a store from a workspace by globbing
    /// `<workspace>/openspec/specs/<cap>/spec.md`, chunking each, and
    /// embedding every chunk via the configured provider. Fails open on
    /// any error — the daemon's workspace-init hook logs WARN and
    /// omits the store from the registry on failure.
    pub async fn rebuild_for_workspace(
        workspace: &Path,
        config: CanonicalRagConfig,
        cache_path: Option<PathBuf>,
    ) -> Result<Self> {
        let workspace_basename = sanitize_workspace_basename(workspace);
        let provider = build_client(&config)?;
        let store = Self {
            workspace_basename,
            provider,
            config,
            entries: RwLock::new(Vec::new()),
            cache_path,
        };
        let spec_paths = discover_canonical_specs(workspace)?;
        store.embed_paths(&spec_paths).await?;
        Ok(store)
    }

    /// Re-embed a named set of capabilities. Removes existing entries
    /// for each capability, re-chunks + re-embeds the matching spec
    /// file, and appends. Capabilities whose spec file is missing
    /// (e.g. removed by the archive) are dropped from the store.
    pub async fn rebuild_capabilities(
        &self,
        workspace: &Path,
        capabilities: &[String],
    ) -> Result<()> {
        let mut new_paths = Vec::new();
        let to_remove: std::collections::HashSet<&str> =
            capabilities.iter().map(|s| s.as_str()).collect();
        {
            let mut guard = self.entries.write().await;
            guard.retain(|e| !to_remove.contains(e.input.capability.as_str()));
        }
        let specs_root = workspace.join("openspec/specs");
        for cap in capabilities {
            let path = specs_root.join(cap).join("spec.md");
            if spec_within_root(&specs_root, &path) {
                new_paths.push(path);
            }
        }
        self.embed_paths(&new_paths).await
    }

    /// Embed the query and return the top-k chunks by cosine
    /// similarity. `top_k` defaults to the config's `top_k`.
    pub async fn query(&self, query: &str, top_k: Option<usize>) -> Result<Vec<RagHit>> {
        let q_embed = self.provider.embed_one(query).await?;
        let top_k = top_k.unwrap_or(self.config.top_k);
        let guard = self.entries.read().await;
        let mut scored: Vec<(f32, &StoreEntry)> = guard
            .iter()
            .map(|e| (cosine_similarity(&q_embed, &e.embedding), e))
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        Ok(scored
            .into_iter()
            .map(|(score, entry)| RagHit {
                capability: entry.input.capability.clone(),
                requirement_title: entry.input.requirement_title.clone(),
                requirement_body: entry.input.text.clone(),
                scenario_titles: entry.input.scenario_titles.clone(),
                relevance_score: score,
            })
            .collect())
    }

    #[allow(dead_code)]
    pub fn workspace_basename(&self) -> &str {
        &self.workspace_basename
    }

    #[allow(dead_code)]
    pub fn config(&self) -> &CanonicalRagConfig {
        &self.config
    }

    async fn embed_paths(&self, paths: &[PathBuf]) -> Result<()> {
        let mut all_chunks: Vec<ChunkInput> = Vec::new();
        for path in paths {
            let chunks =
                chunk_canonical_spec(path, self.config.chunk_strategy.clone_or_default())?;
            all_chunks.extend(chunks);
        }

        // Embed the newly-supplied chunks, consulting the disk cache so only
        // misses reach the provider (hits load their vector with no provider
        // call). An empty `paths` — e.g. a rebuild of a capability whose
        // spec.md was deleted by the archive — adds nothing here but still
        // falls through to the write-through below, which prunes the removed
        // capability's key from the cache.
        if !all_chunks.is_empty() {
            // Absent/corrupt cache → empty map → everything is a miss
            // (today's full-embed behavior, self-healed by the write-through).
            let cache = self
                .cache_path
                .as_deref()
                .map(embed_cache::load)
                .unwrap_or_default();
            let keys: Vec<String> = all_chunks
                .iter()
                .map(|c| self.cache_key_for(&c.text))
                .collect();
            let mut embeddings: Vec<Option<Vec<f32>>> =
                keys.iter().map(|k| cache.get(k).cloned()).collect();
            let miss_idx: Vec<usize> = embeddings
                .iter()
                .enumerate()
                .filter_map(|(i, e)| e.is_none().then_some(i))
                .collect();
            let hits = all_chunks.len() - miss_idx.len();

            if !miss_idx.is_empty() {
                let miss_texts: Vec<String> =
                    miss_idx.iter().map(|&i| all_chunks[i].text.clone()).collect();
                let fresh = self.provider.embed_batch(&miss_texts).await?;
                if fresh.len() != miss_texts.len() {
                    return Err(anyhow::anyhow!(
                        "provider returned {} embeddings for {} chunks",
                        fresh.len(),
                        miss_texts.len()
                    ));
                }
                for (&slot, embedding) in miss_idx.iter().zip(fresh) {
                    embeddings[slot] = Some(embedding);
                }
            }

            let mut guard = self.entries.write().await;
            for (input, embedding) in all_chunks.into_iter().zip(embeddings) {
                // Every slot is filled: hits came from the cache, misses
                // were just embedded above.
                let embedding = embedding.expect("every chunk resolved to an embedding");
                guard.push(StoreEntry { input, embedding });
            }
            drop(guard);

            if self.cache_path.is_some() {
                tracing::info!(
                    cache_hits = hits,
                    cache_misses = miss_idx.len(),
                    "canonical RAG embedding cache: {hits} hit, {} miss",
                    miss_idx.len()
                );
            }
        }

        // Write the cache through from the ENTIRE current store, keyed by
        // each live chunk. This prunes superseded keys (a removed/edited
        // chunk is simply no longer present) while retaining every other
        // capability's vectors — so a post-archive partial rebuild does not
        // drop the unaffected caps from the cache. Runs even when nothing
        // was added this call, so a deleted capability's key is pruned.
        // Fail-open: a write error is a WARN, never a rebuild failure.
        if let Some(cache_path) = self.cache_path.as_deref() {
            let guard = self.entries.read().await;
            let mut fresh: embed_cache::CacheMap =
                embed_cache::CacheMap::with_capacity(guard.len());
            for e in guard.iter() {
                fresh.insert(self.cache_key_for(&e.input.text), e.embedding.clone());
            }
            drop(guard);
            if let Err(e) = embed_cache::write_through(cache_path, &fresh) {
                tracing::warn!(
                    path = %cache_path.display(),
                    "canonical RAG embedding cache write failed: {e:#}; \
                     unchanged chunks will re-embed on the next event"
                );
            }
        }
        Ok(())
    }

    /// Content-hash key for a chunk under this store's provider + model.
    fn cache_key_for(&self, chunk_text: &str) -> String {
        let provider = self.config.provider.map(|p| p.as_str()).unwrap_or("");
        embed_cache::cache_key(provider, &self.config.model, chunk_text)
    }

    pub async fn entry_count(&self) -> usize {
        self.entries.read().await.len()
    }
}

/// `ChunkStrategy: Copy` is intentional; this helper exists so the call
/// site reads as "clone or default" rather than "deref then copy".
trait ChunkStrategyExt {
    fn clone_or_default(&self) -> ChunkStrategy;
}

impl ChunkStrategyExt for ChunkStrategy {
    fn clone_or_default(&self) -> ChunkStrategy {
        *self
    }
}

/// Per-workspace store registry. The daemon holds one of these; the
/// control socket's `query_canonical_specs` handler looks up the store
/// for the requesting workspace.
#[derive(Default, Clone)]
pub struct CanonicalRagRegistry {
    inner: Arc<RwLock<HashMap<String, Arc<CanonicalRagStore>>>>,
}

impl CanonicalRagRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register(&self, basename: String, store: Arc<CanonicalRagStore>) {
        let mut guard = self.inner.write().await;
        guard.insert(basename, store);
    }

    #[allow(dead_code)]
    pub async fn remove(&self, basename: &str) {
        let mut guard = self.inner.write().await;
        guard.remove(basename);
    }

    pub async fn get(&self, basename: &str) -> Option<Arc<CanonicalRagStore>> {
        let guard = self.inner.read().await;
        guard.get(basename).cloned()
    }

    pub async fn contains(&self, basename: &str) -> bool {
        let guard = self.inner.read().await;
        guard.contains_key(basename)
    }

    #[allow(dead_code)]
    pub async fn len(&self) -> usize {
        let guard = self.inner.read().await;
        guard.len()
    }
}

/// Cosine similarity between two equal-length vectors. Returns `0.0`
/// for mismatched dimensions OR zero-norm vectors (a defensive
/// not-a-number guard — the provider should never return either, but
/// silent NaN propagation would break the top-k sort).
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom == 0.0 { 0.0 } else { dot / denom }
}

/// Compute the sanitized workspace basename used as the registry key.
/// Matches the per-workspace path resolution: the file-name component
/// of the workspace path.
pub fn sanitize_workspace_basename(workspace: &Path) -> String {
    workspace
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| "unknown_workspace".to_string())
}

fn discover_canonical_specs(workspace: &Path) -> Result<Vec<PathBuf>> {
    let specs_root = workspace.join("openspec/specs");
    if !specs_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&specs_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let spec_path = entry.path().join("spec.md");
        if spec_within_root(&specs_root, &spec_path) {
            out.push(spec_path);
        }
    }
    out.sort();
    Ok(out)
}

/// Accept `spec_path` for indexing only if it is a regular file that,
/// after fully resolving symlinks, still lives inside `specs_root`.
///
/// The daemon reads these files unsandboxed with owner privileges and
/// hands their contents back to the sandboxed agent via the
/// `query_canonical_specs` control action. A committed `spec.md` (or an
/// intermediate `<cap>/` directory) that is a symlink to an absolute
/// host path — `/etc/passwd`, `~/.ssh/id_ed25519`, a secrets file — must
/// therefore never be followed. `canonicalize()` resolves the whole
/// chain, and the `starts_with` containment check rejects any target
/// that escapes the managed spec tree. Mirrors the no-follow discipline
/// in `workspace_cache::dir_size`. Canonicalizing the root too handles a
/// workspace that is itself reached through a symlink.
fn spec_within_root(specs_root: &Path, spec_path: &Path) -> bool {
    let (Ok(real), Ok(root)) = (spec_path.canonicalize(), specs_root.canonicalize()) else {
        return false;
    };
    real.is_file() && real.starts_with(&root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CanonicalRagConfig, ChunkStrategy, RagProvider};
    use async_trait::async_trait;
    use tempfile::TempDir;

    fn config_for_tests() -> CanonicalRagConfig {
        CanonicalRagConfig {
            enabled: true,
            provider: Some(RagProvider::Ollama),
            model: "nomic-embed-text".into(),
            api_base_url: "http://localhost:11434".into(),
            api_key_env: None,
            api_key: None,
            top_k: 10,
            chunk_strategy: ChunkStrategy::PerRequirement,
            reembed_on_archive: true,
        }
    }

    fn write_spec(workspace: &Path, capability: &str, body: &str) {
        let dir = workspace.join("openspec/specs").join(capability);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("spec.md"), body).unwrap();
    }

    /// Test client: maps a chunk's first non-empty word to a one-hot
    /// embedding so cosine similarity is predictable.
    struct WordMatchClient;

    #[async_trait]
    impl EmbedClient for WordMatchClient {
        async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|t| Self::embed_text(t)).collect())
        }
    }
    impl WordMatchClient {
        fn embed_text(text: &str) -> Vec<f32> {
            // Map the first non-heading word to one of three slots so
            // queries match deterministically.
            let lower = text.to_ascii_lowercase();
            let hit_audit = lower.contains("audit");
            let mut hit_review = lower.contains("review");
            let mut hit_other = !hit_audit && !hit_review;
            // Ensure exactly one is set:
            if hit_audit && hit_review {
                hit_review = false;
            }
            if !hit_audit && !hit_review {
                hit_other = true;
            }
            let v_audit = if hit_audit { 1.0 } else { 0.0 };
            let v_review = if hit_review { 1.0 } else { 0.0 };
            let v_other = if hit_other { 1.0 } else { 0.0 };
            vec![v_audit, v_review, v_other]
        }
    }

    async fn build_store(workspace: &Path) -> CanonicalRagStore {
        let provider: Arc<dyn EmbedClient> = Arc::new(WordMatchClient);
        let store = CanonicalRagStore {
            workspace_basename: sanitize_workspace_basename(workspace),
            provider: provider.clone(),
            config: config_for_tests(),
            entries: RwLock::new(Vec::new()),
            cache_path: None,
        };
        let paths = discover_canonical_specs(workspace).unwrap();
        store.embed_paths(&paths).await.unwrap();
        store
    }

    /// Embed client that counts how many texts it is asked to embed, so a
    /// cache test can assert "zero provider calls" / "exactly one miss".
    struct CountingClient {
        count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl EmbedClient for CountingClient {
        async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            self.count
                .fetch_add(texts.len(), std::sync::atomic::Ordering::SeqCst);
            // Content-derived vector; the exact values don't matter here.
            Ok(texts.iter().map(|t| vec![t.len() as f32, 1.0]).collect())
        }
    }

    /// Build a store backed by a [`CountingClient`] and an optional disk
    /// cache, with a caller-chosen model (to exercise model-change misses).
    async fn build_counting_store(
        workspace: &Path,
        cache_path: Option<PathBuf>,
        count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        model: &str,
    ) -> CanonicalRagStore {
        let mut config = config_for_tests();
        config.model = model.to_string();
        let provider: Arc<dyn EmbedClient> = Arc::new(CountingClient { count });
        let store = CanonicalRagStore {
            workspace_basename: sanitize_workspace_basename(workspace),
            provider,
            config,
            entries: RwLock::new(Vec::new()),
            cache_path,
        };
        let paths = discover_canonical_specs(workspace).unwrap();
        store.embed_paths(&paths).await.unwrap();
        store
    }

    #[tokio::test]
    async fn store_query_ranks_audit_chunk_first() {
        let tmp = TempDir::new().unwrap();
        write_spec(
            tmp.path(),
            "audits",
            "### Requirement: Audit cadence\nSHALL run audits on a schedule.\n",
        );
        write_spec(
            tmp.path(),
            "reviewer",
            "### Requirement: Review block verdict\nSHALL block when policy fails.\n",
        );
        write_spec(
            tmp.path(),
            "other-cap",
            "### Requirement: Other thing\nSHALL do something else.\n",
        );
        let store = build_store(tmp.path()).await;
        let hits = store.query("audit framework cadence", Some(3)).await.unwrap();
        assert_eq!(hits.len(), 3);
        // The audit chunk wins because cosine sim hits the audit slot.
        assert_eq!(hits[0].capability, "audits");
    }

    #[tokio::test]
    async fn rebuild_single_capability_leaves_others_alone() {
        let tmp = TempDir::new().unwrap();
        write_spec(
            tmp.path(),
            "audits",
            "### Requirement: Audit cadence\nSHALL run audits.\n",
        );
        write_spec(
            tmp.path(),
            "reviewer",
            "### Requirement: Review block verdict\nSHALL block.\n",
        );
        let store = build_store(tmp.path()).await;
        assert_eq!(store.entry_count().await, 2);

        // Mutate the audits spec and rebuild just that capability.
        write_spec(
            tmp.path(),
            "audits",
            "### Requirement: Audit cadence\nSHALL run audits.\n\n### Requirement: New audit type\nSHALL register new type.\n",
        );
        store
            .rebuild_capabilities(tmp.path(), &["audits".to_string()])
            .await
            .unwrap();
        let entries = store.entries.read().await;
        let audit_entries: Vec<_> = entries
            .iter()
            .filter(|e| e.input.capability == "audits")
            .collect();
        let reviewer_entries: Vec<_> = entries
            .iter()
            .filter(|e| e.input.capability == "reviewer")
            .collect();
        assert_eq!(audit_entries.len(), 2);
        assert_eq!(reviewer_entries.len(), 1);
    }

    #[tokio::test]
    async fn empty_workspace_yields_empty_store() {
        let tmp = TempDir::new().unwrap();
        // No openspec/specs/ at all.
        let store = build_store(tmp.path()).await;
        assert_eq!(store.entry_count().await, 0);
        let hits = store.query("anything", Some(5)).await.unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn registry_routes_per_workspace() {
        let registry = CanonicalRagRegistry::new();
        let tmp_a = TempDir::new().unwrap();
        let tmp_b = TempDir::new().unwrap();
        // Build two stores with one chunk each (distinct content).
        write_spec(
            tmp_a.path(),
            "audits",
            "### Requirement: Audit cadence\nSHALL.\n",
        );
        write_spec(
            tmp_b.path(),
            "reviewer",
            "### Requirement: Review verdict\nSHALL.\n",
        );
        let a = Arc::new(build_store(tmp_a.path()).await);
        let b = Arc::new(build_store(tmp_b.path()).await);
        let basename_a = sanitize_workspace_basename(tmp_a.path());
        let basename_b = sanitize_workspace_basename(tmp_b.path());
        registry.register(basename_a.clone(), a.clone()).await;
        registry.register(basename_b.clone(), b.clone()).await;
        assert!(registry.contains(&basename_a).await);
        assert!(registry.contains(&basename_b).await);
        let got_a = registry.get(&basename_a).await.unwrap();
        assert_eq!(got_a.entry_count().await, 1);
        let got_b = registry.get(&basename_b).await.unwrap();
        assert_eq!(got_b.entry_count().await, 1);
        let nope = registry.get("never-registered").await;
        assert!(nope.is_none());
    }

    /// Security regression (issues lane): a `spec.md` committed as a
    /// symlink must never be followed, so the daemon cannot read a
    /// symlink target — an in-repo file OR an absolute out-of-tree host
    /// path — and leak its bytes to the sandboxed agent via
    /// `query_canonical_specs`. A regular `spec.md` alongside it is still
    /// indexed (no regression). The symlink targets carry a real
    /// `### Requirement:` heading so that, were they followed, they would
    /// definitely produce indexable chunks — the assertions below prove
    /// they do not.
    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_spec_md_is_skipped_regular_still_indexed() {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new().unwrap();

        // (a) spec.md -> an in-repo secret file.
        let secret_in_repo = tmp.path().join("secret-in-repo.md");
        std::fs::write(
            &secret_in_repo,
            "### Requirement: Stolen\nIN_REPO_SECRET_TOKEN\n",
        )
        .unwrap();
        let evil_a = tmp.path().join("openspec/specs/evil-a");
        std::fs::create_dir_all(&evil_a).unwrap();
        symlink(&secret_in_repo, evil_a.join("spec.md")).unwrap();

        // (b) spec.md -> an absolute out-of-tree host path.
        let outside = TempDir::new().unwrap();
        let secret_host = outside.path().join("host-secret.env");
        std::fs::write(
            &secret_host,
            "### Requirement: Stolen\nHOST_SECRET_PASSWORD\n",
        )
        .unwrap();
        let evil_b = tmp.path().join("openspec/specs/evil-b");
        std::fs::create_dir_all(&evil_b).unwrap();
        symlink(&secret_host, evil_b.join("spec.md")).unwrap();

        // A legitimate regular spec.
        write_spec(
            tmp.path(),
            "audits",
            "### Requirement: Audit cadence\nSHALL run audits.\n",
        );

        // discover_canonical_specs must return only the regular spec.
        let discovered = discover_canonical_specs(tmp.path()).unwrap();
        assert_eq!(discovered.len(), 1, "only the regular spec is discovered");
        assert!(discovered[0].ends_with("audits/spec.md"));

        // And nothing symlinked reaches the index.
        let store = build_store(tmp.path()).await;
        assert_eq!(store.entry_count().await, 1);
        let entries = store.entries.read().await;
        for e in entries.iter() {
            assert_eq!(e.input.capability, "audits");
            assert!(!e.input.text.contains("IN_REPO_SECRET_TOKEN"));
            assert!(!e.input.text.contains("HOST_SECRET_PASSWORD"));
        }
        drop(entries);

        // rebuild_capabilities must also refuse to follow the symlink.
        store
            .rebuild_capabilities(tmp.path(), &["evil-a".to_string(), "evil-b".to_string()])
            .await
            .unwrap();
        assert_eq!(store.entry_count().await, 1);
        let entries = store.entries.read().await;
        for e in entries.iter() {
            assert_eq!(e.input.capability, "audits");
        }
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 3.1: a second build of an unchanged corpus makes ZERO provider calls.
    #[tokio::test]
    async fn warm_build_of_unchanged_corpus_makes_zero_provider_calls() {
        let tmp = TempDir::new().unwrap();
        write_spec(tmp.path(), "audits", "### Requirement: A\nSHALL a.\n");
        write_spec(tmp.path(), "reviewer", "### Requirement: R\nSHALL r.\n");
        let cache_dir = TempDir::new().unwrap();
        let cache_path = cache_dir.path().join("ws.json");

        let counter = Arc::new(AtomicUsize::new(0));
        let cold =
            build_counting_store(tmp.path(), Some(cache_path.clone()), counter.clone(), "m").await;
        assert_eq!(cold.entry_count().await, 2);
        assert_eq!(counter.load(Ordering::SeqCst), 2, "cold build embeds every chunk");

        counter.store(0, Ordering::SeqCst);
        let warm =
            build_counting_store(tmp.path(), Some(cache_path.clone()), counter.clone(), "m").await;
        assert_eq!(warm.entry_count().await, 2);
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "warm build loads every vector from the cache"
        );
    }

    /// 3.1: an edited chunk causes EXACTLY one provider call; the rest hit.
    #[tokio::test]
    async fn edited_chunk_causes_exactly_one_provider_call() {
        let tmp = TempDir::new().unwrap();
        write_spec(tmp.path(), "audits", "### Requirement: A\nSHALL a.\n");
        write_spec(tmp.path(), "reviewer", "### Requirement: R\nSHALL r.\n");
        let cache_dir = TempDir::new().unwrap();
        let cache_path = cache_dir.path().join("ws.json");
        let counter = Arc::new(AtomicUsize::new(0));
        build_counting_store(tmp.path(), Some(cache_path.clone()), counter.clone(), "m").await;

        // Edit exactly ONE chunk's text.
        write_spec(tmp.path(), "reviewer", "### Requirement: R\nSHALL r CHANGED.\n");
        counter.store(0, Ordering::SeqCst);
        build_counting_store(tmp.path(), Some(cache_path.clone()), counter.clone(), "m").await;
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "only the edited chunk misses; the unchanged chunk still hits"
        );
    }

    /// 3.1: a model change (part of the key) misses the entire cache.
    #[tokio::test]
    async fn model_change_misses_entire_cache() {
        let tmp = TempDir::new().unwrap();
        write_spec(tmp.path(), "audits", "### Requirement: A\nSHALL a.\n");
        write_spec(tmp.path(), "reviewer", "### Requirement: R\nSHALL r.\n");
        let cache_dir = TempDir::new().unwrap();
        let cache_path = cache_dir.path().join("ws.json");
        let counter = Arc::new(AtomicUsize::new(0));
        build_counting_store(tmp.path(), Some(cache_path.clone()), counter.clone(), "model-a").await;

        // Same corpus + cache file, DIFFERENT model → every key differs.
        counter.store(0, Ordering::SeqCst);
        build_counting_store(tmp.path(), Some(cache_path.clone()), counter.clone(), "model-b").await;
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "a model change re-embeds the whole corpus"
        );
    }

    /// 3.2: write-through retains only live keys — a removed chunk's entry
    /// disappears from the cache after a partial rebuild.
    #[tokio::test]
    async fn write_through_prunes_removed_chunk_key() {
        use std::collections::HashSet;
        let tmp = TempDir::new().unwrap();
        write_spec(tmp.path(), "audits", "### Requirement: A\nSHALL a.\n");
        write_spec(tmp.path(), "reviewer", "### Requirement: R\nSHALL r.\n");
        let cache_dir = TempDir::new().unwrap();
        let cache_path = cache_dir.path().join("ws.json");
        let counter = Arc::new(AtomicUsize::new(0));
        let store =
            build_counting_store(tmp.path(), Some(cache_path.clone()), counter.clone(), "m").await;
        assert_eq!(store.entry_count().await, 2);
        let before: HashSet<String> = embed_cache::load(&cache_path).into_keys().collect();
        assert_eq!(before.len(), 2);

        // Remove the audits capability entirely, then rebuild it (the
        // post-archive path). Its spec.md is gone → the store drops it AND
        // the write-through prunes its key; the untouched reviewer
        // capability's vector is retained.
        std::fs::remove_file(tmp.path().join("openspec/specs/audits/spec.md")).unwrap();
        store
            .rebuild_capabilities(tmp.path(), &["audits".to_string()])
            .await
            .unwrap();
        assert_eq!(store.entry_count().await, 1);

        let after: HashSet<String> = embed_cache::load(&cache_path).into_keys().collect();
        assert_eq!(after.len(), 1, "cache retains exactly the live key");
        assert!(after.is_subset(&before), "keys are pruned, never re-minted");
        let dropped: Vec<_> = before.difference(&after).collect();
        assert_eq!(dropped.len(), 1, "exactly the removed capability's key disappeared");
    }

    /// 3.2: a corrupt cache file degrades to a full embed and is rewritten
    /// healthy (so the NEXT build is a full cache hit).
    #[tokio::test]
    async fn corrupt_cache_forces_full_embed_then_rewrites_healthy() {
        let tmp = TempDir::new().unwrap();
        write_spec(tmp.path(), "audits", "### Requirement: A\nSHALL a.\n");
        write_spec(tmp.path(), "reviewer", "### Requirement: R\nSHALL r.\n");
        let cache_dir = TempDir::new().unwrap();
        let cache_path = cache_dir.path().join("ws.json");
        std::fs::write(&cache_path, b"{ this is not valid json").unwrap();

        let counter = Arc::new(AtomicUsize::new(0));
        let store =
            build_counting_store(tmp.path(), Some(cache_path.clone()), counter.clone(), "m").await;
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "a corrupt cache degrades to a full provider embed"
        );
        // The write-through replaced the corrupt file with a valid one.
        assert_eq!(embed_cache::load(&cache_path).len(), store.entry_count().await);

        counter.store(0, Ordering::SeqCst);
        build_counting_store(tmp.path(), Some(cache_path.clone()), counter.clone(), "m").await;
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "the healthy rewrite makes the next build a full cache hit"
        );
    }

    /// 3.2: a missing cache directory is created lazily on first write.
    #[tokio::test]
    async fn missing_cache_directory_is_created_on_write() {
        let tmp = TempDir::new().unwrap();
        write_spec(tmp.path(), "audits", "### Requirement: A\nSHALL a.\n");
        let cache_root = TempDir::new().unwrap();
        let cache_path = cache_root.path().join("rag-embeddings/nested/ws.json");
        assert!(!cache_path.parent().unwrap().exists());
        let counter = Arc::new(AtomicUsize::new(0));
        build_counting_store(tmp.path(), Some(cache_path.clone()), counter.clone(), "m").await;
        assert!(
            cache_path.exists(),
            "write_through created the missing directory chain + file"
        );
    }

    #[test]
    fn cosine_similarity_handles_edge_cases() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 0.0]), 0.0);
        let a = [1.0f32, 0.0];
        let b = [1.0f32, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
        let c = [0.0f32, 1.0];
        assert!(cosine_similarity(&a, &c).abs() < 1e-6);
    }
}
