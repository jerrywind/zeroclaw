use super::traits::{Channel, ChannelMessage};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tokio::time::interval;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

const QQ_API_BASE: &str = "https://api.sgroup.qq.com";
const QQ_SANDBOX_API_BASE: &str = "https://sandbox.api.sgroup.qq.com";

pub struct QQChannel {
    app_id: String,
    app_secret: String,
    api_base: String,
    client: Client,
    access_token: RwLock<Option<String>>,
    token_expires_at: RwLock<u64>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: String,
}

impl QQChannel {
    pub fn new(app_id: String, app_secret: String, sandbox: bool) -> Self {
        let api_base = if sandbox {
            QQ_SANDBOX_API_BASE.to_string()
        } else {
            QQ_API_BASE.to_string()
        };

        Self {
            app_id,
            app_secret,
            api_base,
            client: Client::new(),
            access_token: RwLock::new(None),
            token_expires_at: RwLock::new(0),
        }
    }

    fn api_url(&self, endpoint: &str) -> String {
        format!("{}{}", self.api_base, endpoint)
    }

    async fn get_access_token(&self) -> anyhow::Result<String> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        // 1. Try to read from cache
        {
            let token_lock = self.access_token.read().await;
            let expiry_lock = self.token_expires_at.read().await;
            if let Some(token) = &*token_lock {
                if now < *expiry_lock {
                    return Ok(token.clone());
                }
            }
        }

        // 2. Cache miss or expired, acquire write lock
        let mut token_lock = self.access_token.write().await;
        let mut expiry_lock = self.token_expires_at.write().await;

        // Double-check in case another thread refreshed it
        if let Some(token) = &*token_lock {
            if now < *expiry_lock {
                return Ok(token.clone());
            }
        }

        tracing::info!("Refetching QQ access token...");

        let url = self.api_url("/app/getAppAccessToken");
        let payload = json!({
            "appId": self.app_id,
            "clientSecret": self.app_secret
        });

        let resp = self.client.post(&url).json(&payload).send().await?;

        if !resp.status().is_success() {
            let err = resp.text().await.unwrap_or_default();
            anyhow::bail!("Failed to fetch QQ access token: {}", err);
        }

        let data: TokenResponse = resp.json().await?;
        let expires_in: u64 = data.expires_in.parse().unwrap_or(7200);

        let new_token = data.access_token;
        let new_expiry = now + expires_in - 60; // buffer 60s

        *token_lock = Some(new_token.clone());
        *expiry_lock = new_expiry;

        Ok(new_token)
    }

    async fn auth_header(&self) -> anyhow::Result<String> {
        let token = self.get_access_token().await?;
        Ok(format!("QQBot {}", token))
    }

    async fn get_gateway_url(&self) -> anyhow::Result<String> {
        let url = self.api_url("/gateway");
        let resp = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header().await?)
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("Failed to get gateway URL: {}", resp.status());
        }

        let body: serde_json::Value = resp.json().await?;
        let wss_url = body["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No 'url' in gateway response"))?;
        Ok(wss_url.to_string())
    }
}

#[async_trait]
impl Channel for QQChannel {
    fn name(&self) -> &str {
        "qq"
    }

    async fn send(&self, message: &str, recipient: &str) -> anyhow::Result<()> {
        let url = self.api_url(&format!("/channels/{recipient}/messages"));

        // Note: msg_id is often required for passive messages (replies).
        // For now, we just send content. Active push might need messge_id if it's a reply.
        let body = json!({
            "content": message
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header().await?)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err_text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Failed to send QQ message: {} - {}", url, err_text);
        }

        Ok(())
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        let gateway_url = self.get_gateway_url().await?;
        tracing::info!("Connecting to QQ Gateway: {}", gateway_url);

        let (ws_stream, _) = connect_async(&gateway_url).await?;
        let (mut write, mut read) = ws_stream.split();

        // Heartbeat interval (default logic)
        let mut heartbeat_interval = interval(Duration::from_secs(40));
        let mut last_seq: Option<u32> = None;

        // Identify
        // Need fresh token for identify payload
        let token = self.get_access_token().await?;
        let intents = (1 << 30) | (1 << 12); // PUBLIC_GUILD_MESSAGES | DIRECT_MESSAGES

        let identify_payload = json!({
            "op": 2,
            "d": {
                "token": format!("QQBot {}", token), // Verify standard format: "QQBot <token>" or just token?
                                                    // Docs say "Bot <app_id>.<token>" for old, "QQBot <access_token>" for new.
                                                    // In identify payload, field is "token".
                                                    // Usually it includes the prefix. "QQBot <token>"
                "intents": intents,
                "shard": [0, 1],
                "properties": {
                    "$os": "linux",
                    "$browser": "zeroclaw",
                    "$device": "zeroclaw"
                }
            }
        });

        write
            .send(Message::Text(identify_payload.to_string()))
            .await?;

        loop {
            tokio::select! {
                _ = heartbeat_interval.tick() => {
                    let hb = json!({
                        "op": 1,
                        "d": last_seq
                    });
                    if let Err(e) = write.send(Message::Text(hb.to_string())).await {
                        tracing::error!("Failed to send heartbeat: {}", e);
                        break;
                    }
                }
                msg = read.next() => {
                    let msg = match msg {
                        Some(Ok(m)) => m,
                        Some(Err(e)) => return Err(e.into()),
                        None => break,
                    };

                    if let Message::Text(text) = msg {
                        let data: serde_json::Value = serde_json::from_str(&text)?;

                        let op = data["op"].as_u64().unwrap_or(0);

                        // Hello Packet
                        if op == 10 {
                            if let Some(interval_ms) = data["d"]["heartbeat_interval"].as_u64() {
                                heartbeat_interval = interval(Duration::from_millis(interval_ms));
                            }
                        }

                        // Dispatch
                        if op == 0 {
                            if let Some(s) = data["s"].as_u64() {
                                if let Ok(seq) = u32::try_from(s) {
                                    last_seq = Some(seq);
                                }
                            }

                            if let Some("AT_MESSAGE_CREATE" | "MESSAGE_CREATE") = data["t"].as_str() {
                                let d = &data["d"];
                                let content = d["content"].as_str().unwrap_or_default();
                                // let author = &d["author"];
                                let channel_id = d["channel_id"].as_str().unwrap_or("unknown");
                                let msg_id = d["id"].as_str().unwrap_or("unknown");

                                // Removed allowed_users check as requested

                                let msg = ChannelMessage {
                                    id: msg_id.to_string(),
                                    sender: channel_id.to_string(),
                                    content: content.to_string(),
                                    channel: "qq".to_string(),
                                    timestamp: SystemTime::now()
                                        .duration_since(UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs(),
                                };

                                if tx.send(msg).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn health_check(&self) -> bool {
        match self
            .client
            .get(self.api_url("/users/@me"))
            .header(
                "Authorization",
                match self.auth_header().await {
                    Ok(h) => h,
                    Err(_) => return false,
                },
            )
            .send()
            .await
        {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }
}
