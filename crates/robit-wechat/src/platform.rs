//! WeChat iLink Bot platform adapter.
//!
//! Implements [`robit_chatbot::PlatformAdapter`] for the official WeChat bot
//! channel (iLink AI Bot, `ilinkai.weixin.qq.com`) on top of the
//! protocol-only `weixin-agent` SDK:
//!
//! - **Credential**: a long-lived `bot_token` obtained by the one-time
//!   `robit-wechat login` QR scan (persisted in `wechat-state.json`).
//! - **Inbound**: the SDK runs a long-poll `getUpdates` loop (no public
//!   webhook needed); the [`MessageHandler`] bridge converts each message
//!   into [`PlatformEvent::Message`] over an mpsc channel.
//! - **Outbound**: `send_text` / `send_media` HTTP calls, correlated to the
//!   peer's last inbound `context_token` when available.
//! - **Media**: WeChat CDN payloads are AES-encrypted, so inbound media is
//!   downloaded and decrypted by the SDK here and handed to the manager with
//!   `local_path` already set (the manager skips its generic URL download).
//!
//! `chat_id` encoding: `"private:{ilink_user_id}"` — the channel is 1:1 only.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use robit_agent::error::{AgentError, Result};
use robit_ai::config::RobitConfig;
use robit_chatbot::adapter::{
    ChatMessage, ChatType, MediaAttachment, PlatformAdapter, PlatformCaps, PlatformEvent,
    SendResult, SenderInfo, UploadResult,
};
use tokio::sync::{mpsc, Mutex, Notify};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use weixin_agent::{
    MediaInfo, MediaType, MessageContext, MessageHandler, TokenStaleInfo, WeixinClient,
    WeixinConfig,
};

use crate::state::WechatState;

/// WeChat adapter configuration resolved from `[channels.wechat_bot]` and the
/// login state file.
#[derive(Debug, Clone)]
pub struct WechatConfig {
    /// Long-lived bot token (config override or login state).
    pub bot_token: String,
    /// Optional iLink API base URL override (config or login response).
    pub base_url: Option<String>,
    /// Optional CDN base URL override.
    pub cdn_base_url: Option<String>,
    /// Bot working directory (state file + media dir live here).
    pub working_dir: PathBuf,
}

impl WechatConfig {
    /// Extract WeChat config from the loaded `RobitConfig` plus the login
    /// state file. Token priority: `[channels.wechat_bot].bot_token` (env
    /// vars already resolved) → `wechat-state.json`.
    pub fn from_config(config: &RobitConfig, working_dir: &Path) -> std::result::Result<Self, String> {
        let section = config
            .channels
            .as_ref()
            .and_then(|c| c.wechat_bot.as_ref());
        let state = WechatState::load(working_dir);

        let bot_token = section
            .and_then(|s| s.bot_token.clone())
            .filter(|t| !t.trim().is_empty())
            .or_else(|| state.as_ref().and_then(|s| s.bot_token.clone()))
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| {
                "WeChat bot token not found. Run `robit-wechat login` first, or set \
                 [channels.wechat_bot] bot_token in config.toml"
                    .to_string()
            })?;

        Ok(Self {
            bot_token,
            base_url: section
                .and_then(|s| s.base_url.clone())
                .or_else(|| state.as_ref().and_then(|s| s.base_url.clone())),
            cdn_base_url: section.and_then(|s| s.cdn_base_url.clone()),
            working_dir: working_dir.to_path_buf(),
        })
    }

    /// Build the protocol SDK config. `token` may be empty for the QR login
    /// flow (login endpoints are unauthenticated; an empty token omits the
    /// Authorization header).
    pub fn sdk_config(&self, token: &str) -> std::result::Result<WeixinConfig, String> {
        let mut builder = WeixinConfig::builder()
            .token(token)
            .bot_agent(format!("robit-wechat/{}", env!("CARGO_PKG_VERSION")));
        if let Some(url) = &self.base_url {
            builder = builder.base_url(url.clone());
        }
        if let Some(url) = &self.cdn_base_url {
            builder = builder.cdn_base_url(url.clone());
        }
        builder.build().map_err(|e| format!("SDK config error: {}", e))
    }
}

