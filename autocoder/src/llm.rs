//! LLM client abstraction. The code-reviewer module is the only caller; this
//! file isolates HTTP details from review semantics and supports multiple
//! providers behind one trait so users can pick Claude, GPT, Grok, Ollama,
//! or any OpenAI-compatible endpoint.

use crate::config::{ReviewerConfig, ReviewerProvider};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

const DEFAULT_ANTHROPIC_BASE: &str = "https://api.anthropic.com";
const DEFAULT_OPENAI_BASE: &str = "https://api.openai.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 4096;

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(&self, prompt: &str) -> Result<String>;
}

pub struct AnthropicClient {
    api_base: String,
    api_key: String,
    model: String,
}

impl AnthropicClient {
    pub fn new(api_base: String, api_key: String, model: String) -> Self {
        Self { api_base, api_key, model }
    }
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContentBlock>,
}

#[derive(Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    text: Option<String>,
}

#[async_trait]
impl LlmClient for AnthropicClient {
    async fn complete(&self, prompt: &str) -> Result<String> {
        let url = format!("{}/v1/messages", self.api_base.trim_end_matches('/'));
        let payload = json!({
            "model": self.model,
            "max_tokens": DEFAULT_MAX_TOKENS,
            "messages": [{
                "role": "user",
                "content": prompt,
            }],
        });
        let resp = reqwest::Client::new()
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| anyhow!("anthropic request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(500).collect();
            return Err(anyhow!("anthropic API error {status}: {snippet}"));
        }
        let parsed: AnthropicResponse = resp
            .json()
            .await
            .map_err(|e| anyhow!("anthropic response decode failed: {e}"))?;
        for block in parsed.content {
            if block.block_type == "text"
                && let Some(text) = block.text
            {
                return Ok(text);
            }
        }
        Err(anyhow!("anthropic response contained no text block"))
    }
}

pub struct OpenAiCompatibleClient {
    api_base: String,
    api_key: String,
    model: String,
}

impl OpenAiCompatibleClient {
    pub fn new(api_base: String, api_key: String, model: String) -> Self {
        Self { api_base, api_key, model }
    }
}

#[derive(Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
}

#[derive(Deserialize)]
struct OpenAiMessage {
    content: String,
}

#[async_trait]
impl LlmClient for OpenAiCompatibleClient {
    async fn complete(&self, prompt: &str) -> Result<String> {
        let url = format!(
            "{}/chat/completions",
            self.api_base.trim_end_matches('/')
        );
        let payload = json!({
            "model": self.model,
            "messages": [{
                "role": "user",
                "content": prompt,
            }],
        });
        let resp = reqwest::Client::new()
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| anyhow!("openai-compatible request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(500).collect();
            return Err(anyhow!("openai-compatible API error {status}: {snippet}"));
        }
        let parsed: OpenAiResponse = resp
            .json()
            .await
            .map_err(|e| anyhow!("openai-compatible response decode failed: {e}"))?;
        parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| anyhow!("openai-compatible response contained no choices"))
    }
}

