//! QQ Official Bot platform adapter.
//!
//! Implements [`robit_chatbot::PlatformAdapter`] for the QQ Official Bot API:
//!
//! - **Access token**: obtained via the OAuth2 `getAppAccessToken` endpoint
//!   (app_id + app_secret), refreshed before expiry, and used for both the
//!   WebSocket Identify and HTTP message sends.
//! - **WebSocket gateway**: connects, sends Identify (`op=2`), then runs a
//!   heartbeat task and a dispatch task. Dispatch converts
//!   `C2C_MESSAGE_CREATE` / `GROUP_AT_MESSAGE_CREATE` events into
//!   [`PlatformEvent::Message`].
//! - **HTTP send**: POSTs text to the group or user messages endpoint.
//!
//! `chat_id` encoding: `"group:{group_openid}"` for group chats,
//! `"private:{user_openid}"` for C2C chats.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use robit_agent::error::{AgentError, Result};
use robit_ai::config::RobitConfig;
use robit_chatbot::adapter::{
    ChatMessage, ChatType, PlatformAdapter, PlatformCaps, PlatformEvent, SendResult, SenderInfo,
};
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use crate::protocol::{
    event_type, AccessTokenRequest, AccessTokenResponse, GatewayPayload, HelloData, MessageEvent,
    SendMessageRequest, SendMessageResponse, op,
};

/// QQ Bot configuration parsed from `[channels.qq_bot]`.
#[derive(Debug, Clone)]
pub struct QqConfig {
    pub app_id: String,
    pub app_secret: String,
    /// Bot token (used as a fallback / for sandbox; the live API uses the
    /// app-access-token flow derived from app_id + app_secret).
    pub bot_token: String,
    pub sandbox: bool,
}

impl QqConfig {
    /// Extract QQ Bot config from the loaded `RobitConfig`.
    pub fn from_config(config: &RobitConfig) -> std::result::Result<Self, String> {
        let qq = config
            .channels
            .as_ref()
            .and_then(|c| c.qq_bot.as_ref())
            .ok_or_else(|| {
                "QQ Bot config not found. Add [channels.qq_bot] section to config.toml".to_string()
            })?;
        Ok(Self {
            app_id: qq.app_id.clone(),
            app_secret: qq.app_secret.clone(),
            bot_token: qq.bot_token.clone(),
            sandbox: false,
        })
    }

    /// WebSocket gateway URL.
    pub fn gateway_url(&self) -> &str {
        if self.sandbox {
            "wss://sandbox.api.sgroup.qq.com/websockets"
        } else {
            "wss://api.sgroup.qq.com/websockets"
        }
    }

    /// HTTP API base URL.
    pub fn api_base_url(&self) -> &str {
        if self.sandbox {
            "https://sandbox.api.sgroup.qq.com"
        } else {
            "https://api.sgroup.qq.com"
        }
    }

    /// App access token endpoint.
    pub fn access_token_url(&self) -> &str {
        "https://bots.qq.com/app/getAppAccessToken"
    }
}

/// A cached access token with its expiry.
struct CachedToken {
    token: String,
    /// Instant at which the token expires.
    expires_at: Instant,
}

/// QQ Official Bot platform adapter.
pub struct QqPlatformAdapter {
    config: QqConfig,
    /// HTTP client for sending messages and fetching access tokens.
    http: reqwest::Client,
    /// Cached app access token (refreshed as needed).
    access_token: RwLock<Option<CachedToken>>,
    /// Last sequence number received (for heartbeats / resume).
    last_seq: Mutex<Option<u64>>,
    /// Session ID from the Ready event (for resume).
    session_id: Mutex<Option<String>>,
    /// Inbound event channel: dispatch/heartbeat tasks push, recv_event pops.
    event_tx: mpsc::Sender<PlatformEvent>,
    event_rx: Mutex<mpsc::Receiver<PlatformEvent>>,
    /// Outbound WebSocket writes (shared between send_message-via-WS and the
    /// heartbeat task). Currently send_message uses HTTP, so this is owned by
    /// the dispatch/heartbeat tasks.
    ws_tx: Mutex<Option<futures_util::stream::SplitSink<WebSocket, Message>>>,
    /// Platform capabilities (kept for diagnostics; capabilities() is static).
    #[allow(dead_code)]
    caps: PlatformCaps,
    /// Heartbeat interval (from the Hello event).
    heartbeat_interval: RwLock<Duration>,
    /// Tracks the last received message ID per chat, so passive replies can
    /// reference it (QQ requires `msg_id` within 5 min of the original).
    last_inbound_msg_id: Mutex<Option<(String, String)>>, // (chat_id, msg_id)
    /// Monotonic counter for `msg_seq` per reply.
    msg_seq: Mutex<u32>,
}

type WebSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

impl QqPlatformAdapter {
    /// Connect to the QQ gateway and start the heartbeat + dispatch tasks.
    ///
    /// This is the constructor used in place of `PlatformAdapter::connect` when
    /// we already own the config (avoids the `Self::Config` indirection).
    pub async fn connect(config: QqConfig) -> Result<Arc<Self>> {
        let http = reqwest::Client::new();
        let caps = PlatformCaps::qq();
        let (event_tx, event_rx) = mpsc::channel::<PlatformEvent>(256);

        let adapter = Arc::new(Self {
            config: config.clone(),
            http,
            access_token: RwLock::new(None),
            last_seq: Mutex::new(None),
            session_id: Mutex::new(None),
            event_tx,
            event_rx: Mutex::new(event_rx),
            ws_tx: Mutex::new(None),
            caps,
            heartbeat_interval: RwLock::new(Duration::from_secs(41)),
            last_inbound_msg_id: Mutex::new(None),
            msg_seq: Mutex::new(0),
        });

        adapter.establish_connection().await?;
        Ok(adapter)
    }

    /// Open the WebSocket, complete the Hello → Identify handshake, and spawn
    /// the heartbeat + dispatch background tasks.
    async fn establish_connection(self: &Arc<Self>) -> Result<()> {
        info!("Connecting to QQ gateway: {}", self.config.gateway_url());
        let (ws_stream, _response) = tokio_tungstenite::connect_async(self.config.gateway_url())
            .await
            .map_err(|e| AgentError::InternalError(format!("WebSocket connect failed: {}", e)))?;

        let (mut write, mut read) = ws_stream.split();
        write
            .send(Message::Ping(Vec::new()))
            .await
            .map_err(|e| AgentError::InternalError(format!("WS ping failed: {}", e)))?;

        // 1. Wait for Hello (op=10) to learn the heartbeat interval.
        let heartbeat_interval = loop {
            let msg = read
                .next()
                .await
                .ok_or_else(|| AgentError::InternalError("WebSocket closed before Hello".into()))?
                .map_err(|e| AgentError::InternalError(format!("WS read error: {}", e)))?;
            if let Message::Text(text) = msg {
                let payload: GatewayPayload =
                    serde_json::from_str(&text).map_err(|e| {
                        AgentError::InternalError(format!("Invalid Hello JSON: {}", e))
                    })?;
                if payload.op == op::HELLO {
                    let hello: HelloData = serde_json::from_value(
                        payload.d.ok_or_else(|| AgentError::InternalError("Hello missing d".into()))?,
                    )
                    .map_err(|e| AgentError::InternalError(format!("Invalid Hello data: {}", e)))?;
                    break Duration::from_millis(hello.heartbeat_interval);
                }
            }
        };
        *self.heartbeat_interval.write().await = heartbeat_interval;
        info!("QQ heartbeat interval: {:?}", heartbeat_interval);

        // 2. Fetch an app access token and send Identify (op=2).
        let access_token = self.fetch_access_token().await?;
        let identify =
            GatewayPayload::identify(&access_token, INTENT_C2C | INTENT_GROUP_AT_MESSAGE);
        write
            .send(Message::Text(serde_json::to_string(&identify).unwrap()))
            .await
            .map_err(|e| AgentError::InternalError(format!("Identify send failed: {}", e)))?;

        // 3. Store the write half and spawn the heartbeat + dispatch tasks.
        *self.ws_tx.lock().await = Some(write);

        spawn_heartbeat(Arc::clone(self));
        spawn_dispatch(Arc::clone(self), read);

        Ok(())
    }

    /// Fetch (and cache) an app access token, returning a fresh one.
    async fn fetch_access_token(&self) -> Result<String> {
        // Return cached if still valid (with a 60s safety margin).
        {
            let cache = self.access_token.read().await;
            if let Some(cached) = cache.as_ref() {
                if cached.expires_at.duration_since(Instant::now())
                    > Duration::from_secs(60)
                {
                    return Ok(cached.token.clone());
                }
            }
        }

        let req = AccessTokenRequest {
            app_id: self.config.app_id.clone(),
            client_secret: self.config.app_secret.clone(),
        };

        let response = self
            .http
            .post(self.config.access_token_url())
            .json(&req)
            .send()
            .await
            .map_err(|e| AgentError::InternalError(format!("Access token request failed: {}", e)))?;

        let status = response.status();
        let text = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(AgentError::InternalError(format!("Access token request failed ({}): {}", status, text)));
        }

