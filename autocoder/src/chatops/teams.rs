//! Microsoft Teams ChatOps backend (EXPERIMENTAL).
//!
//! OAuth `client_credentials` against Microsoft Graph. Token cached
//! in-process; re-acquired on 401 or expiry.

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use super::{ChatOpsBackend, HumanReply};

const DEFAULT_GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";
const DEFAULT_LOGIN_BASE: &str = "https://login.microsoftonline.com";

#[derive(Debug)]
struct TokenCache {
    access_token: String,
    expires_at: Instant,
}

pub struct TeamsBackend {
    client: reqwest::Client,
    api_base: String,
    login_base: String,
    tenant_id: String,
    client_id: String,
    client_secret: String,
    team_id: String,
    token_cache: RwLock<Option<TokenCache>>,
}

impl TeamsBackend {
    pub async fn new(
        tenant_id: String,
        client_id: String,
        client_secret: String,
        team_id: String,
    ) -> Result<Self> {
        Self::new_at(
            DEFAULT_GRAPH_BASE.to_string(),
            DEFAULT_LOGIN_BASE.to_string(),
            tenant_id,
            client_id,
            client_secret,
            team_id,
        )
        .await
    }

    #[doc(hidden)]
    pub async fn new_at(
        api_base: String,
        login_base: String,
        tenant_id: String,
        client_id: String,
        client_secret: String,
        team_id: String,
    ) -> Result<Self> {
        let backend = Self {
            client: reqwest::Client::new(),
            api_base,
            login_base,
            tenant_id,
            client_id,
            client_secret,
            team_id,
            token_cache: RwLock::new(None),
        };
        backend.acquire_token().await?;
        Ok(backend)
    }

    /// Bot identity (the Teams app's own client_id). Used to filter the
    /// bot's own messages out of thread polls.
    pub fn bot_identity(&self) -> &str {
        &self.client_id
    }

    async fn acquire_token(&self) -> Result<String> {
        let url = format!(
            "{}/{}/oauth2/v2.0/token",
            self.login_base.trim_end_matches('/'),
            self.tenant_id
        );
        let body = format!(
            "grant_type=client_credentials&client_id={}&client_secret={}&scope=https%3A%2F%2Fgraph.microsoft.com%2F.default",
            urlencode_form(&self.client_id),
            urlencode_form(&self.client_secret),
        );
        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| anyhow!("teams token request failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "teams oauth token endpoint failed: http {status}: {body}"
            ));
        }
        let parsed: TokenResponse = resp
            .json()
            .await
            .map_err(|e| anyhow!("teams token decode failed: {e}"))?;
        let token = parsed.access_token.clone();
        let expires = Instant::now()
            + Duration::from_secs(parsed.expires_in.saturating_sub(60).max(60));
        let mut guard = self.token_cache.write().unwrap();
        *guard = Some(TokenCache {
            access_token: parsed.access_token,
            expires_at: expires,
        });
        Ok(token)
    }

    fn cached_token(&self) -> Option<String> {
        let guard = self.token_cache.read().unwrap();
        guard.as_ref().and_then(|c| {
            if c.expires_at > Instant::now() {
                Some(c.access_token.clone())
            } else {
                None
            }
        })
    }

    async fn token(&self) -> Result<String> {
        if let Some(t) = self.cached_token() {
            return Ok(t);
        }
        self.acquire_token().await
    }
}