/// Construct the right `LlmClient` for the configured provider. Reads the
/// API key from the environment variable named by `cfg.api_key_env`.
pub fn build_from_config(cfg: &ReviewerConfig) -> Result<Box<dyn LlmClient>> {
    let api_key = if let Some(inline) = cfg.api_key.as_ref() {
        let key = inline.resolve("reviewer.api_key")?;
        if std::env::var(&cfg.api_key_env).is_ok() && inline.is_inline() {
            tracing::warn!(
                "reviewer.api_key (inline) takes precedence; env var `{}` is being ignored for the reviewer key",
                cfg.api_key_env
            );
        }
        key
    } else {
        crate::config::SecretSource::EnvVar(cfg.api_key_env.clone())
            .resolve(&format!("reviewer.api_key_env={}", cfg.api_key_env))?
    };
    let provider = cfg.provider;
    let model = cfg.model.clone();
    let base = cfg.api_base_url.clone();

    Ok(match provider {
        ReviewerProvider::Anthropic => Box::new(AnthropicClient::new(
            base.unwrap_or_else(|| DEFAULT_ANTHROPIC_BASE.to_string()),
            api_key,
            model,
        )),
        ReviewerProvider::OpenAiCompatible => Box::new(OpenAiCompatibleClient::new(
            base.unwrap_or_else(|| DEFAULT_OPENAI_BASE.to_string()),
            api_key,
            model,
        )),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `build_from_config` MUST use `reviewer.api_key` (inline) verbatim and
    /// SHOULD NOT touch `reviewer.api_key_env`'s env var even if it happens
    /// to be set. Asserted by checking the bearer/api-key header on the
    /// outgoing request matches the inline value.
    #[tokio::test]
    async fn inline_api_key_takes_precedence_over_env_var() {
        use crate::config::{ReviewerConfig, ReviewerProvider, SecretSource};

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .match_header("x-api-key", "inline-key-wins")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"content":[{"type":"text","text":"ok"}]}"#)
            .create_async()
            .await;

        // Set the env-var pointed to by api_key_env so we can confirm it's
        // ignored — if precedence were wrong, the request would carry the
        // env value and mockito would 501 the request shape.
        unsafe {
            std::env::set_var("AUTOCODER_TEST_INLINE_PREC_KEY", "env-value-must-not-be-sent")
        };
        let cfg = ReviewerConfig {
            enabled: true,
            provider: ReviewerProvider::Anthropic,
            model: "claude-sonnet-4-6".into(),
            api_key_env: "AUTOCODER_TEST_INLINE_PREC_KEY".into(),
            api_key: Some(SecretSource::Inline {
                value: "inline-key-wins".into(),
            }),
            api_base_url: Some(server.url()),
            prompt_template_path: None,
        };
        let client = build_from_config(&cfg).expect("inline build should succeed");
        let _ = client.complete("hi").await.expect("complete succeeds");
        mock.assert_async().await;
        unsafe { std::env::remove_var("AUTOCODER_TEST_INLINE_PREC_KEY") };
    }

    #[tokio::test]
    async fn anthropic_serializes_request_and_parses_response() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .match_header("x-api-key", "testkey")
            .match_header("anthropic-version", ANTHROPIC_VERSION)
            .match_body(mockito::Matcher::JsonString(
                r#"{"model":"claude-sonnet-4-6","max_tokens":4096,"messages":[{"role":"user","content":"hi"}]}"#
                    .to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"content":[{"type":"text","text":"hello back"}]}"#)
            .create_async()
            .await;

        let client = AnthropicClient::new(
            server.url(),
            "testkey".to_string(),
            "claude-sonnet-4-6".to_string(),
        );
        let out = client.complete("hi").await.unwrap();
        assert_eq!(out, "hello back");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn anthropic_surfaces_non_2xx_with_status_and_snippet() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/v1/messages")
            .with_status(429)
            .with_body(r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#)
            .create_async()
            .await;

        let client = AnthropicClient::new(
            server.url(),
            "testkey".to_string(),
            "claude-sonnet-4-6".to_string(),
        );
        let err = client.complete("hi").await.expect_err("429 must error");
        let msg = format!("{err:#}");
        assert!(msg.contains("429"), "must include status: {msg}");
        assert!(msg.contains("rate_limit_error"), "must include body snippet: {msg}");
    }

    #[tokio::test]
    async fn openai_compatible_serializes_request_and_parses_response() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .match_header("authorization", "Bearer testkey")
            .match_body(mockito::Matcher::JsonString(
                r#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}"#.to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"choices":[{"message":{"role":"assistant","content":"hello back"}}]}"#,
            )
            .create_async()
            .await;

        let client = OpenAiCompatibleClient::new(
            server.url(),
            "testkey".to_string(),
            "gpt-4o".to_string(),
        );
        let out = client.complete("hi").await.unwrap();
        assert_eq!(out, "hello back");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn openai_compatible_surfaces_non_2xx() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/chat/completions")
            .with_status(401)
            .with_body(r#"{"error":{"message":"invalid api key"}}"#)
            .create_async()
            .await;

        let client = OpenAiCompatibleClient::new(
            server.url(),
            "testkey".to_string(),
            "gpt-4o".to_string(),
        );
        let err = client.complete("hi").await.expect_err("401 must error");
        let msg = format!("{err:#}");
        assert!(msg.contains("401"), "{msg}");
        assert!(msg.contains("invalid api key"), "{msg}");
    }
}
