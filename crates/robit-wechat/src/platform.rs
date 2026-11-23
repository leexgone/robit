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
use robit_ai::config::{RateLimitSection, RobitConfig};
use robit_chatbot::adapter::{
    ChatMessage, ChatType, HintPolicy, MediaAttachment, PlatformAdapter, PlatformCaps,
    PlatformEvent, SendResult, SenderInfo, UploadResult,
};
use robit_chatbot::outbound::prepare_outbound;
use robit_chatbot::throttle::{SendLimiter, ThrottleConfig, ThrottleError, WaitMode};
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use weixin_agent::{
    MediaInfo, MediaType, MessageContext, MessageHandler, TokenStaleInfo, WeixinClient,
    WeixinConfig,
};

use crate::state::WechatState;
use crate::typing::{SdkTyping, TypingController};

/// Server-side anti-flood code: `{"ret":-2,"errmsg":"prepare failed"}`.
///
/// Per the protocol SDK's production notes, a burst of sends makes *every*
/// outbound call fail with this code for minutes, independently of
/// `context_token`; it is account level and survives restarts.
const ERRCODE_ANTI_FLOOD: i32 = -2;

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
    /// Account-level outbound send budget (conservative defaults, enabled).
    pub rate_limit: ThrottleConfig,
    /// Progress-hint policy override from config; `None` = platform default.
    pub progress_hint: Option<HintPolicy>,
    /// Native typing indicator switch (default `false`; flipped only after
    /// the real-device probe gate, design §11).
    pub typing: bool,
    /// Typing refresh cadence (`Duration::ZERO` = send once, no refresh).
    pub typing_refresh: Duration,
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

        let empty_limit = RateLimitSection::default();
        Ok(Self {
            bot_token,
            base_url: section
                .and_then(|s| s.base_url.clone())
                .or_else(|| state.as_ref().and_then(|s| s.base_url.clone())),
            cdn_base_url: section.and_then(|s| s.cdn_base_url.clone()),
            working_dir: working_dir.to_path_buf(),
            rate_limit: ThrottleConfig::from_section(
                section.map(|s| &s.rate_limit).unwrap_or(&empty_limit),
            ),
            progress_hint: section
                .and_then(|s| s.progress_hint.as_deref())
                .and_then(parse_hint_policy),
            typing: section.and_then(|s| s.typing).unwrap_or(false),
            typing_refresh: Duration::from_secs(
                section.and_then(|s| s.typing_refresh_secs).unwrap_or(5),
            ),
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
    /// Account-level outbound send budget. Shared by every outbound path
    /// (text, media, hints) so one quota domain is enforced per bot account.
    throttle: Arc<SendLimiter>,
    /// Cancels throttled waits during shutdown (Ctrl+C must never hang on a
    /// queued send).
    shutdown: CancellationToken,
    /// Where `wechat-state.json` lives; the send budget is persisted there.
    working_dir: PathBuf,
    /// Progress-hint policy override from config (`None` = platform default).
    progress_hint: Option<HintPolicy>,
    /// Native typing lifecycle (`None` = disabled by config). Policy only —
    /// protocol calls delegate to the SDK (see `crate::typing`).
    typing: Option<TypingController>,
}

/// Parse `[channels.wechat_bot].progress_hint`. Unrecognized values warn once
/// and fall back to the platform default (WeChat: `off`).
fn parse_hint_policy(raw: &str) -> Option<HintPolicy> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" | "none" | "never" => Some(HintPolicy::Off),
        "best_effort" | "best-effort" | "auto" => Some(HintPolicy::BestEffort),
        "always" | "on" => Some(HintPolicy::Always),
        other => {
            warn!(
                "Unknown [channels.wechat_bot].progress_hint '{}'; using the platform default",
                other
            );
            None
        }
    }
}

/// Whether a failed send is the platform's transient anti-flood rejection.
///
/// Deliberately narrow: only the explicit `-2` code counts. Other `Error::Api`
/// values include HTTP status codes synthesized by the SDK's `api_post` /
/// `api_get` paths (so a 502 is *not* a rate limit).
fn is_anti_flood(error: &weixin_agent::Error) -> bool {
    matches!(
        error,
        weixin_agent::Error::Api { errcode, .. } if *errcode == ERRCODE_ANTI_FLOOD
    )
}