/// WeChat iLink Bot platform adapter.
pub struct WechatPlatformAdapter {
    /// Protocol client (shared with the poll supervisor task).
    client: Arc<WeixinClient>,
    /// Inbound event channel: the handler bridge pushes, recv_event pops.
    event_rx: Mutex<mpsc::Receiver<PlatformEvent>>,
    /// Cancels the SDK poll loop on shutdown (held so the adapter outlives
    /// the supervisor task's copy; the supervisor triggers it).
    #[allow(dead_code)]
    poll_cancel: CancellationToken,
    /// Shared login/transport state (sync_buf + context tokens persistence).
    state: Arc<Mutex<WechatState>>,
}

impl WechatPlatformAdapter {
    /// Build the adapter and start the long-poll loop behind a supervisor.
    ///
    /// The SDK poll loop already retries transport/API failures with its own
    /// backoff, so this supervisor only handles an unexpected loop exit
    /// (restart with backoff) and Ctrl+C (graceful stop). A stale token is
    /// terminal: the bridge emits `Disconnected` so the manager exits and the
    /// user is told to re-run `robit-wechat login`.
    pub async fn connect(
        config: WechatConfig,
        shutdown: Arc<Notify>,
    ) -> Result<Arc<Self>> {
        let sdk_config = config
            .sdk_config(&config.bot_token)
            .map_err(|e| AgentError::InternalError(e))?;

        let state = WechatState::load(&config.working_dir).unwrap_or_default();
        let media_dir = config.working_dir.join("media");
        let (event_tx, event_rx) = mpsc::channel::<PlatformEvent>(256);

        let poll_cancel = CancellationToken::new();
        let token_stale = Arc::new(AtomicBool::new(false));
        let shared_state = Arc::new(Mutex::new(state.clone()));

        let handler = WechatHandlerBridge {
            event_tx,
            state: Arc::clone(&shared_state),
            working_dir: config.working_dir.clone(),
            media_dir: media_dir.clone(),
            poll_cancel: poll_cancel.clone(),
            token_stale: Arc::clone(&token_stale),
        };

        let client = WeixinClient::builder(sdk_config)
            .on_message(handler)
            .with_cancel_token(poll_cancel.clone())
            .build()
            .map_err(|e| AgentError::InternalError(format!("WeChat client build failed: {}", e)))?;
        let client = Arc::new(client);
        // Restore outbound context tokens from the persisted state.
        client.context_tokens().import(state.context_tokens.clone());

        let adapter = Arc::new(Self {
            client: Arc::clone(&client),
            event_rx: Mutex::new(event_rx),
            poll_cancel: poll_cancel.clone(),
            state: shared_state,
        });

        info!(
            "Starting WeChat iLink Bot long-poll loop (state: {})",
            crate::state::WechatState::path(&config.working_dir).display()
        );

        let supervisor_state = Arc::clone(&adapter.state);
        tokio::spawn(async move {
            poll_supervisor(client, supervisor_state, poll_cancel, token_stale, shutdown).await;
        });

        Ok(adapter)
    }

    /// The outbound context token for a peer, if any (client store first —
    /// refreshed by the poll loop — then the persisted state map).
    async fn outbound_context_token(&self, peer: &str) -> Option<String> {
        if let Some(token) = self.client.context_tokens().get(peer) {
            return Some(token);
        }
        self.state
            .lock()
            .await
            .context_tokens
            .get(peer)
            .cloned()
    }
}

