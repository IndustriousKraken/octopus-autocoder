//! Matrix ChatOps backend (EXPERIMENTAL).
//!
//! Client-Server API: `PUT /rooms/{r}/send/m.room.message/{txn}` for sending,
//! `GET /rooms/{r}/messages?from=...` for polling. Reply threading uses
//! `m.relates_to.m.in_reply_to.event_id`.

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::RwLock;

use super::{ChatOpsBackend, HumanReply, urlencode};

pub struct MatrixBackend {
    client: reqwest::Client,
    homeserver_url: String,
    access_token: String,
    user_id: String,
    sync_from: RwLock<Option<String>>,
}

impl MatrixBackend {
    pub async fn new(homeserver_url: String, access_token: String) -> Result<Self> {
        let client = reqwest::Client::new();

        // Identity discovery via whoami.
        let whoami_url = format!(
            "{}/_matrix/client/v3/account/whoami",
            homeserver_url.trim_end_matches('/')
        );
        let resp = client
            .get(&whoami_url)
            .header("Authorization", format!("Bearer {access_token}"))
            .send()
            .await
            .map_err(|e| anyhow!("matrix whoami request failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "matrix whoami identity call failed: http {status}: {body}"
            ));
        }
        let parsed: WhoAmI = resp
            .json()
            .await
            .map_err(|e| anyhow!("matrix whoami decode failed: {e}"))?;

        // Initial sync to obtain a `next_batch` token used as the starting
        // point for subsequent message polls.
        let sync_url = format!(
            "{}/_matrix/client/v3/sync?timeout=0",
            homeserver_url.trim_end_matches('/')
        );
        let sync_resp = client
            .get(&sync_url)
            .header("Authorization", format!("Bearer {access_token}"))
            .send()
            .await
            .map_err(|e| anyhow!("matrix initial sync request failed: {e}"))?;
        let sync_status = sync_resp.status();
        let sync_from = if sync_status.is_success() {
            let parsed: SyncResponse = sync_resp
                .json()
                .await
                .map_err(|e| anyhow!("matrix sync decode failed: {e}"))?;
            parsed.next_batch
        } else {
            None
        };

        Ok(Self {
            client,
            homeserver_url,
            access_token,
            user_id: parsed.user_id,
            sync_from: RwLock::new(sync_from),
        })
    }

    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    fn current_sync_from(&self) -> Option<String> {
        self.sync_from.read().unwrap().clone()
    }

    fn set_sync_from(&self, token: String) {
        *self.sync_from.write().unwrap() = Some(token);
    }
}