#[async_trait]
impl ChatOpsBackend for TeamsBackend {
    fn provider_name(&self) -> &'static str {
        "teams"
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
            "{}/teams/{}/channels/{channel}/messages",
            self.api_base.trim_end_matches('/'),
            self.team_id
        );
        let content = format!("❓ <code>{change}</code>: {question}");
        let payload = serde_json::json!({
            "body": {
                "content": content,
                "contentType": "html",
            }
        });
        let make_request = || async {
            let token = self.token().await?;
            self.client
                .post(&url)
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .json(&payload)
                .send()
                .await
                .map_err(|e| anyhow!("teams post request failed: {e}"))
        };

        let mut resp = make_request().await?;
        if resp.status().as_u16() == 401 {
            self.acquire_token().await?;
            resp = make_request().await?;
        }
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("teams post http {status}: {body}"));
        }
        let parsed: PostedMessage = resp
            .json()
            .await
            .map_err(|e| anyhow!("teams post decode failed: {e}"))?;
        Ok(parsed.id)
    }

    async fn poll_thread_for_human_reply(
        &self,
        channel: &str,
        handle: &str,
    ) -> Result<Option<HumanReply>> {
        let url = format!(
            "{}/teams/{}/channels/{channel}/messages/{handle}/replies",
            self.api_base.trim_end_matches('/'),
            self.team_id
        );
        let make_request = || async {
            let token = self.token().await?;
            self.client
                .get(&url)
                .header("Authorization", format!("Bearer {token}"))
                .send()
                .await
                .map_err(|e| anyhow!("teams replies request failed: {e}"))
        };

        let mut resp = make_request().await?;
        if resp.status().as_u16() == 401 {
            self.acquire_token().await?;
            resp = make_request().await?;
        }
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("teams replies http {status}: {body}"));
        }
        let parsed: RepliesResponse = resp
            .json()
            .await
            .map_err(|e| anyhow!("teams replies decode failed: {e}"))?;

        let mut human_replies: Vec<TeamsMessage> = parsed
            .value
            .into_iter()
            .filter(|m| {
                m.from
                    .as_ref()
                    .and_then(|f| f.user.as_ref())
                    .map(|u| u.id.as_str() != self.client_id.as_str())
                    .unwrap_or(false)
            })
            .collect();
        human_replies.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(human_replies.into_iter().next().map(|m| {
            let user = m
                .from
                .and_then(|f| f.user)
                .map(|u| u.id)
                .unwrap_or_default();
            let text = m
                .body
                .map(|b| b.content)
                .unwrap_or_default();
            HumanReply {
                text,
                user_id: user,
                ts: m.id,
            }
        }))
    }

    async fn post_notification(&self, channel: &str, text: &str) -> Result<()> {
        let url = format!(
            "{}/teams/{}/channels/{channel}/messages",
            self.api_base.trim_end_matches('/'),
            self.team_id
        );
        let payload = serde_json::json!({
            "body": {
                "content": text,
                "contentType": "text",
            }
        });
        let make_request = || async {
            let token = self.token().await?;
            self.client
                .post(&url)
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .json(&payload)
                .send()
                .await
                .map_err(|e| anyhow!("teams notification request failed: {e}"))
        };
        let mut resp = make_request().await?;
        if resp.status().as_u16() == 401 {
            self.acquire_token().await?;
            resp = make_request().await?;
        }
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("teams notification http {status}: {body}"));
        }
        Ok(())
    }
}