/// Supervisor for the SDK poll loop: restarts it with bounded backoff after an
/// unexpected exit, and stops on shutdown or a stale token (terminal — the
/// bridge has already emitted `Disconnected` so the manager is exiting).
async fn poll_supervisor(
    client: Arc<WeixinClient>,
    state: Arc<Mutex<WechatState>>,
    poll_cancel: CancellationToken,
    token_stale: Arc<AtomicBool>,
    shutdown: Arc<Notify>,
) {
    let mut failures: u32 = 0;
    loop {
        let sync_buf = state.lock().await.sync_buf.clone();
        let result = tokio::select! {
            res = client.start(sync_buf) => res,
            _ = shutdown.notified() => {
                debug!("WeChat poll supervisor received shutdown signal");
                poll_cancel.cancel();
                return;
            }
        };

        match result {
            Ok(()) => info!("WeChat poll loop ended"),
            Err(e) => warn!("WeChat poll loop exited with error: {}", e),
        }

        if token_stale.load(Ordering::SeqCst) {
            // The bridge already emitted Disconnected; the manager is exiting
            // and the user must re-run `robit-wechat login`.
            return;
        }

        failures = failures.saturating_add(1);
        let backoff = reconnect_backoff(failures);
        warn!(
            "Restarting WeChat poll loop (attempt #{} ) in {:?}",
            failures, backoff
        );
        if sleep_or_shutdown(backoff, &shutdown).await {
            poll_cancel.cancel();
            return;
        }
    }
}

/// Exponential backoff after `failures` consecutive loop exits:
/// 1s, 2s, 4s, 8s, 16s, 30s, 30s, ...
fn reconnect_backoff(failures: u32) -> Duration {
    let secs = 1u64 << failures.saturating_sub(1).min(5);
    Duration::from_secs(secs.min(30))
}

/// Sleep for `dur`, returning `true` if shutdown was notified meanwhile.
async fn sleep_or_shutdown(dur: Duration, shutdown: &Arc<Notify>) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(dur) => false,
        _ = shutdown.notified() => true,
    }
}

/// Bridge between the SDK's `MessageHandler` callbacks and the platform event
/// channel + persisted state.
struct WechatHandlerBridge {
    event_tx: mpsc::Sender<PlatformEvent>,
    state: Arc<Mutex<WechatState>>,
    working_dir: PathBuf,
    media_dir: PathBuf,
    poll_cancel: CancellationToken,
    token_stale: Arc<AtomicBool>,
}

#[async_trait]
impl MessageHandler for WechatHandlerBridge {
    async fn on_message(&self, ctx: &MessageContext) -> weixin_agent::Result<()> {
        let peer = ctx.from.clone();
        if peer.is_empty() {
            return Ok(());
        }

        // Persist the peer's outbound context token (reply correlation).
        if let Some(token) = ctx.context_token.as_deref().filter(|t| !t.is_empty()) {
            let mut state = self.state.lock().await;
            state.set_context_token(&peer, token);
            if let Err(e) = state.save(&self.working_dir) {
                warn!("Failed to persist WeChat state: {}", e);
            }
        }

        let text = ctx.body.clone().unwrap_or_default();

        // WeChat CDN payloads are AES-encrypted: download + decrypt here (the
        // manager's generic URL download would fetch ciphertext) and hand over
        // `local_path` already set.
        let mut attachments: Vec<MediaAttachment> = Vec::new();
        if let Some(media) = ctx.media.as_ref() {
            match self.save_inbound_media(ctx, media).await {
                Ok(att) => attachments.push(att),
                Err(e) => {
                    warn!("Failed to download WeChat media from {}: {}", peer, e);
                }
            }
        }

        if text.is_empty() && attachments.is_empty() {
            return Ok(()); // nothing actionable (e.g. unsupported item type)
        }

        let chat_id = format!("private:{}", peer);
        let msg = ChatMessage {
            text,
            sender: SenderInfo {
                user_id: peer,
                chat_id,
                chat_type: ChatType::Private,
            },
            attachments,
        };

        let _ = self
            .event_tx
            .send(PlatformEvent::Message(msg))
            .await; // manager gone → drop silently
        Ok(())
    }

    async fn on_sync_buf_updated(&self, sync_buf: &str) -> weixin_agent::Result<()> {
        let mut state = self.state.lock().await;
        state.update_sync_buf(sync_buf);
        if let Err(e) = state.save(&self.working_dir) {
            warn!("Failed to persist WeChat sync cursor: {}", e);
        }
        Ok(())
    }