        let resp: AccessTokenResponse = serde_json::from_str(&text)
            .map_err(|e| AgentError::InternalError(format!("Access token parse failed: {}", e)))?;

        let token = resp.access_token.clone();
        let expires_in = resp.expires_in.max(60);
        let cached = CachedToken {
            token: token.clone(),
            expires_at: Instant::now() + Duration::from_secs(expires_in),
        };
        *self.access_token.write().await = Some(cached);
        debug!("Fetched QQ access token (expires in {}s)", expires_in);
        Ok(token)
    }

    /// Build the Authorization header value (`QQBot {token}`).
    async fn auth_header(&self) -> Result<String> {
        let token = self.fetch_access_token().await?;
        Ok(format!("QQBot {}", token))
    }

    /// Record the inbound message ID for a chat (so a later reply can reference it).
    fn record_inbound(&self, chat_id: &str, msg_id: &str) {
        if let Ok(mut guard) = self.last_inbound_msg_id.try_lock() {
            *guard = Some((chat_id.to_string(), msg_id.to_string()));
        }
    }

    /// The inbound message ID to reference for a reply to `chat_id`, if any.
    async fn reply_msg_id(&self, chat_id: &str) -> Option<String> {
        let guard = self.last_inbound_msg_id.lock().await;
        guard
            .as_ref()
            .filter(|(cid, _)| cid == chat_id)
            .map(|(_, id)| id.clone())
    }

    async fn next_msg_seq(&self) -> u32 {
        let mut seq = self.msg_seq.lock().await;
        *seq = seq.wrapping_add(1);
        *seq
    }
}

#[async_trait]
impl PlatformAdapter for QqPlatformAdapter {
    fn capabilities() -> PlatformCaps {
        PlatformCaps::qq()
    }