/// Map our own throttling refusal to an agent-level error. Waiters that ran
/// out of budget or hit a cooldown are transient, so they surface as
/// [`AgentError::RateLimited`] for the host to decide about.
fn throttle_to_agent_error(error: ThrottleError) -> AgentError {
    match error {
        ThrottleError::Cancelled => {
            AgentError::InternalError("WeChat send cancelled during shutdown".to_string())
        }
        ThrottleError::Cooldown { retry_after } | ThrottleError::Saturated { retry_after } => {
            AgentError::RateLimited {
                retry_after_secs: Some(retry_after.as_secs().max(1)),
                message: format!("outbound send budget: {}", error),
            }
        }
    }
}

impl WechatPlatformAdapter {
    /// Build the adapter and start the long-poll loop behind a supervisor.
    ///
    /// The SDK poll loop already retries transport/API failures with its own
    /// backoff, so this supervisor only handles an unexpected loop exit
    /// (restart with backoff) and Ctrl+C (graceful stop via `shutdown`). A
    /// stale token is terminal: the bridge emits `Disconnected` so the manager
    /// exits and the user is told to re-run `robit-wechat login`.
    pub async fn connect(
        config: WechatConfig,
        shutdown: CancellationToken,
    ) -> Result<Arc<Self>> {
        let sdk_config = config
            .sdk_config(&config.bot_token)
            .map_err(AgentError::InternalError)?;

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

        // Restore the send budget: the anti-flood penalty is account level and
        // outlives the process, so an in-memory-only budget would be empty
        // exactly when a restart-triggered burst could hit the wall again.
        let throttle = Arc::new(SendLimiter::new(config.rate_limit.clone()));
        throttle.restore(&state.outbound).await;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if state.outbound.cooldown_until_ms > now_ms {
            warn!(
                "Restored WeChat outbound cooldown: ~{}s remaining (server anti-flood is account level and survives restarts)",
                (state.outbound.cooldown_until_ms - now_ms) / 1000
            );
        }

        let adapter = Arc::new(Self {
            client: Arc::clone(&client),
            event_rx: Mutex::new(event_rx),
            poll_cancel: poll_cancel.clone(),
            state: shared_state,
            throttle,
            shutdown: shutdown.clone(),
            working_dir: config.working_dir.clone(),
            progress_hint: config.progress_hint,
            typing: config.typing.then(|| {
                TypingController::new(
                    Arc::new(SdkTyping(Arc::clone(&client))),
                    config.typing_refresh,
                    shutdown.clone(),
                )
            }),
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

    /// Persist the send budget so a restart cannot replay a burst.
    ///
    /// Called once per send (≈ a handful of small file writes per minute) and
    /// on every anti-flood cooldown. Shares the state mutex with the inbound
    /// bridge, so writes are naturally serialized.
    async fn persist_throttle(&self) {
        let snapshot = self.throttle.snapshot().await;
        let mut state = self.state.lock().await;
        state.outbound = snapshot;
        if let Err(e) = state.save(&self.working_dir) {
            warn!("Failed to persist WeChat send budget: {}", e);
        }
    }

    /// Send text through the account-level budget.
    ///
    /// `WaitMode::Final` queues for a slot; `WaitMode::BestEffort` returns
    /// `Ok(None)` when the platform cannot take it right now.
    ///
    /// **Length guard**: text is transparently split into bounded segments
    /// via [`prepare_outbound`] before sending.  Each segment passes through
    /// the throttle independently.  On partial failure the remaining segments
    /// are abandoned and the last successfully sent `msg_id` is returned.
    async fn send_text_inner(
        &self,
        chat_id: &str,
        text: &str,
        mode: WaitMode,
    ) -> Result<Option<SendResult>> {
        let peer = peer_from_chat_id(chat_id)?.to_string();
        let caps = Self::capabilities();
        let outbound = prepare_outbound(text, &caps);

        let mut last_result: Option<SendResult> = None;

        for (i, segment) in outbound.segments.iter().enumerate() {
            if let Err(e) = self.throttle.reserve(mode, &self.shutdown).await {
                return match mode {
                    WaitMode::Final => {
                        if i > 0 {
                            // Some segments already sent; log and return what we have.
                            warn!(
                                "WeChat multi-segment send throttled after {}/{} segments for {}",
                                i, outbound.segments.len(), chat_id
                            );
                            Ok(last_result)
                        } else {
                            Err(throttle_to_agent_error(e))
                        }
                    }
                    WaitMode::BestEffort => {
                        debug!("WeChat send skipped for {} ({}): {}", chat_id, mode_label(mode), e);
                        Ok(last_result) // None if nothing sent yet
                    }
                };
            }
            self.persist_throttle().await;

            let token = self.outbound_context_token(&peer).await;
            match self.client.send_text(&peer, segment, token.as_deref()).await {
                Ok(result) => {
                    debug!(
                        "WeChat message sent to {} (id={}, segment={}/{})",
                        chat_id, result.message_id, i + 1, outbound.segments.len()
                    );
                    last_result = Some(SendResult {
                        msg_id: result.message_id,
                    });
                }
                Err(e) => {
                    let mapped = self.map_send_error(e).await;
                    if i > 0 {
                        // Partial failure: abandon remaining segments, return what we have.
                        warn!(
                            "WeChat multi-segment send failed at segment {}/{} for {}: {}; \
                             abandoning remaining segments",
                            i + 1, outbound.segments.len(), chat_id, mapped
                        );
                        return Ok(last_result);
                    }
                    return Err(mapped);
                }
            }
        }

        Ok(last_result)
    }

    /// Classify a failed outbound call.
    ///
    /// `-2` is the platform's transient anti-flood rejection: it starts a
    /// cooldown, is logged once per incident with actionable guidance, and
    /// surfaces as [`AgentError::RateLimited`] so hosts can tell it apart from
    /// permanent failures. Everything else stays a generic internal error.
    async fn map_send_error(&self, error: weixin_agent::Error) -> AgentError {
        if is_anti_flood(&error) {
            let cooldown = self.throttle.config().cooldown;
            let fresh = self.throttle.note_throttled(cooldown);
            self.persist_throttle().await;
            if fresh {
                warn!(
                    "WeChat outbound rejected by the server's anti-flood limit (ret=-2, \
                     'prepare failed'): this is an account-level rate limit, it self-heals \
                     within minutes, dense retries make it worse, and restarting the bot \
                     does NOT clear it (cooling down for {}s)",
                    cooldown.as_secs()
                );
            }
            return AgentError::RateLimited {
                retry_after_secs: Some(cooldown.as_secs()),
                message: format!("{}", error),
            };
        }
        AgentError::InternalError(format!("WeChat send failed: {}", error))
    }
}

/// Short label for a wait mode (logs only).
fn mode_label(mode: WaitMode) -> &'static str {
    match mode {
        WaitMode::Final => "final",
        WaitMode::BestEffort => "best-effort",
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
    shutdown: CancellationToken,
) {
    let mut failures: u32 = 0;
    loop {
        let sync_buf = state.lock().await.sync_buf.clone();
        let result = tokio::select! {
            res = client.start(sync_buf) => res,
            _ = shutdown.cancelled() => {
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

/// Sleep for `dur`, returning `true` if shutdown was cancelled meanwhile.
async fn sleep_or_shutdown(dur: Duration, shutdown: &CancellationToken) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(dur) => false,
        _ = shutdown.cancelled() => true,
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

    /// Per-channel override (config) of the platform default progress-hint
    /// policy; WeChat defaults to `off`.
    fn hint_policy(&self) -> HintPolicy {
        self.progress_hint
            .unwrap_or_else(|| Self::capabilities().progress_hints)
    }

    /// Per-channel typing policy: the config gate (default off), not the raw
    /// capability — WeChat supports typing but keeps it disabled until the
    /// operator enables it (probe-gated rollout, design §8).
    fn typing_enabled(&self) -> bool {
        self.typing.is_some()
    }

    /// Busy-state signal: resolve the peer's current context token and hand
    /// the transition to the typing controller, which spawns all HTTP work
    /// and returns promptly (trait contract). Typing bypasses the message
    /// send budget — sendtyping is a separate endpoint that produces no
    /// chat message; the controller paces itself (refresh cadence + breaker).
    async fn set_typing(&self, chat_id: &str, active: bool) -> Result<()> {
        let Some(controller) = &self.typing else {
            return Ok(());
        };
        let peer = peer_from_chat_id(chat_id)?;
        let token = self.outbound_context_token(peer).await;
        controller.set(peer, active, token).await;
        Ok(())
    }

    async fn send_message(&self, chat_id: &str, text: &str) -> Result<SendResult> {
        self.send_text_inner(chat_id, text, WaitMode::Final)
            .await?
            .ok_or_else(|| {
                AgentError::InternalError(
                    "WeChat send unexpectedly skipped under the final wait mode".to_string(),
                )
            })
    }

    async fn send_message_best_effort(
        &self,
        chat_id: &str,
        text: &str,
    ) -> Result<Option<SendResult>> {
        self.send_text_inner(chat_id, text, WaitMode::BestEffort)
            .await
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
        self.throttle
            .reserve(WaitMode::Final, &self.shutdown)
            .await
            .map_err(throttle_to_agent_error)?;
        self.persist_throttle().await;

        let token = self.outbound_context_token(peer).await;
        let path = Path::new(file_url);
        match self.client.send_media(peer, path, token.as_deref()).await {
            Ok(result) => {
                debug!("WeChat media sent to {} (id={})", chat_id, result.message_id);
                Ok(SendResult {
                    msg_id: result.message_id,
                })
            }
            Err(e) => Err(self.map_send_error(e).await),
        }
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
    fn hint_policy_config_parsing() {
        assert_eq!(parse_hint_policy("off"), Some(HintPolicy::Off));
        assert_eq!(parse_hint_policy(" Off "), Some(HintPolicy::Off));
        assert_eq!(parse_hint_policy("best_effort"), Some(HintPolicy::BestEffort));
        assert_eq!(parse_hint_policy("best-effort"), Some(HintPolicy::BestEffort));
        assert_eq!(parse_hint_policy("always"), Some(HintPolicy::Always));
        // Unknown values fall back to the platform default instead of failing.
        assert_eq!(parse_hint_policy("sometimes"), None);
    }

    #[test]
    fn anti_flood_is_only_errcode_minus_two() {
        let flood = weixin_agent::Error::Api {
            errcode: -2,
            errmsg: "prepare failed".to_string(),
        };
        assert!(is_anti_flood(&flood));

        // -14 (stale token), HTTP-status codes and zero must NOT be treated as
        // rate limiting: only the explicit anti-flood code is transient.
        for code in [-14, 0, 400, 429, 500] {
            let other = weixin_agent::Error::Api {
                errcode: code,
                errmsg: "no".to_string(),
            };
            assert!(!is_anti_flood(&other), "errcode {code} must not be a rate limit");
        }
    }

    #[test]
    fn from_config_applies_rate_limit_and_hint_overrides() {
        let toml_str = r#"
            [providers.x]
            base_url = "https://x"
            api_key = "k"
            [[providers.x.models]]
            id = "m"

            [channels.wechat_bot]
            bot_token = "tok"
            progress_hint = "best_effort"

            [channels.wechat_bot.rate_limit]
            window_sends = 6
            cooldown_secs = 45
        "#;
        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        let dir = std::env::temp_dir().join("robit-wechat-rate-limit-test");
        let cfg = WechatConfig::from_config(&config, &dir).unwrap();

        assert_eq!(cfg.rate_limit.window_sends, 6);
        assert_eq!(cfg.rate_limit.cooldown,
            Duration::from_secs(45));
        // Unspecified fields keep the documented defaults.
        assert!(cfg.rate_limit.enabled);
        assert_eq!(cfg.rate_limit.min_interval, Duration::from_millis(2_000));
        assert_eq!(cfg.rate_limit.window_len, Duration::from_secs(180));
        assert_eq!(cfg.progress_hint, Some(HintPolicy::BestEffort));
    }

    #[test]
    fn from_config_without_rate_limit_section_is_enabled_by_default() {
        let toml_str = r#"
            [providers.x]
            base_url = "https://x"
            api_key = "k"
            [[providers.x.models]]
            id = "m"

            [channels.wechat_bot]
            bot_token = "tok"
        "#;
        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        let dir = std::env::temp_dir().join("robit-wechat-rate-limit-default-test");
        let cfg = WechatConfig::from_config(&config, &dir).unwrap();
        assert_eq!(cfg.rate_limit, ThrottleConfig::default());
        assert!(cfg.progress_hint.is_none(), "no override → platform default (off)");
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

    #[test]
    fn from_config_typing_defaults_off() {
        let toml_str = r#"
            [providers.x]
            base_url = "https://x"
            api_key = "k"
            [[providers.x.models]]
            id = "m"
            [channels.wechat_bot]
            bot_token = "tok"
        "#;
        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        let dir = std::env::temp_dir().join("robit-wechat-typing-default-test");
        let cfg = WechatConfig::from_config(&config, &dir).unwrap();
        assert!(!cfg.typing, "typing must default to off (probe-gated rollout)");
        assert_eq!(cfg.typing_refresh, Duration::from_secs(5));
    }

    #[test]
    fn from_config_typing_override() {
        let toml_str = r#"
            [providers.x]
            base_url = "https://x"
            api_key = "k"
            [[providers.x.models]]
            id = "m"
            [channels.wechat_bot]
            bot_token = "tok"
            typing = true
            typing_refresh_secs = 0
        "#;
        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        let dir = std::env::temp_dir().join("robit-wechat-typing-override-test");
        let cfg = WechatConfig::from_config(&config, &dir).unwrap();
        assert!(cfg.typing);
        assert_eq!(cfg.typing_refresh, Duration::ZERO, "0 = send once, no refresh");
    }
}