    async fn on_token_stale(&self, _info: &TokenStaleInfo) -> weixin_agent::Result<()> {
        // Terminal: polling can't recover without a fresh QR login.
        self.token_stale.store(true, Ordering::SeqCst);
        self.poll_cancel.cancel();
        let _ = self.event_tx.send(PlatformEvent::Disconnected).await;
        tracing::error!(
            "WeChat bot token is stale (errcode -14). Run `robit-wechat login` to \
             re-authenticate, then restart the bot."
        );
        Ok(())
    }
}

impl WechatHandlerBridge {
    /// Download + decrypt one inbound media item into the media directory.
    async fn save_inbound_media(
        &self,
        ctx: &MessageContext,
        media: &MediaInfo,
    ) -> std::result::Result<MediaAttachment, String> {
        tokio::fs::create_dir_all(&self.media_dir)
            .await
            .map_err(|e| format!("create media dir failed: {}", e))?;

        let filename = inbound_media_filename(media);
        let dest = self.media_dir.join(&filename);
        ctx.download_media(media, &dest)
            .await
            .map_err(|e| format!("CDN download failed: {}", e))?;
        debug!("Saved WeChat inbound media to {}", dest.display());

        Ok(MediaAttachment {
            content_type: mime_for_media(media.media_type),
            url: media.url.clone().unwrap_or_default(),
            filename: Some(filename),
            size: media.file_size,
            width: None,
            height: None,
            local_path: Some(dest),
        })
    }
}

/// MIME type for an inbound media item.
fn mime_for_media(media_type: MediaType) -> String {
    match media_type {
        MediaType::Image => "image/jpeg",
        MediaType::Video => "video/mp4",
        MediaType::Voice => "audio/silk",
        MediaType::File => "application/octet-stream",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Sanitized local filename for an inbound media item. A timestamp prefix
/// avoids collisions; the original name is kept for generic files.
fn inbound_media_filename(media: &MediaInfo) -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let base = match media.media_type {
        MediaType::File => media
            .file_name
            .as_deref()
            .map(sanitize_filename)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "file.bin".to_string()),
        MediaType::Image => "image.jpg".to_string(),
        MediaType::Video => "video.mp4".to_string(),
        MediaType::Voice => "voice.silk".to_string(),
        _ => "media.bin".to_string(),
    };
    format!("wx-{}-{}", ts, base)
}

/// Strip path separators and control characters from a filename, and drop
/// leading non-alphanumeric filler (e.g. `..\evil\a.txt` → `evil_a.txt`).
fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ' | '(' | ')') {
                c
            } else {
                '_'
            }
        })
        .collect();
    cleaned
        .trim()
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .to_string()
}

/// Extract the peer user ID from a platform chat_id.
fn peer_from_chat_id(chat_id: &str) -> std::result::Result<&str, AgentError> {
    chat_id.strip_prefix("private:").ok_or_else(|| {
        AgentError::InternalError(format!(
            "Invalid chat_id '{}': expected 'private:{{id}}' (WeChat is 1:1 only)",
            chat_id
        ))
    })
}

#[async_trait]
impl PlatformAdapter for WechatPlatformAdapter {
    fn capabilities() -> PlatformCaps {
        PlatformCaps::wechat()
    }

