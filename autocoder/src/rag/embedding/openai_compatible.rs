//! OpenAI-compatible embedding adapter (a21).
//!
//! POSTs to `<base_url>/embeddings` with `{"model": "<model>", "input":
//! [...]}` and `Authorization: Bearer <api_key>`. Parses the standard
//! OpenAI embeddings response format (`{ data: [{ embedding: [...] }, ... ] }`)
//! into `Vec<Vec<f32>>`.

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;

use super::EmbedClient;

const BATCH_SIZE: usize = 32;

pub struct OpenAiCompatEmbedClient {
    base_url: String,
    model: String,
    api_key: String,
    client: reqwest::Client,
}

impl OpenAiCompatEmbedClient {
    pub fn new(base_url: String, model: String, api_key: String) -> Self {
        Self {
            base_url,
            model,
            api_key,
            client: reqwest::Client::new(),
        }
    }
}

#[derive(Deserialize)]
struct OpenAiEmbedResponse {
    data: Vec<OpenAiEmbedItem>,
}

#[derive(Deserialize)]
struct OpenAiEmbedItem {
    embedding: Vec<f32>,
}

#[async_trait]
impl EmbedClient for OpenAiCompatEmbedClient {
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/embeddings", self.base_url.trim_end_matches('/'));
        let mut out = Vec::with_capacity(texts.len());
        for batch in texts.chunks(BATCH_SIZE) {
            let body = serde_json::json!({
                "model": self.model,
                "input": batch,
            });
            let resp = self
                .client
                .post(&url)
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await
                .map_err(|e| anyhow!("openai-compat embed request to {url}: {e}"))?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(anyhow!(
                    "openai-compat embed {url} returned HTTP {status}: {body}"
                ));
            }
            let parsed: OpenAiEmbedResponse = resp
                .json()
                .await
                .map_err(|e| anyhow!("openai-compat embed response decode: {e}"))?;
            if parsed.data.len() != batch.len() {
                return Err(anyhow!(
                    "openai-compat embed returned {} embeddings for {} inputs",
                    parsed.data.len(),
                    batch.len()
                ));
            }
            out.extend(parsed.data.into_iter().map(|item| item.embedding));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn embed_batch_parses_openai_response() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/embeddings")
            .match_header("authorization", "Bearer test-key")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"data":[{"embedding":[0.1,0.2]},{"embedding":[0.3,0.4]}]}"#,
            )
            .create_async()
            .await;
        let client = OpenAiCompatEmbedClient::new(server.url(), "voyage-2".into(), "test-key".into());
        let out = client
            .embed_batch(&["a".to_string(), "b".to_string()])
            .await
            .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], vec![0.1, 0.2]);
        assert_eq!(out[1], vec![0.3, 0.4]);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn embed_batch_4xx_returns_error_with_body() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/embeddings")
            .with_status(401)
            .with_body("unauthorized")
            .create_async()
            .await;
        let client = OpenAiCompatEmbedClient::new(server.url(), "x".into(), "bad".into());
        let err = client
            .embed_batch(&["a".to_string()])
            .await
            .expect_err("401 should propagate");
        let msg = format!("{err:#}");
        assert!(msg.contains("401"), "msg should name status: {msg}");
    }

    #[tokio::test]
    async fn embed_batch_empty_input_returns_empty() {
        let server = mockito::Server::new_async().await;
        let client = OpenAiCompatEmbedClient::new(server.url(), "x".into(), "k".into());
        let out = client.embed_batch(&[]).await.unwrap();
        assert!(out.is_empty());
    }
}