    async fn send_message(&self, chat_id: &str, text: &str) -> Result<SendResult> {
        let auth = self.auth_header().await?;
        let (endpoint, is_group) = resolve_send_endpoint(self.config.api_base_url(), chat_id)?;

        let msg_id = self.reply_msg_id(chat_id).await;
        let msg_seq = self.next_msg_seq().await;
        let body = SendMessageRequest {
            content: text.to_string(),
            msg_type: crate::protocol::msg_type::TEXT,
            msg_id,
            msg_seq: Some(msg_seq),
        };

        let resp = self
            .http
            .post(&endpoint)
            .header("Authorization", &auth)
            .json(&body)
            .send()
            .await
            .map_err(|e| AgentError::InternalError(format!("QQ send failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            warn!("QQ send {} failed ({}): {}", endpoint, status, text);
            return Err(AgentError::InternalError(format!(
                "QQ send failed ({})",
                status
            )));
        }

        let parsed: SendMessageResponse = resp
            .json()
            .await
            .map_err(|e| AgentError::InternalError(format!("QQ send response parse: {}", e)))?;

        let id = parsed
            .id
            .or(parsed.msg_id)
            .unwrap_or_else(|| format!("sent-{}", msg_seq));
        debug!("QQ message sent to {} (id={})", chat_id, id);
        let _ = is_group; // currently unused beyond endpoint selection
        Ok(SendResult { msg_id: id })
    }

    async fn edit_message(&self, _chat_id: &str, _msg_id: &str, _text: &str) -> Result<()> {
        // QQ's passive-reply model doesn't support editing a sent message in
        // place (each reply is a new message referencing a msg_id). Fall back
        // to a fresh send so edit-based streaming degrades gracefully.
        self.send_message(_chat_id, _text).await?;
        Ok(())
    }

    async fn recv_event(&self) -> Result<PlatformEvent> {
        self.event_rx
            .lock()
            .await
            .recv()
            .await
            .ok_or_else(|| AgentError::InternalError("QQ event channel closed".into()))
    }
}

/// Intent bitmask for C2C + group @-messages.
const INTENT_C2C: u32 = crate::protocol::INTENT_C2C;
const INTENT_GROUP_AT_MESSAGE: u32 = crate::protocol::INTENT_GROUP_AT_MESSAGE;

/// Resolve the HTTP send endpoint for a chat_id.
///
/// `group:{openid}` → `/v2/groups/{openid}/messages`
/// `private:{openid}` → `/v2/users/{openid}/messages`
fn resolve_send_endpoint(base: &str, chat_id: &str) -> Result<(String, bool)> {
    if let Some(group_id) = chat_id.strip_prefix("group:") {
        return Ok((format!("{}/v2/groups/{}/messages", base, group_id), true));
    }
    if let Some(user_id) = chat_id.strip_prefix("private:") {
        return Ok((format!("{}/v2/users/{}/messages", base, user_id), false));
    }
    Err(AgentError::InternalError(format!(
        "Invalid chat_id '{}': expected 'group:{{id}}' or 'private:{{id}}'",
        chat_id
    )))
}

/// Spawn the periodic heartbeat task.
fn spawn_heartbeat(adapter: Arc<QqPlatformAdapter>) {
    tokio::spawn(async move {
        loop {
            let interval = *adapter.heartbeat_interval.read().await;
            tokio::time::sleep(interval).await;

            let last_seq = *adapter.last_seq.lock().await;
            let heartbeat = GatewayPayload::heartbeat(last_seq);
            let payload = match serde_json::to_string(&heartbeat) {
                Ok(p) => p,
                Err(e) => {
                    warn!("Failed to serialize heartbeat: {}", e);
                    continue;
                }
            };
            let mut ws_tx = adapter.ws_tx.lock().await;
            if let Some(write) = ws_tx.as_mut() {
                if let Err(e) = write.send(Message::Text(payload)).await {
                    warn!("Heartbeat send failed: {}", e);
                    let _ = adapter
                        .event_tx
                        .send(PlatformEvent::Disconnected)
                        .await;
                    return;
                }
                debug!("Heartbeat sent (seq={:?})", last_seq);
            } else {
                warn!("Heartbeat: no WS writer (disconnected)");
                return;
            }
        }
    });
}

/// Spawn the dispatch task: reads WS frames, converts dispatch events to
/// [`PlatformEvent`], and forwards them to the event channel.
fn spawn_dispatch(adapter: Arc<QqPlatformAdapter>, mut read: impl futures_util::Stream<Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin + Send + 'static) {
    tokio::spawn(async move {
        while let Some(frame) = read.next().await {
            let msg = match frame {
                Ok(m) => m,
                Err(e) => {
                    warn!("WS read error: {}", e);
                    let _ = adapter.event_tx.send(PlatformEvent::Disconnected).await;
                    return;
                }
            };
            let text = match msg {
                Message::Text(t) => t,
                Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
                Message::Close(_) => {
                    info!("QQ WebSocket closed by server");
                    let _ = adapter.event_tx.send(PlatformEvent::Disconnected).await;
                    return;
                }
                _ => continue,
            };

            let payload: GatewayPayload = match serde_json::from_str(&text) {
                Ok(p) => p,
                Err(e) => {
                    debug!("Skipping non-JSON WS frame: {}", e);
                    continue;
                }
            };

            match payload.op {
                op::HEARTBEAT_ACK => {
                    debug!("Heartbeat ACK");
                }
                op::RECONNECT => {
                    warn!("Server requested reconnect");
                    let _ = adapter.event_tx.send(PlatformEvent::Disconnected).await;
                    return;
                }
                op::DISPATCH => {
                    if let Some(seq) = payload.s {
                        *adapter.last_seq.lock().await = Some(seq);
                    }
                    let event_name = payload.t.as_deref().unwrap_or("");
                    match event_name {
                        event_type::READY => {
                            info!("QQ bot is ready");
                            if let Some(d) = payload.d {
                                if let Some(sid) = d.get("session_id").and_then(|v| v.as_str()) {
                                    *adapter.session_id.lock().await = Some(sid.to_string());
                                }
                            }
                        }
                        event_type::C2C_MESSAGE_CREATE
                        | event_type::GROUP_AT_MESSAGE_CREATE => {
                            if let Some(d) = payload.d {
                                if let Ok(ev) = serde_json::from_value::<MessageEvent>(d) {
                                    // Record the inbound msg_id for replies, then forward.
                                    if let Some(chat_id) = chat_id_for_event(event_name, &ev) {
                                        adapter.record_inbound(&chat_id, &ev.id);
                                    }
                                    if let Some(platform_ev) =
                                        build_platform_event(event_name, &ev)
                                    {
                                        let _ = adapter.event_tx.send(platform_ev).await;
                                    }
                                }
                            }
                        }
                        _ => {
                            debug!("Ignoring dispatch event: {}", event_name);
                        }
                    }
                }
                _ => {
                    debug!("Unhandled op {}: {:?}", payload.op, payload.t);
                }
            }
        }
        info!("QQ dispatch stream ended");
        let _ = adapter.event_tx.send(PlatformEvent::Disconnected).await;
    });
}

/// Compute the platform `chat_id` for a QQ message event.
fn chat_id_for_event(event_name: &str, ev: &MessageEvent) -> Option<String> {
    match event_name {
        event_type::GROUP_AT_MESSAGE_CREATE => {
            Some(format!("group:{}", ev.group_openid.clone()?))
        }
        event_type::C2C_MESSAGE_CREATE => {
            Some(format!("private:{}", ev.user_id()?))
        }
        _ => None,
    }
}

/// Convert a QQ message event into a platform-agnostic [`PlatformEvent::Message`].
fn build_platform_event(event_name: &str, ev: &MessageEvent) -> Option<PlatformEvent> {
    let chat_id = chat_id_for_event(event_name, ev)?;
    let chat_type = match event_name {
        event_type::GROUP_AT_MESSAGE_CREATE => ChatType::Group,
        event_type::C2C_MESSAGE_CREATE => ChatType::Private,
        _ => return None,
    };
    let user_id = ev.user_id().unwrap_or("unknown").to_string();
    // QQ group @-message content typically has a leading space from the @mention.
    let text = ev.content.trim().to_string();
    Some(PlatformEvent::Message(ChatMessage {
        text,
        sender: SenderInfo {
            user_id,
            chat_id,
            chat_type,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> QqConfig {
        QqConfig {
            app_id: "id".into(),
            app_secret: "secret".into(),
            bot_token: "tok".into(),
            sandbox: false,
        }
    }

    #[test]
    fn resolves_group_send_endpoint() {
        let (url, is_group) = resolve_send_endpoint("https://api.sgroup.qq.com", "group:abc").unwrap();
        assert_eq!(url, "https://api.sgroup.qq.com/v2/groups/abc/messages");
        assert!(is_group);
    }

    #[test]
    fn resolves_private_send_endpoint() {
        let (url, is_group) =
            resolve_send_endpoint("https://api.sgroup.qq.com", "private:user1").unwrap();
        assert_eq!(url, "https://api.sgroup.qq.com/v2/users/user1/messages");
        assert!(!is_group);
    }

    #[test]
    fn rejects_invalid_chat_id() {
        assert!(resolve_send_endpoint("https://x", "bogus").is_err());
    }

    #[test]
    fn builds_platform_event_for_group() {
        let ev = MessageEvent {
            id: "m1".into(),
            content: " hello".into(),
            author: crate::protocol::Author {
                user_openid: None,
                member_openid: Some("mem1".into()),
            },
            group_openid: Some("grp1".into()),
        };
        let pe = build_platform_event(event_type::GROUP_AT_MESSAGE_CREATE, &ev).unwrap();
        match pe {
            PlatformEvent::Message(m) => {
                assert_eq!(m.sender.chat_id, "group:grp1");
                assert_eq!(m.sender.chat_type, ChatType::Group);
                assert_eq!(m.text, "hello"); // leading space trimmed
                assert_eq!(m.sender.user_id, "mem1");
            }
            _ => panic!("expected Message"),
        }
    }

    #[test]
    fn builds_platform_event_for_c2c() {
        let ev = MessageEvent {
            id: "m2".into(),
            content: "hi".into(),
            author: crate::protocol::Author {
                user_openid: Some("u1".into()),
                member_openid: None,
            },
            group_openid: None,
        };
        let pe = build_platform_event(event_type::C2C_MESSAGE_CREATE, &ev).unwrap();
        match pe {
            PlatformEvent::Message(m) => {
                assert_eq!(m.sender.chat_id, "private:u1");
                assert_eq!(m.sender.chat_type, ChatType::Private);
            }
            _ => panic!("expected Message"),
        }
    }

    #[test]
    fn from_config_extracts_qq_section() {
        let toml_str = r#"
            [channels.qq_bot]
            app_id = "123"
            app_secret = "s"
            bot_token = "t"
        "#;
        // RobitConfig requires a non-empty `providers` map, so add a minimal one.
        let toml_with_providers = format!(
            "{}\n[providers.x]\nbase_url = \"https://x\"\napi_key = \"k\"\n[[providers.x.models]]\nid = \"m\"\n",
            toml_str
        );
        let config: RobitConfig = toml::from_str(&toml_with_providers).unwrap();
        let qq = QqConfig::from_config(&config).unwrap();
        assert_eq!(qq.app_id, "123");
        assert_eq!(qq.app_secret, "s");
        assert_eq!(qq.bot_token, "t");
    }

    #[test]
    fn from_config_errors_when_missing() {
        let toml_str = r#"
            [providers.x]
            base_url = "https://x"
            api_key = "k"
            [[providers.x.models]]
            id = "m"
        "#;
        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        assert!(QqConfig::from_config(&config).is_err());
    }

    #[test]
    fn gateway_and_api_urls() {
        let c = cfg();
        assert!(c.gateway_url().starts_with("wss://"));
        assert!(c.api_base_url().starts_with("https://"));
    }
}