    fn platform_name() -> &'static str {
        "wechat"
    }

    async fn send_message(&self, chat_id: &str, text: &str) -> Result<SendResult> {
        let peer = peer_from_chat_id(chat_id)?;
        let token = self.outbound_context_token(peer).await;
        let result = self
            .client
            .send_text(peer, text, token.as_deref())
            .await
            .map_err(|e| AgentError::InternalError(format!("WeChat send failed: {}", e)))?;
        debug!("WeChat message sent to {} (id={})", chat_id, result.message_id);
        Ok(SendResult {
            msg_id: result.message_id,
        })
    }

    async fn upload_file(
        &self,
        _chat_id: &str,
        file_path: &str,
        _media_type: &str,
    ) -> Result<UploadResult> {
        // The WeChat protocol SDK uploads and sends media in one call
        // (`send_media`), so there is no standalone upload step. Return the
        // local path as the "URL" and let `send_media_message` consume it.
        if !Path::new(file_path).is_file() {
            return Err(AgentError::InternalError(format!(
                "File not found: {}",
                file_path
            )));
        }
        Ok(UploadResult {
            file_id: file_path.to_string(),
            url: file_path.to_string(),
        })
    }

    async fn send_media_message(
        &self,
        chat_id: &str,
        file_url: &str,
        _file_name: &str,
        _media_type: &str,
    ) -> Result<SendResult> {
        let peer = peer_from_chat_id(chat_id)?;
        let token = self.outbound_context_token(peer).await;
        let path = Path::new(file_url);
        let result = self
            .client
            .send_media(peer, path, token.as_deref())
            .await
            .map_err(|e| AgentError::InternalError(format!("WeChat media send failed: {}", e)))?;
        debug!("WeChat media sent to {} (id={})", chat_id, result.message_id);
        Ok(SendResult {
            msg_id: result.message_id,
        })
    }

    async fn recv_event(&self) -> Result<PlatformEvent> {
        self.event_rx
            .lock()
            .await
            .recv()
            .await
            .ok_or_else(|| AgentError::InternalError("WeChat event channel closed".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_then_caps() {
        assert_eq!(reconnect_backoff(1), Duration::from_secs(1));
        assert_eq!(reconnect_backoff(3), Duration::from_secs(4));
        assert_eq!(reconnect_backoff(6), Duration::from_secs(30));
        assert_eq!(reconnect_backoff(100), Duration::from_secs(30));
    }

    #[test]
    fn peer_extraction() {
        assert_eq!(peer_from_chat_id("private:wxid_1").unwrap(), "wxid_1");
        assert!(peer_from_chat_id("group:g1").is_err());
        assert!(peer_from_chat_id("bogus").is_err());
    }

    #[test]
    fn mime_mapping() {
        assert_eq!(mime_for_media(MediaType::Image), "image/jpeg");
        assert_eq!(mime_for_media(MediaType::Voice), "audio/silk");
    }

    #[test]
    fn sanitize_strips_path_separators() {
        assert_eq!(sanitize_filename("..\\evil\\a.txt"), "evil_a.txt");
        assert_eq!(sanitize_filename("对账单 v1.csv"), "对账单 v1.csv");
        assert_eq!(sanitize_filename(""), "");
    }

    #[test]
    fn from_config_requires_token() {
        let toml_str = r#"
            [providers.x]
            base_url = "https://x"
            api_key = "k"
            [[providers.x.models]]
            id = "m"
        "#;
        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        let dir = std::env::temp_dir().join("robit-wechat-cfg-test");
        // No [channels.wechat_bot] and no state file → error.
        assert!(WechatConfig::from_config(&config, &dir).is_err());

        // With a state file present the token is picked up.
        let mut state = WechatState::default();
        state.bot_token = Some("tok-from-login".into());
        state.save(&dir).unwrap();
        let cfg = WechatConfig::from_config(&config, &dir).unwrap();
        assert_eq!(cfg.bot_token, "tok-from-login");
        std::fs::remove_file(WechatState::path(&dir)).ok();
    }

    #[test]
    fn from_config_prefers_config_token() {
        let toml_str = r#"
            [providers.x]
            base_url = "https://x"
            api_key = "k"
            [[providers.x.models]]
            id = "m"
            [channels.wechat_bot]
            bot_token = "tok-from-config"
        "#;
        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        let dir = std::env::temp_dir().join("robit-wechat-cfg-test2");
        let mut state = WechatState::default();
        state.bot_token = Some("tok-from-login".into());
        state.save(&dir).unwrap();
        let cfg = WechatConfig::from_config(&config, &dir).unwrap();
        assert_eq!(cfg.bot_token, "tok-from-config");
        std::fs::remove_file(WechatState::path(&dir)).ok();
    }
}
