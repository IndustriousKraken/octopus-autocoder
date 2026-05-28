//! Embedding provider trait + builder for the canonical-spec RAG
//! pipeline (a21). Two adapters today:
//! - [`ollama::OllamaEmbedClient`] POSTs to `<base_url>/api/embed`.
//! - [`openai_compatible::OpenAiCompatEmbedClient`] POSTs to
//!   `<base_url>/embeddings` with a Bearer token.
//!
//! Both implement [`EmbedClient`]. The provider trait is `async_trait`
//! so adapters can use plain reqwest without locking the call site
//! into any specific HTTP runtime detail.

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::sync::Arc;

use crate::config::{CanonicalRagConfig, RagProvider};

pub mod ollama;
pub mod openai_compatible;

#[async_trait]
pub trait EmbedClient: Send + Sync {
    /// Embed a batch of texts. Implementations SHOULD respect their
    /// provider's batch limit; the canonical batch ceiling is 32 per
    /// the orchestrator spec, but adapters may choose smaller batches
    /// internally so long as `texts.len() == returned.len()` and the
    /// ordering is preserved.
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// Embed a single text. Default implementation wraps `embed_batch`.
    async fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let mut out = self.embed_batch(&[text.to_string()]).await?;
        out.pop()
            .ok_or_else(|| anyhow!("embed_one: provider returned empty vec"))
    }
}

/// Construct an embed client from the canonical-RAG config. Resolves
/// API keys per the documented `inline > env_var` precedence with WARN
/// when both are set (the `resolve_api_key` impl logs).
pub fn build_client(config: &CanonicalRagConfig) -> Result<Arc<dyn EmbedClient>> {
    match config.provider {
        RagProvider::Ollama => {
            let api_key = config.resolve_api_key().ok().flatten();
            Ok(Arc::new(ollama::OllamaEmbedClient::new(
                config.api_base_url.clone(),
                config.model.clone(),
                api_key,
            )))
        }
        RagProvider::OpenaiCompatible => {
            let api_key = config
                .resolve_api_key()?
                .ok_or_else(|| anyhow!(
                    "canonical_rag.provider=openai_compatible requires api_key OR api_key_env"
                ))?;
            Ok(Arc::new(openai_compatible::OpenAiCompatEmbedClient::new(
                config.api_base_url.clone(),
                config.model.clone(),
                api_key,
            )))
        }
    }
}
