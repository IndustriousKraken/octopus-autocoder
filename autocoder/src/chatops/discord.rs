//! Discord ChatOps backend (EXPERIMENTAL).
//!
//! Bot tokens via `Authorization: Bot <token>`. Reply threading uses
//! `message_reference.message_id` on subsequent channel messages.

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;

use super::{ChatOpsBackend, HumanReply};

const DEFAULT_DISCORD_BASE: &str = "https://discord.com/api/v10";

pub struct DiscordBackend {
    client: reqwest::Client,
    api_base: String,
    bot_token: String,
    bot_user_id: String,
}

impl DiscordBackend {
    pub async fn new(bot_token: String) -> Result<Self> {
        Self::new_at(DEFAULT_DISCORD_BASE.to_string(), bot_token).await
    }

    #[doc(hidden)]
    pub async fn new_at(api_base: String, bot_token: String) -> Result<Self> {
        let client = reqwest::Client::new();
        let url = format!("{}/users/@me", api_base.trim_end_matches('/'));
        let resp = client
            .get(&url)
            .header("Authorization", format!("Bot {bot_token}"))
            .send()
            .await
            .map_err(|e| anyhow!("discord users/@me request failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "discord users/@me identity call failed: http {status}: {body}"
            ));
        }
        let parsed: SelfUser = resp
            .json()
            .await
            .map_err(|e| anyhow!("discord users/@me decode failed: {e}"))?;
        Ok(Self {
            client,
            api_base,
            bot_token,
            bot_user_id: parsed.id,
        })
    }

    pub fn bot_user_id(&self) -> &str {
        &self.bot_user_id
    }
}

#[async_trait]
impl ChatOpsBackend for DiscordBackend {
    fn provider_name(&self) -> &'static str {
        "discord"
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
        let url = format!(
            "{}/channels/{channel}/messages",
            self.api_base.trim_end_matches('/')
        );
        let content = format!("❓ `{change}`: {question}");
        let payload = serde_json::json!({ "content": content });
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bot {}", self.bot_token))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| anyhow!("discord post request failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("discord post http {status}: {body}"));
        }
        let parsed: PostedMessage = resp
            .json()
            .await
            .map_err(|e| anyhow!("discord post decode failed: {e}"))?;
        Ok(parsed.id)
    }

    async fn poll_thread_for_human_reply(
        &self,
        channel: &str,
        handle: &str,
    ) -> Result<Option<HumanReply>> {
        let url = format!(
            "{}/channels/{channel}/messages?after={handle}&limit=50",
            self.api_base.trim_end_matches('/')
        );
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bot {}", self.bot_token))
            .send()
            .await
            .map_err(|e| anyhow!("discord messages request failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("discord messages http {status}: {body}"));
        }
        let messages: Vec<ChannelMessage> = resp
            .json()
            .await
            .map_err(|e| anyhow!("discord messages decode failed: {e}"))?;

        // Discord returns most-recent first when using `after`, but per the
        // docs it's actually oldest-first. Either way, find the EARLIEST
        // message that references our handle and is not from a bot. To be
        // safe we sort ascending by id (snowflake → time-ordered).
        let mut filtered: Vec<ChannelMessage> = messages
            .into_iter()
            .filter(|m| {
                m.message_reference
                    .as_ref()
                    .and_then(|r| r.message_id.as_deref())
                    == Some(handle)
                    && !m.author.bot.unwrap_or(false)
            })
            .collect();
        filtered.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(filtered.into_iter().next().map(|m| HumanReply {
            text: m.content,
            user_id: m.author.id,
            ts: m.id,
        }))
    }

    async fn post_notification(&self, channel: &str, text: &str) -> Result<()> {
        let url = format!(
            "{}/channels/{channel}/messages",
            self.api_base.trim_end_matches('/')
        );
        let payload = serde_json::json!({ "content": text });
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bot {}", self.bot_token))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| anyhow!("discord notification request failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("discord notification http {status}: {body}"));
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
struct ChannelMessage {
    id: String,
    content: String,
    author: AuthorInfo,
    #[serde(default)]
    message_reference: Option<MessageReference>,
}

#[derive(Deserialize)]
struct AuthorInfo {
    id: String,
    #[serde(default)]
    bot: Option<bool>,
}

#[derive(Deserialize)]
struct MessageReference {
    #[serde(default)]
    message_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture_backend(server: &mut mockito::Server) -> DiscordBackend {
        let _identity = server
            .mock("GET", "/users/@me")
            .with_status(200)
            .with_body(r#"{"id":"BOT123"}"#)
            .create_async()
            .await;
        DiscordBackend::new_at(server.url(), "discord-token".into())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn provider_metadata_discord() {
        let mut server = mockito::Server::new_async().await;
        let backend = fixture_backend(&mut server).await;
        assert_eq!(backend.provider_name(), "discord");
        assert!(backend.is_experimental());
    }

    #[tokio::test]
    async fn posts_to_messages_endpoint_with_bot_auth() {
        let mut server = mockito::Server::new_async().await;
        let backend = fixture_backend(&mut server).await;

        let post_mock = server
            .mock("POST", "/channels/CHAN/messages")
            .match_header("authorization", "Bot discord-token")
            .match_body(mockito::Matcher::JsonString(
                r#"{"content":"❓ `feat-x`: pick a name?"}"#.to_string(),
            ))
            .with_status(200)
            .with_body(r#"{"id":"999000111222333"}"#)
            .create_async()
            .await;

        let id = backend
            .post_question("CHAN", "feat-x", "pick a name?")
            .await
            .unwrap();
        assert_eq!(id, "999000111222333");
        post_mock.assert_async().await;
    }

    #[tokio::test]
    async fn polls_replies_filtered_by_message_reference() {
        let mut server = mockito::Server::new_async().await;
        let backend = fixture_backend(&mut server).await;

        let _human = server
            .mock("GET", "/channels/CHAN/messages?after=999&limit=50")
            .with_status(200)
            .with_body(
                r#"[
                    {"id":"1001","content":"hi","author":{"id":"BOT123","bot":true},
                     "message_reference":{"message_id":"999"}},
                    {"id":"1002","content":"use SAMPLE","author":{"id":"USER77","bot":false},
                     "message_reference":{"message_id":"999"}}
                ]"#,
            )
            .create_async()
            .await;

        let reply = backend
            .poll_thread_for_human_reply("CHAN", "999")
            .await
            .unwrap()
            .expect("human reply present");
        assert_eq!(reply.text, "use SAMPLE");
        assert_eq!(reply.user_id, "USER77");
        assert_eq!(reply.ts, "1002");
    }

    #[tokio::test]
    async fn polls_returns_none_when_only_bot_reply() {
        let mut server = mockito::Server::new_async().await;
        let backend = fixture_backend(&mut server).await;

        let _bot_only = server
            .mock("GET", "/channels/CHAN/messages?after=999&limit=50")
            .with_status(200)
            .with_body(
                r#"[
                    {"id":"1001","content":"hi","author":{"id":"BOT123","bot":true},
                     "message_reference":{"message_id":"999"}}
                ]"#,
            )
            .create_async()
            .await;

        let reply = backend
            .poll_thread_for_human_reply("CHAN", "999")
            .await
            .unwrap();
        assert!(reply.is_none());
    }
}
