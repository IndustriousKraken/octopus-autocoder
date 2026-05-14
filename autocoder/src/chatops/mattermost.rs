//! Mattermost ChatOps backend (EXPERIMENTAL).
//!
//! `/api/v4/posts` for sending and `/api/v4/posts/{id}/thread` for polling.
//! `Authorization: Bearer <token>` PAT auth.

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashMap;

use super::{ChatOpsBackend, HumanReply};

pub struct MattermostBackend {
    client: reqwest::Client,
    server_url: String,
    access_token: String,
    bot_user_id: String,
}

impl MattermostBackend {
    pub async fn new(server_url: String, access_token: String) -> Result<Self> {
        let client = reqwest::Client::new();
        let url = format!("{}/api/v4/users/me", server_url.trim_end_matches('/'));
        let resp = client
            .get(&url)
            .header("Authorization", format!("Bearer {access_token}"))
            .send()
            .await
            .map_err(|e| anyhow!("mattermost users/me request failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "mattermost users/me identity call failed: http {status}: {body}"
            ));
        }
        let parsed: SelfUser = resp
            .json()
            .await
            .map_err(|e| anyhow!("mattermost users/me decode failed: {e}"))?;
        Ok(Self {
            client,
            server_url,
            access_token,
            bot_user_id: parsed.id,
        })
    }

    pub fn bot_user_id(&self) -> &str {
        &self.bot_user_id
    }
}

#[async_trait]
impl ChatOpsBackend for MattermostBackend {
    fn provider_name(&self) -> &'static str {
        "mattermost"
    }

    fn is_experimental(&self) -> bool {
        true
    }

    async fn post_question(
        &self,
        channel: &str,
        change: &str,
        question: &str,
    ) -> Result<String> {
        let url = format!("{}/api/v4/posts", self.server_url.trim_end_matches('/'));
        let message = format!("❓ `{change}`: {question}");
        let payload = serde_json::json!({
            "channel_id": channel,
            "message": message,
        });
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| anyhow!("mattermost post request failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("mattermost post http {status}: {body}"));
        }
        let parsed: PostedMessage = resp
            .json()
            .await
            .map_err(|e| anyhow!("mattermost post decode failed: {e}"))?;
        Ok(parsed.id)
    }

    async fn poll_thread_for_human_reply(
        &self,
        _channel: &str,
        handle: &str,
    ) -> Result<Option<HumanReply>> {
        let url = format!(
            "{}/api/v4/posts/{handle}/thread",
            self.server_url.trim_end_matches('/')
        );
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .send()
            .await
            .map_err(|e| anyhow!("mattermost thread request failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("mattermost thread http {status}: {body}"));
        }
        let parsed: ThreadResponse = resp
            .json()
            .await
            .map_err(|e| anyhow!("mattermost thread decode failed: {e}"))?;

        let mut replies: Vec<MattermostPost> = parsed
            .posts
            .into_values()
            .filter(|p| {
                p.root_id.as_deref() == Some(handle)
                    && p.user_id.as_deref() != Some(self.bot_user_id.as_str())
            })
            .collect();
        replies.sort_by(|a, b| a.create_at.cmp(&b.create_at));
        Ok(replies.into_iter().next().map(|p| HumanReply {
            text: p.message,
            user_id: p.user_id.unwrap_or_default(),
            ts: p.id,
        }))
    }

    async fn post_notification(&self, channel: &str, text: &str) -> Result<()> {
        let url = format!("{}/api/v4/posts", self.server_url.trim_end_matches('/'));
        let payload = serde_json::json!({
            "channel_id": channel,
            "message": text,
        });
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| anyhow!("mattermost notification request failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("mattermost notification http {status}: {body}"));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct SelfUser {
    id: String,
}

#[derive(Deserialize)]
struct PostedMessage {
    id: String,
}

#[derive(Deserialize)]
struct ThreadResponse {
    #[serde(default)]
    posts: HashMap<String, MattermostPost>,
}

#[derive(Deserialize)]
struct MattermostPost {
    id: String,
    #[serde(default)]
    create_at: u64,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    root_id: Option<String>,
    #[serde(default)]
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture_backend(server: &mut mockito::Server) -> MattermostBackend {
        let _identity = server
            .mock("GET", "/api/v4/users/me")
            .with_status(200)
            .with_body(r#"{"id":"BOT_MM"}"#)
            .create_async()
            .await;
        MattermostBackend::new(server.url(), "mm-token".into())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn provider_metadata_mattermost() {
        let mut server = mockito::Server::new_async().await;
        let backend = fixture_backend(&mut server).await;
        assert_eq!(backend.provider_name(), "mattermost");
        assert!(backend.is_experimental());
    }

    #[tokio::test]
    async fn posts_to_v4_posts_endpoint() {
        let mut server = mockito::Server::new_async().await;
        let backend = fixture_backend(&mut server).await;

        let post_mock = server
            .mock("POST", "/api/v4/posts")
            .match_header("authorization", "Bearer mm-token")
            .match_body(mockito::Matcher::JsonString(
                r#"{"channel_id":"CH-X","message":"❓ `feat-x`: pick?"}"#.to_string(),
            ))
            .with_status(201)
            .with_body(r#"{"id":"POST123"}"#)
            .create_async()
            .await;

        let id = backend.post_question("CH-X", "feat-x", "pick?").await.unwrap();
        assert_eq!(id, "POST123");
        post_mock.assert_async().await;
    }

    #[tokio::test]
    async fn polls_thread_filters_bot_self() {
        let mut server = mockito::Server::new_async().await;
        let backend = fixture_backend(&mut server).await;

        let _thread = server
            .mock("GET", "/api/v4/posts/POST123/thread")
            .with_status(200)
            .with_body(
                r#"{"posts":{
                    "POST123":{"id":"POST123","create_at":100,"user_id":"BOT_MM","root_id":"","message":"❓ ..."},
                    "POSTBOT":{"id":"POSTBOT","create_at":110,"user_id":"BOT_MM","root_id":"POST123","message":"bot follow-up"},
                    "POSTHUM":{"id":"POSTHUM","create_at":120,"user_id":"USER77","root_id":"POST123","message":"use SAMPLE"}
                }}"#,
            )
            .create_async()
            .await;

        let reply = backend
            .poll_thread_for_human_reply("CH-X", "POST123")
            .await
            .unwrap()
            .expect("human reply");
        assert_eq!(reply.text, "use SAMPLE");
        assert_eq!(reply.user_id, "USER77");
        assert_eq!(reply.ts, "POSTHUM");
    }
}
