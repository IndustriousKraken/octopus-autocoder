//! Ollama embedding adapter (a21).
//!
//! POSTs to `<base_url>/api/embed` with `{"model": "<model>", "input":
//! [...]}` and parses the documented Ollama embeddings response into
//! `Vec<Vec<f32>>`.

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;

use super::EmbedClient;

/// Batch ceiling per the canonical orchestrator spec. Larger inputs are
/// split into multiple requests before hitting the wire.
const BATCH_SIZE: usize = 32;

pub struct OllamaEmbedClient {
    base_url: String,
    model: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl OllamaEmbedClient {
    pub fn new(base_url: String, model: String, api_key: Option<String>) -> Self {
        Self {
            base_url,
            model,
            api_key,
            client: reqwest::Client::new(),
        }
    }
}

#[derive(Deserialize)]
struct OllamaEmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

#[async_trait]
impl EmbedClient for OllamaEmbedClient {
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/api/embed", self.base_url.trim_end_matches('/'));
        let mut out = Vec::with_capacity(texts.len());
        for batch in texts.chunks(BATCH_SIZE) {
            let body = serde_json::json!({
                "model": self.model,
                "input": batch,
            });
            let mut req = self.client.post(&url).json(&body);
            if let Some(key) = self.api_key.as_deref() {
                req = req.bearer_auth(key);
            }
            let resp = req.send().await.map_err(|e| {
                anyhow!("ollama embed request to {url}: {e}")
            })?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(anyhow!(
                    "ollama embed {url} returned HTTP {status}: {body}"
                ));
            }
            let parsed: OllamaEmbedResponse = resp.json().await.map_err(|e| {
                anyhow!("ollama embed response decode: {e}")
            })?;
            if parsed.embeddings.len() != batch.len() {
                return Err(anyhow!(
                    "ollama embed returned {} embeddings for {} inputs",
                    parsed.embeddings.len(),
                    batch.len()
                ));
            }
            out.extend(parsed.embeddings);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn embed_batch_parses_ollama_response() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/embed")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"embeddings":[[0.1,0.2,0.3],[0.4,0.5,0.6]]}"#,
            )
            .create_async()
            .await;
        let client = OllamaEmbedClient::new(server.url(), "nomic-embed-text".into(), None);
        let out = client
            .embed_batch(&["a".to_string(), "b".to_string()])
            .await
            .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], vec![0.1, 0.2, 0.3]);
        assert_eq!(out[1], vec![0.4, 0.5, 0.6]);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn embed_batch_propagates_5xx() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/api/embed")
            .with_status(500)
            .with_body("boom")
            .create_async()
            .await;
        let client = OllamaEmbedClient::new(server.url(), "x".into(), None);
        let err = client
            .embed_batch(&["a".to_string()])
            .await
            .expect_err("5xx should propagate");
        let msg = format!("{err:#}");
        assert!(msg.contains("500"), "msg should name status: {msg}");
    }

    #[tokio::test]
    async fn embed_batch_chunks_inputs_above_batch_size() {
        let mut server = mockito::Server::new_async().await;
        let body = format!(
            r#"{{"embeddings":[{}]}}"#,
            std::iter::repeat_n("[1.0]", BATCH_SIZE)
                .collect::<Vec<_>>()
                .join(",")
        );
        let mock = server
            .mock("POST", "/api/embed")
            .with_status(200)
            .with_body(body)
            .expect_at_least(2)
            .create_async()
            .await;
        let client = OllamaEmbedClient::new(server.url(), "x".into(), None);
        let inputs: Vec<String> = (0..BATCH_SIZE + 1).map(|i| format!("t{i}")).collect();
        // Two batches: one full (32) one with the leftover (1). The mock
        // returns a 32-vec for both so we expect a length mismatch on
        // the second batch — proves that the chunking actually issued
        // a second request.
        let err = client.embed_batch(&inputs).await.expect_err(
            "expected a length-mismatch error proving the second batch was issued",
        );
        let msg = format!("{err:#}");
        assert!(
            msg.contains("embeddings for") && msg.contains("inputs"),
            "expected length-mismatch error; got {msg}"
        );
        mock.assert_async().await;
    }
}