#[async_trait]
impl ChatOpsBackend for MatrixBackend {
    fn provider_name(&self) -> &'static str {
        "matrix"
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
        let txn_id = uuid::Uuid::new_v4().to_string();
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
            self.homeserver_url.trim_end_matches('/'),
            urlencode(channel),
            urlencode(&txn_id),
        );
        let body_text = format!("❓ {change}: {question}");
        let payload = serde_json::json!({
            "msgtype": "m.text",
            "body": body_text,
        });
        let resp = self
            .client
            .put(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| anyhow!("matrix send request failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("matrix send http {status}: {body}"));
        }
        let parsed: SendResponse = resp
            .json()
            .await
            .map_err(|e| anyhow!("matrix send decode failed: {e}"))?;
        Ok(parsed.event_id)
    }

    async fn poll_thread_for_human_reply(
        &self,
        channel: &str,
        handle: &str,
    ) -> Result<Option<HumanReply>> {
        let mut url = format!(
            "{}/_matrix/client/v3/rooms/{}/messages?dir=f",
            self.homeserver_url.trim_end_matches('/'),
            urlencode(channel),
        );
        if let Some(from) = self.current_sync_from() {
            url.push_str(&format!("&from={}", urlencode(&from)));
        }

        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .send()
            .await
            .map_err(|e| anyhow!("matrix messages request failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("matrix messages http {status}: {body}"));
        }
        let parsed: MessagesResponse = resp
            .json()
            .await
            .map_err(|e| anyhow!("matrix messages decode failed: {e}"))?;

        if let Some(end) = parsed.end.clone() {
            self.set_sync_from(end);
        }

        let reply = parsed.chunk.into_iter().find(|ev| {
            ev.sender != self.user_id
                && ev
                    .content
                    .relates_to
                    .as_ref()
                    .and_then(|r| r.in_reply_to.as_ref())
                    .map(|rt| rt.event_id.as_str() == handle)
                    .unwrap_or(false)
        });
        Ok(reply.map(|ev| HumanReply {
            text: ev.content.body.unwrap_or_default(),
            user_id: ev.sender,
            ts: ev.event_id,
        }))
    }

    async fn post_notification(&self, channel: &str, text: &str) -> Result<()> {
        let txn_id = uuid::Uuid::new_v4().to_string();
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
            self.homeserver_url.trim_end_matches('/'),
            urlencode(channel),
            urlencode(&txn_id),
        );
        let payload = serde_json::json!({
            "msgtype": "m.text",
            "body": text,
        });
        let resp = self
            .client
            .put(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| anyhow!("matrix notification request failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("matrix notification http {status}: {body}"));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct WhoAmI {
    user_id: String,
}

#[derive(Deserialize)]
struct SyncResponse {
    #[serde(default)]
    next_batch: Option<String>,
}

#[derive(Deserialize)]
struct SendResponse {
    event_id: String,
}

#[derive(Deserialize)]
struct MessagesResponse {
    #[serde(default)]
    chunk: Vec<RoomEvent>,
    #[serde(default)]
    end: Option<String>,
}

#[derive(Deserialize)]
struct RoomEvent {
    event_id: String,
    sender: String,
    #[serde(default)]
    content: EventContent,
}

#[derive(Deserialize, Default)]
struct EventContent {
    #[serde(default)]
    body: Option<String>,
    #[serde(rename = "m.relates_to", default)]
    relates_to: Option<RelatesTo>,
}

#[derive(Deserialize)]
struct RelatesTo {
    #[serde(rename = "m.in_reply_to", default)]
    in_reply_to: Option<InReplyTo>,
}

#[derive(Deserialize)]
struct InReplyTo {
    event_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture_backend(server: &mut mockito::Server) -> MatrixBackend {
        let _whoami = server
            .mock("GET", "/_matrix/client/v3/account/whoami")
            .with_status(200)
            .with_body(r#"{"user_id":"@bot:server.tld"}"#)
            .create_async()
            .await;
        let _sync = server
            .mock("GET", "/_matrix/client/v3/sync?timeout=0")
            .with_status(200)
            .with_body(r#"{"next_batch":"BATCH_INIT"}"#)
            .create_async()
            .await;
        MatrixBackend::new(server.url(), "matrix-token".into())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn provider_metadata_matrix() {
        let mut server = mockito::Server::new_async().await;
        let backend = fixture_backend(&mut server).await;
        assert_eq!(backend.provider_name(), "matrix");
        assert!(backend.is_experimental());
    }

    #[tokio::test]
    async fn posts_room_message_event() {
        let mut server = mockito::Server::new_async().await;
        let backend = fixture_backend(&mut server).await;

        // Match any txn_id (UUIDv4); just validate the path prefix and body.
        let post_mock = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(
                    r"^/_matrix/client/v3/rooms/%21abc%3Aserver\.tld/send/m\.room\.message/.+$"
                        .into(),
                ),
            )
            .match_header("authorization", "Bearer matrix-token")
            .match_body(mockito::Matcher::JsonString(
                r#"{"msgtype":"m.text","body":"❓ feat-x: pick?"}"#.to_string(),
            ))
            .with_status(200)
            .with_body(r#"{"event_id":"$evt:server.tld"}"#)
            .create_async()
            .await;

        let id = backend
            .post_question("!abc:server.tld", "feat-x", "pick?")
            .await
            .unwrap();
        assert_eq!(id, "$evt:server.tld");
        post_mock.assert_async().await;
    }

    #[tokio::test]
    async fn polls_messages_filters_by_in_reply_to() {
        let mut server = mockito::Server::new_async().await;
        let backend = fixture_backend(&mut server).await;

        let _msgs = server
            .mock(
                "GET",
                mockito::Matcher::Regex(
                    r"^/_matrix/client/v3/rooms/%21abc%3Aserver\.tld/messages\?dir=f.*$"
                        .into(),
                ),
            )
            .with_status(200)
            .with_body(
                r#"{"chunk":[
                    {"event_id":"$x:server.tld","sender":"@bot:server.tld",
                     "content":{"body":"original","m.relates_to":null}},
                    {"event_id":"$y:server.tld","sender":"@bot:server.tld",
                     "content":{"body":"bot follow-up","m.relates_to":{"m.in_reply_to":{"event_id":"$evt:server.tld"}}}},
                    {"event_id":"$z:server.tld","sender":"@user:server.tld",
                     "content":{"body":"use SAMPLE","m.relates_to":{"m.in_reply_to":{"event_id":"$evt:server.tld"}}}}
                ],"end":"BATCH_NEXT"}"#,
            )
            .create_async()
            .await;

        let reply = backend
            .poll_thread_for_human_reply("!abc:server.tld", "$evt:server.tld")
            .await
            .unwrap()
            .expect("human reply");
        assert_eq!(reply.text, "use SAMPLE");
        assert_eq!(reply.user_id, "@user:server.tld");
        assert_eq!(reply.ts, "$z:server.tld");
    }
}