fn urlencode_form(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

#[derive(Deserialize)]
struct PostedMessage {
    id: String,
}

#[derive(Deserialize)]
struct RepliesResponse {
    #[serde(default)]
    value: Vec<TeamsMessage>,
}

#[derive(Deserialize)]
struct TeamsMessage {
    id: String,
    #[serde(default)]
    body: Option<MessageBody>,
    #[serde(default)]
    from: Option<MessageFrom>,
}

#[derive(Deserialize)]
struct MessageBody {
    #[serde(default)]
    content: String,
}

#[derive(Deserialize)]
struct MessageFrom {
    #[serde(default)]
    user: Option<MessageUser>,
}

#[derive(Deserialize)]
struct MessageUser {
    id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn make_backend(server: &mut mockito::Server) -> TeamsBackend {
        let _token_mock = server
            .mock("POST", "/tenant-x/oauth2/v2.0/token")
            .with_status(200)
            .with_body(r#"{"access_token":"GRAPH_TOKEN","expires_in":3600}"#)
            .create_async()
            .await;
        TeamsBackend::new_at(
            server.url(),
            server.url(),
            "tenant-x".into(),
            "client-x".into(),
            "secret-x".into(),
            "team-x".into(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn provider_metadata_teams() {
        let mut server = mockito::Server::new_async().await;
        let backend = make_backend(&mut server).await;
        assert_eq!(backend.provider_name(), "teams");
        assert!(backend.is_experimental());
    }

    #[tokio::test]
    async fn acquires_token_at_construction() {
        let mut server = mockito::Server::new_async().await;
        let token_mock = server
            .mock("POST", "/tenant-x/oauth2/v2.0/token")
            .match_header("content-type", "application/x-www-form-urlencoded")
            .match_body(mockito::Matcher::Regex("grant_type=client_credentials".into()))
            .with_status(200)
            .with_body(r#"{"access_token":"GRAPH_TOKEN","expires_in":3600}"#)
            .create_async()
            .await;
        let _ = TeamsBackend::new_at(
            server.url(),
            server.url(),
            "tenant-x".into(),
            "client-x".into(),
            "secret-x".into(),
            "team-x".into(),
        )
        .await
        .unwrap();
        token_mock.assert_async().await;
    }

    #[tokio::test]
    async fn posts_to_messages_endpoint_with_bearer_token() {
        let mut server = mockito::Server::new_async().await;
        let backend = make_backend(&mut server).await;

        let post_mock = server
            .mock("POST", "/teams/team-x/channels/CHAN/messages")
            .match_header("authorization", "Bearer GRAPH_TOKEN")
            .match_body(mockito::Matcher::JsonString(
                r#"{"body":{"content":"❓ <code>feat-x</code>: pick?","contentType":"html"}}"#
                    .to_string(),
            ))
            .with_status(201)
            .with_body(r#"{"id":"MSG-1"}"#)
            .create_async()
            .await;

        let id = backend.post_question("CHAN", "feat-x", "pick?").await.unwrap();
        assert_eq!(id, "MSG-1");
        post_mock.assert_async().await;
    }

    #[tokio::test]
    async fn polls_replies_filters_bot_self() {
        let mut server = mockito::Server::new_async().await;
        let backend = make_backend(&mut server).await;

        let _replies = server
            .mock("GET", "/teams/team-x/channels/CHAN/messages/MSG-1/replies")
            .with_status(200)
            .with_body(
                r#"{"value":[
                    {"id":"R1","body":{"content":"bot follow-up"},
                     "from":{"user":{"id":"client-x"}}},
                    {"id":"R2","body":{"content":"use SAMPLE"},
                     "from":{"user":{"id":"USER-77"}}}
                ]}"#,
            )
            .create_async()
            .await;

        let reply = backend
            .poll_thread_for_human_reply("CHAN", "MSG-1")
            .await
            .unwrap()
            .expect("human reply present");
        assert_eq!(reply.text, "use SAMPLE");
        assert_eq!(reply.user_id, "USER-77");
        assert_eq!(reply.ts, "R2");
    }

    #[tokio::test]
    async fn re_acquires_token_on_401() {
        let mut server = mockito::Server::new_async().await;
        let backend = make_backend(&mut server).await;

        // First attempt: 401.
        let _first = server
            .mock("POST", "/teams/team-x/channels/CHAN/messages")
            .match_header("authorization", "Bearer GRAPH_TOKEN")
            .with_status(401)
            .with_body("expired")
            .expect(1)
            .create_async()
            .await;

        // Second token endpoint hit returns a NEW token.
        let _re_token = server
            .mock("POST", "/tenant-x/oauth2/v2.0/token")
            .with_status(200)
            .with_body(r#"{"access_token":"NEW_TOKEN","expires_in":3600}"#)
            .expect(1)
            .create_async()
            .await;

        // Retry succeeds with the new token.
        let _retry = server
            .mock("POST", "/teams/team-x/channels/CHAN/messages")
            .match_header("authorization", "Bearer NEW_TOKEN")
            .with_status(201)
            .with_body(r#"{"id":"MSG-RETRY"}"#)
            .create_async()
            .await;

        let id = backend.post_question("CHAN", "ch", "q?").await.unwrap();
        assert_eq!(id, "MSG-RETRY");
    }
}
