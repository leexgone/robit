//! Per-session Frontend implementation for Bot platforms.
//!
//! [`ChatbotFrontend`] implements [`robit_agent::frontend::Frontend`] for a
//! single chat (group or private). It buffers streaming `TextDelta` events and
//! flushes them in natural-boundary segments (so Markdown and code blocks
//! aren't cut mid-construct), sends a progress hint when tools run, and
//! delegates tool confirmation to the shared [`Confirmer`].
//!
//! On platforms with edit support, the first message is sent then edited in
//! place to create a "growing message" effect; otherwise segments are sent as
//! separate messages, bounded per flush (see `MAX_OUTBOUND_SEGMENTS`).
//!
//! # Outbound inventory
//!
//! Every message a Bot sends goes through one of these paths — all of them end
//! up in the platform adapter, so a platform-level send budget covers them:
//!
//! | Path | Trigger | Must deliver? |
//! |---|---|---|
//! | `send_progress_hint` | first tool call of a turn (auto-approve only, once per turn) | no (policy-dependent) |
//! | `flush_buffer` at `ToolCallRequested` | buffered text before a tool call | yes |
//! | `flush_buffer` at `TurnComplete` | final answer | yes |
//! | `flush_buffer` at `Error` | partial answer before an error | yes |
//! | Async-task hint | `ToolCallResult` with `is_pending` | yes |
//! | Error notice | `AgentEvent::Error` | yes |
//! | [`Confirmer::request`] | confirmation prompt, decision notice, timeout notice (manual mode) | yes |
//! | Slash-command replies (`/clear`, `/stop`, `/cancel`) | manager command handling | yes |
//! | Media sends (`send_media_message`) | `send_file` tool | yes |
//!
//! The progress hint is the only optional one, which is why it is the only
//! path that can be skipped under send-budget pressure.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use robit_agent::error::Result;
use robit_agent::event::AgentEvent;
use robit_agent::frontend::Frontend;
use robit_agent::tool::ToolCallInfo;
use tokio::sync::Mutex;

use crate::adapter::{HintPolicy, PlatformCaps, SendResult, UploadResult};
use crate::confirmer::Confirmer;
/// Abstracted message sending capability (platform-agnostic).
///
/// `ChatbotFrontend` talks to the platform through this trait rather than
/// `PlatformAdapter` directly, so the manager can supply a bridge that wraps
/// the concrete adapter.
#[async_trait]
pub trait PlatformSender: Send + Sync {
    /// Send a text message to a chat; returns the platform message ID.
    async fn send(&self, chat_id: &str, text: &str) -> Result<SendResult>;
    /// Send a text message that may be skipped when the platform cannot take
    /// it right now (`Ok(None)` = skipped, which is acceptable for hints).
    ///
    /// The default implementation is an ordinary [`Self::send`], so the
    /// behavior of every existing sender is unchanged.
    async fn send_best_effort(
        &self,
        chat_id: &str,
        text: &str,
    ) -> Result<Option<SendResult>> {
        self.send(chat_id, text).await.map(Some)
    }
    /// Edit a previously-sent message in place.
    async fn edit(&self, chat_id: &str, msg_id: &str, text: &str) -> Result<()>;
    /// Signal the chat's busy state (native typing indicator). Idempotent,
    /// best-effort, and must return promptly (see
    /// [`crate::adapter::PlatformAdapter::set_typing`] for the contract).
    /// Default: no-op — platforms without a native indicator are unchanged.
    async fn set_typing(&self, _chat_id: &str, _active: bool) -> Result<()> {
        Ok(())
    }
    /// Upload a file to the platform. Returns the platform file URL/ID.
    async fn upload_file(&self, chat_id: &str, file_path: &str, media_type: &str) -> Result<UploadResult>;
    /// Send a media message (image/file) to a chat.
    async fn send_media_message(&self, chat_id: &str, file_url: &str, file_name: &str, media_type: &str) -> Result<SendResult>;
    /// Platform capabilities (drives streaming strategy).
    fn capabilities(&self) -> PlatformCaps;
}

/// Platform extension for file/media operations.
///
/// Exposed to tools via `ToolContext.extensions` under key `"chatbot.platform_ext"`.
/// `ChatbotFrontend` implements this by delegating to its `PlatformSender`,
/// using the frontend's own `chat_id` internally.
#[async_trait]
pub trait PlatformExt: Send + Sync {
    /// Upload a local file to the platform. Returns the platform identifier.
    async fn upload_file(&self, file_path: &str, media_type: &str) -> Result<UploadResult>;
    /// Send an already-uploaded media file to the chat.
    async fn send_media_message(&self, file_url: &str, file_name: &str, media_type: &str) -> Result<SendResult>;
}

/// Per-session Frontend trait implementation for Bot platforms.
///
/// Each chat (group or private) gets its own `ChatbotFrontend` instance.
/// `TextDelta` events are buffered and flushed once at TurnComplete through
/// the shared outbound guard (content projection + length guard), so every
/// platform gets readable output — including plain-text platforms, whose
/// all-false features produce a plain-text projection.
pub struct ChatbotFrontend {
    /// The chat this frontend belongs to (`group:{id}` or `private:{id}`).
    pub chat_id: String,
    /// The session ID for database storage.
    pub session_id: String,
    /// Platform message sender (shared across all frontends).
    pub platform_sender: Arc<dyn PlatformSender>,
    /// Tool confirmation coordinator (shared).
    pub confirmer: Arc<Confirmer>,
    /// Database connection for persisting messages.
    pub db: Arc<Mutex<rusqlite::Connection>>,
    /// Buffer to accumulate text until TurnComplete.
    pub buffer: Mutex<String>,
    /// ID of the last message sent (for edit-based updates, e.g., replacing a
    /// progress hint with the actual response).
    pub last_msg_id: Mutex<Option<String>>,
    /// Whether a progress hint has already been sent this turn (rate limit).
    pub progress_hint_sent: Mutex<bool>,
    /// How progress hints should be delivered on this platform
    /// (see [`HintPolicy`]). Defaults to [`HintPolicy::Always`]; the manager
    /// overrides it with the platform's policy.
    pub hint_policy: HintPolicy,
    /// Resolved typing policy (manager wires `platform.typing_enabled()`).
    /// `false` = the platform hook is never called.
    pub typing_enabled: bool,
    /// Desired typing state; dedupes hook calls (agent events re-assert
    /// constantly, the platform must only see transitions).
    typing_on: Mutex<bool>,
    /// ThinkingDelta events seen this turn, counted for a single end-of-turn
    /// summary log (per-delta logging would flood the log file).
    pub thinking_deltas: AtomicUsize,
    /// Total bytes of ThinkingDelta text seen this turn (same purpose).
    pub thinking_bytes: AtomicUsize,
    /// Auto-approve all tool calls.
    pub auto_approve: bool,
    /// True while a turn is in flight (first delta/tool event → TurnComplete
    /// or Error). Shared with the manager's `AgentHandle` so the idle-session
    /// cleanup never evicts a session mid-turn (H3).
    pub busy: Arc<AtomicBool>,
}

impl ChatbotFrontend {
    /// Create a new `ChatbotFrontend` for `chat_id`.
    pub fn new(
        chat_id: String,
        session_id: String,
        platform_sender: Arc<dyn PlatformSender>,
        confirmer: Arc<Confirmer>,
        db: Arc<Mutex<rusqlite::Connection>>,
        auto_approve: bool,
    ) -> Self {
        Self {
            chat_id,
            session_id,
            platform_sender,
            confirmer,
            db,
            buffer: Mutex::new(String::new()),
            last_msg_id: Mutex::new(None),
            progress_hint_sent: Mutex::new(false),
            hint_policy: HintPolicy::Always,
            typing_enabled: false,
            typing_on: Mutex::new(false),
            thinking_deltas: AtomicUsize::new(0),
            thinking_bytes: AtomicUsize::new(0),
            auto_approve,
            busy: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Override the platform's default progress-hint policy.
    pub fn with_hint_policy(mut self, policy: HintPolicy) -> Self {
        self.hint_policy = policy;
        self
    }

    /// Override the typing policy (manager wires `platform.typing_enabled()`).
    pub fn with_typing_enabled(mut self, enabled: bool) -> Self {
        self.typing_enabled = enabled;
        self
    }

    /// Transition the desired typing state; calls the platform hook only on
    /// change. Never fails the turn: hook errors are logged and swallowed.
    async fn set_typing_state(&self, on: bool) {
        if !self.typing_enabled {
            return;
        }
        let mut cur = self.typing_on.lock().await;
        if *cur == on {
            return;
        }
        *cur = on;
        tracing::debug!(
            "[chatbot] typing {}: chat_id='{}'",
            if on { "on" } else { "off" },
            self.chat_id
        );
        if let Err(e) = self.platform_sender.set_typing(&self.chat_id, on).await {
            tracing::warn!(
                "[chatbot] set_typing({}) failed: chat_id='{}', error={}",
                on, self.chat_id, e
            );
        }
    }

    /// Light the busy indicator right after a user turn was successfully
    /// injected (manager calls this). Earliest correct point — covers the
    /// LLM first-token latency before any agent event arrives.
    pub async fn notify_turn_started(&self) {
        self.set_typing_state(true).await;
    }

    /// Save an assistant message to the database.
    async fn save_assistant_message(&self, content: &str) {
        let db = self.db.lock().await;
        // Don't save empty messages - they cause problems with LLM API
        if content.is_empty() {
            tracing::debug!("Not saving empty assistant message to DB");
            return;
        }
        match robit_agent::storage::insert_message(
            &db,
            &self.session_id,
            "assistant",
            content,
            None,
            None,
            None,
            None,
        ) {
            Ok(_) => {
                tracing::debug!("Saved assistant message to DB: session_id={}", self.session_id);
                let _ = robit_agent::storage::touch_session(&db, &self.session_id);
            }
            Err(e) => tracing::warn!("Failed to save assistant message: {}", e),
        }
    }

    /// Save a user message to the database (called from manager).
    pub async fn save_user_message(&self, content: &str) {
        self.save_user_message_with_source(content, None).await;
    }

    /// Save a user message with an origin marker. `source` is `None` for real
    /// platform messages, or a label (e.g. "scheduler") for host-injected
    /// turns so the archive can distinguish them (H1).
    pub async fn save_user_message_with_source(&self, content: &str, source: Option<&str>) {
        let db = self.db.lock().await;
        match robit_agent::storage::insert_message(
            &db,
            &self.session_id,
            "user",
            content,
            None,
            None,
            None,
            source,
        ) {
            Ok(_) => {
                tracing::debug!("Saved user message to DB: session_id={}", self.session_id);
                let _ = robit_agent::storage::touch_session(&db, &self.session_id);
            }
            Err(e) => tracing::warn!("Failed to save user message: {}", e),
        }
    }

    /// Append a delta to the buffer (no streaming send, just accumulate).
    /// For QQ Bot, we send the full sanitized message at TurnComplete to
    /// avoid duplicate messages and ensure proper Markdown handling.
    async fn append_delta(&self, delta: &str) {
        let mut buffer = self.buffer.lock().await;
        buffer.push_str(delta);
    }

    /// Emit one log line summarizing this turn's (unrendered) thinking deltas,
    /// then reset the counters. No-op if no thinking arrived.
    fn log_thinking_summary(&self) {
        let deltas = self.thinking_deltas.swap(0, Ordering::Relaxed);
        let bytes = self.thinking_bytes.swap(0, Ordering::Relaxed);
        if deltas > 0 {
            tracing::trace!(
                "[chatbot] thinking deltas received (not rendered): chat_id='{}', deltas={}, bytes={}",
                self.chat_id, deltas, bytes
            );
        }
    }

    /// Flush the buffer: take all accumulated text, sanitize it, send it, and save to DB.
    /// This ensures Markdown is parsed as a whole and we only send once per turn.
    async fn flush_buffer(&self) {
        let mut buffer = self.buffer.lock().await;
        if buffer.is_empty() {
            return;
        }
        let text = std::mem::take(&mut *buffer);
        drop(buffer);

        tracing::trace!(
            "[chatbot] flush_buffer: chat_id='{}', text_len={}",
            self.chat_id,
            text.len()
        );

        let caps = self.platform_sender.capabilities();

        // Shared outbound guard: content projection (platform readability)
        // followed by the length guard. Projection runs on EVERY platform —
        // all-false features mean plain-text projection, not "skip". It is
        // idempotent, so the adapter calling `prepare_outbound` again on each
        // segment is a byte-level no-op.
        let outbound = prepare_outbound(&text, &caps);

        let mut last_msg_id = self.last_msg_id.lock().await;

        if caps.supports_edit {
            // Edit-capable platforms keep the single-message model: the first
            // message of the turn is edited in place into the final answer.
            let prepared = &outbound.segments[0];
            match last_msg_id.clone() {
                Some(msg_id) => {
                    if self
                        .platform_sender
                        .edit(&self.chat_id, &msg_id, prepared)
                        .await
                        .is_err()
                    {
                        // Edit failed, fall back to send.
                        if let Ok(res) = self.platform_sender.send(&self.chat_id, prepared).await {
                            *last_msg_id = Some(res.msg_id);
                        }
                    }
                }
                None => match self.platform_sender.send(&self.chat_id, prepared).await {
                    Ok(res) => {
                        *last_msg_id = Some(res.msg_id);
                        tracing::trace!(
                            "[chatbot] reply sent: chat_id='{}', msg_len={}",
                            self.chat_id,
                            prepared.len()
                        );
                    }
                    Err(e) => tracing::warn!(
                        "[chatbot] failed to send reply to platform: chat_id='{}', error={}",
                        self.chat_id,
                        e
                    ),
                },
            }
        } else {
            // No in-place edit: a long answer must be split into bounded
            // segments. Truncating a single message would silently drop the
            // tail of the reply.
            if outbound.truncated {
                tracing::warn!(
                    "[chatbot] reply truncated after {} segments: chat_id='{}', text_len={}",
                    MAX_OUTBOUND_SEGMENTS,
                    self.chat_id,
                    text.chars().count()
                );
            }
            for segment in &outbound.segments {
                match self.platform_sender.send(&self.chat_id, segment).await {
                    Ok(res) => {
                        *last_msg_id = Some(res.msg_id);
                        tracing::trace!(
                            "[chatbot] reply segment sent: chat_id='{}', msg_len={}",
                            self.chat_id,
                            segment.len()
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[chatbot] failed to send reply segment: chat_id='{}', error={}",
                            self.chat_id,
                            e
                        );
                        break;
                    }
                }
            }
        }

        // Save the original (un-truncated) message to database
        if !text.is_empty() {
            self.save_assistant_message(&text).await;
        }
    }

    /// Send a brief progress hint. Only sends once per turn to avoid spam.
    ///
    /// Delivery follows [`ChatbotFrontend::hint_policy`]: `Off` skips entirely,
    /// `BestEffort` skips silently when the platform's send budget is
    /// saturated, `Always` is an ordinary (queued) send.
    async fn send_progress_hint(&self, tool_name: &str) {
        if self.hint_policy == HintPolicy::Off {
            tracing::trace!(
                "[chatbot] progress hint suppressed by policy: chat_id='{}', tool='{}'",
                self.chat_id, tool_name
            );
            return;
        }

        let mut sent = self.progress_hint_sent.lock().await;
        if *sent {
            return;
        }
        *sent = true;
        drop(sent);

        let hint = match tool_name {
            "bash" => "🔧 正在执行命令...".to_string(),
            "read" => "📖 正在读取文件...".to_string(),
            "write" => "✏️ 正在写入文件...".to_string(),
            "edit" => "✏️ 正在编辑文件...".to_string(),
            "grep" => "🔍 正在搜索...".to_string(),
            "find" => "🔍 正在查找...".to_string(),
            _ => "🔧 正在处理...".to_string(),
        };

        if self.hint_policy == HintPolicy::BestEffort {
            match self.platform_sender.send_best_effort(&self.chat_id, &hint).await {
                Ok(Some(res)) => {
                    *self.last_msg_id.lock().await = Some(res.msg_id);
                    tracing::trace!(
                        "[chatbot] progress hint sent: chat_id='{}', tool='{}'",
                        self.chat_id, tool_name
                    );
                }
                Ok(None) => tracing::trace!(
                    "[chatbot] progress hint skipped (platform throttled): chat_id='{}', tool='{}'",
                    self.chat_id, tool_name
                ),
                Err(e) => tracing::warn!(
                    "[chatbot] failed to send progress hint: chat_id='{}', tool='{}', error={}",
                    self.chat_id, tool_name, e
                ),
            }
            return;
        }

        match self.platform_sender.send(&self.chat_id, &hint).await {
            Ok(res) => {
                *self.last_msg_id.lock().await = Some(res.msg_id);
                tracing::trace!(
                    "[chatbot] progress hint sent: chat_id='{}', tool='{}'",
                    self.chat_id, tool_name
                );
            }
            Err(e) => tracing::warn!(
                "[chatbot] failed to send progress hint: chat_id='{}', tool='{}', error={}",
                self.chat_id, tool_name, e
            ),
        }
    }
}

#[async_trait]
impl Frontend for ChatbotFrontend {
    async fn on_event(&self, event: AgentEvent) -> Result<()> {
        match event {
            AgentEvent::TextDelta(delta) => {
                self.busy.store(true, Ordering::Relaxed);
                self.set_typing_state(true).await;
                self.append_delta(&delta).await;
            }
            AgentEvent::ThinkingDelta(delta) => {
                self.busy.store(true, Ordering::Relaxed);
                self.set_typing_state(true).await;
                // Silent by design: thinking text is long-form and would
                // flood the platform's message segmentation (QQ 等消息长度
                // 受限)，Bot 面本轮不渲染思考过程。
                // Count only — a per-delta trace line floods the log file;
                // `log_thinking_summary` emits one line at end of turn.
                self.thinking_deltas.fetch_add(1, Ordering::Relaxed);
                self.thinking_bytes.fetch_add(delta.len(), Ordering::Relaxed);
            }
            AgentEvent::ToolCallRequested { name, .. } => {
                self.busy.store(true, Ordering::Relaxed);
                self.set_typing_state(true).await;
                tracing::trace!(
                    "[chatbot] ToolCallRequested received: chat_id='{}', tool='{}' (auto_approve={})",
                    self.chat_id, name, self.auto_approve
                );
                // Flush any buffered text before showing tool progress.
                self.flush_buffer().await;
                // In auto-approve mode, send a progress hint so the user knows
                // the bot is working. In manual mode, the Confirmer already
                // sends a confirm prompt — no extra hint needed.
                if self.auto_approve {
                    self.send_progress_hint(&name).await;
                }
            }
            AgentEvent::ToolCallResult { tool_call_id, ref result } => {
                // Async task started: in manual mode (no progress hint was
                // sent for this call) tell the user the task is running in the
                // background. In auto-approve mode the ToolCallRequested hint
                // already covered "working on it".
                if result.is_pending && !self.auto_approve {
                    let task_id = result.pending_task_id.as_deref().unwrap_or("?");
                    let msg = format!(
                        "🎨 后台任务已启动(task_id={}),完成后自动通知",
                        task_id
                    );
                    if let Err(e) = self.platform_sender.send(&self.chat_id, &msg).await {
                        tracing::warn!(
                            "[chatbot] failed to send async-task hint: chat_id='{}', error={}",
                            self.chat_id, e
                        );
                    }
                }
                // Tool outputs are internal; the user only sees the final text
                // reply. Traced for diagnostics.
                tracing::trace!(
                    "[chatbot] ToolCallResult received: chat_id='{}', tool_call_id='{}', is_pending={}, is_error={}, content_len={}",
                    self.chat_id, tool_call_id, result.is_pending, result.is_error, result.content.len()
                );
            }
            AgentEvent::TurnComplete => {
                tracing::trace!(
                    "[chatbot] TurnComplete received: chat_id='{}', flushing buffered reply",
                    self.chat_id
                );
                self.flush_buffer().await;
                self.log_thinking_summary();
                // Reset per-turn state.
                *self.progress_hint_sent.lock().await = false;
                *self.last_msg_id.lock().await = None;
                self.busy.store(false, Ordering::Relaxed);
                self.set_typing_state(false).await;
            }
            AgentEvent::Error(e) => {
                tracing::trace!(
                    "[chatbot] Error received: chat_id='{}', error={}",
                    self.chat_id, e
                );
                self.flush_buffer().await;
                self.log_thinking_summary();
                self.busy.store(false, Ordering::Relaxed);
                self.set_typing_state(false).await;
                let msg = format!("❌ Error: {}", e);
                if let Err(send_err) = self.platform_sender.send(&self.chat_id, &msg).await {
                    tracing::warn!(
                        "[chatbot] failed to send error message to platform: chat_id='{}', error={}",
                        self.chat_id, send_err
                    );
                }
            }
            AgentEvent::SkillTriggered { ref name, .. } => {
                tracing::trace!(
                    "[chatbot] SkillTriggered received (silent by design): chat_id='{}', skill='{}'",
                    self.chat_id, name
                );
            }
            AgentEvent::AsyncToolCompleted {
                task_id,
                tool_call_id,
                result,
            } => {
                // Silent: the Agent wakes the LLM, whose reply is delivered
                // via TextDelta and flushed to the chat as usual. Traced so
                // completion is observable in logs.
                tracing::trace!(
                    "[chatbot] AsyncToolCompleted: chat_id='{}', task_id='{}', tool_call_id='{}', is_error={}",
                    self.chat_id, task_id, tool_call_id, result.is_error
                );
            }
        }
        Ok(())
    }

    async fn request_tool_confirmation(&self, info: &ToolCallInfo) -> Result<bool> {
        if self.auto_approve {
            // Immediate decision — pausing here would only flicker the
            // indicator (cancel + re-send) for no user-visible benefit.
            return self
                .confirmer
                .request(&self.chat_id, info, self.auto_approve)
                .await;
        }
        // Manual mode: while waiting for the user's reply the agent is idle,
        // so the busy indicator must not claim "typing". Re-light after the
        // decision (approve / reject / timeout alike); the turn-end event
        // turns it off again.
        self.set_typing_state(false).await;
        let decision = self
            .confirmer
            .request(&self.chat_id, info, self.auto_approve)
            .await;
        self.set_typing_state(true).await;
        decision
    }
}

#[async_trait]
impl PlatformExt for ChatbotFrontend {
    async fn upload_file(&self, file_path: &str, media_type: &str) -> Result<UploadResult> {
        self.platform_sender
            .upload_file(&self.chat_id, file_path, media_type)
            .await
    }

    async fn send_media_message(
        &self,
        file_url: &str,
        file_name: &str,
        media_type: &str,
    ) -> Result<SendResult> {
        self.platform_sender
            .send_media_message(&self.chat_id, file_url, file_name, media_type)
            .await
    }
}

// Outbound length handling is delegated to the public `outbound` module so
// that adapter direct-send paths share the exact same logic.  The constants
// and helpers that used to live here are now in `crate::outbound`.
use crate::outbound::{prepare_outbound, MAX_OUTBOUND_SEGMENTS};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::MarkdownFeatures;
    use std::sync::Mutex as StdMutex;

    struct MockSender {
        sent: StdMutex<Vec<(String, String)>>,
        edits: StdMutex<Vec<(String, String, String)>>,
        caps: PlatformCaps,
        /// When true, `send_best_effort` reports "skipped" (budget saturated).
        skip_best_effort: bool,
        /// Recorded `set_typing` transitions, in order.
        typing: StdMutex<Vec<(String, bool)>>,
        /// When true, `set_typing` records the call then returns an error.
        fail_typing: bool,
    }

    impl MockSender {
        fn new_with_edit(edit: bool) -> Arc<Self> {
            Self::new_full(edit, false)
        }

        fn new_full(edit: bool, skip_best_effort: bool) -> Arc<Self> {
            Arc::new(Self {
                sent: StdMutex::new(Vec::new()),
                edits: StdMutex::new(Vec::new()),
                skip_best_effort,
                typing: StdMutex::new(Vec::new()),
                fail_typing: false,
                caps: PlatformCaps {
                    supports_edit: edit,
                    returns_msg_id: true,
                    supports_markdown: true,
                    markdown_features: MarkdownFeatures::qq(),
                    supports_images: true,
                    supports_files: true,
                    max_upload_size: 20 * 1024 * 1024,
                    progress_hints: HintPolicy::Always,
                    ..Default::default()
                },
            })
        }
        /// WeChat-style sender: plain-text caps, no in-place edit.
        fn new_wechat() -> Arc<Self> {
            Arc::new(Self {
                sent: StdMutex::new(Vec::new()),
                edits: StdMutex::new(Vec::new()),
                skip_best_effort: false,
                typing: StdMutex::new(Vec::new()),
                fail_typing: false,
                caps: PlatformCaps::wechat(),
            })
        }
        /// Sender whose typing hook always fails (best-effort contract test).
        fn new_failing_typing() -> Arc<Self> {
            Arc::new(Self {
                sent: StdMutex::new(Vec::new()),
                edits: StdMutex::new(Vec::new()),
                skip_best_effort: false,
                typing: StdMutex::new(Vec::new()),
                fail_typing: true,
                caps: PlatformCaps::qq(),
            })
        }
        fn sent_texts(&self) -> Vec<String> {
            self.sent.lock().unwrap().iter().map(|(_, t)| t.clone()).collect()
        }
        fn typing_calls(&self) -> Vec<(String, bool)> {
            self.typing.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl PlatformSender for MockSender {
        async fn send(&self, chat_id: &str, text: &str) -> Result<SendResult> {
            self.sent
                .lock()
                .unwrap()
                .push((chat_id.to_string(), text.to_string()));
            Ok(SendResult {
                msg_id: format!("msg-{}", self.sent.lock().unwrap().len()),
            })
        }
        async fn send_best_effort(
            &self,
            chat_id: &str,
            text: &str,
        ) -> Result<Option<SendResult>> {
            if self.skip_best_effort {
                return Ok(None);
            }
            self.send(chat_id, text).await.map(Some)
        }
        async fn edit(&self, chat_id: &str, msg_id: &str, text: &str) -> Result<()> {
            self.edits.lock().unwrap().push((
                chat_id.to_string(),
                msg_id.to_string(),
                text.to_string(),
            ));
            Ok(())
        }
        async fn set_typing(&self, chat_id: &str, active: bool) -> Result<()> {
            self.typing.lock().unwrap().push((chat_id.to_string(), active));
            if self.fail_typing {
                return Err(robit_agent::AgentError::InternalError(
                    "typing unavailable".into(),
                ));
            }
            Ok(())
        }
        async fn upload_file(&self, _chat_id: &str, _file_path: &str, _media_type: &str) -> Result<UploadResult> {
            Ok(UploadResult {
                file_id: "mock-file-id".into(),
                url: "/mock/upload.png".into(),
            })
        }
        async fn send_media_message(
            &self,
            chat_id: &str,
            _file_url: &str,
            file_name: &str,
            media_type: &str,
        ) -> Result<SendResult> {
            self.sent
                .lock()
                .unwrap()
                .push((chat_id.to_string(), format!("[media:{}] {}", media_type, file_name)));
            Ok(SendResult {
                msg_id: format!("msg-{}", self.sent.lock().unwrap().len()),
            })
        }
        fn capabilities(&self) -> PlatformCaps {
            self.caps.clone()
        }
    }

    async fn make_frontend(sender: Arc<dyn PlatformSender>, auto_approve: bool) -> ChatbotFrontend {
        let confirmer = Arc::new(Confirmer::new(sender.clone(), std::time::Duration::from_secs(60)));
        // 创建一个内存中的 SQLite 连接用于测试
        let db = Arc::new(Mutex::new(rusqlite::Connection::open_in_memory().unwrap()));
        // 初始化数据库 schema
        {
            let db = db.lock().await;
            robit_agent::storage::init_db(&db).unwrap();
        }
        ChatbotFrontend::new(
            "group:1".to_string(),
            "test-session-1".to_string(),
            sender,
            confirmer,
            db,
            auto_approve
        )
    }

    #[tokio::test]
    async fn textdelta_accumulates_until_turn_complete() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), false).await;
        // Text deltas only accumulate, nothing is sent until TurnComplete.
        fe.on_event(AgentEvent::TextDelta("你好".to_string())).await.unwrap();
        fe.on_event(AgentEvent::TextDelta("世界".to_string())).await.unwrap();
        assert!(sender.sent_texts().is_empty());
        // Nothing sent until TurnComplete.
        fe.on_event(AgentEvent::TurnComplete).await.unwrap();
        assert_eq!(sender.sent_texts().len(), 1);
        assert!(sender.sent_texts()[0].contains("你好世界"));
    }

    #[tokio::test]
    async fn turn_complete_flushes_accumulated_text() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), false).await;
        fe.on_event(AgentEvent::TextDelta("一段未被刷新的文本".to_string())).await.unwrap();
        assert!(sender.sent_texts().is_empty());
        fe.on_event(AgentEvent::TurnComplete).await.unwrap();
        assert_eq!(sender.sent_texts().len(), 1);
        assert!(sender.sent_texts()[0].contains("一段未被刷新的文本"));
    }

    #[tokio::test]
    async fn progress_hint_rate_limited_per_turn() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), true).await; // auto_approve
        // Two tool calls in one turn — only one hint should be sent.
        fe.on_event(AgentEvent::ToolCallRequested {
            tool_call_id: "tc1".into(),
            name: "bash".into(),
            arguments: "{}".into(),
        }).await.unwrap();
        fe.on_event(AgentEvent::ToolCallRequested {
            tool_call_id: "tc2".into(),
            name: "read".into(),
            arguments: "{}".into(),
        }).await.unwrap();
        let hints: Vec<_> = sender
            .sent_texts()
            .into_iter()
            .filter(|t| t.contains("正在"))
            .collect();
        assert_eq!(hints.len(), 1);
    }

    #[tokio::test]
    async fn progress_hint_resets_on_turn_complete() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), true).await;
        fe.on_event(AgentEvent::ToolCallRequested {
            tool_call_id: "tc1".into(),
            name: "bash".into(),
            arguments: "{}".into(),
        }).await.unwrap();
        fe.on_event(AgentEvent::TurnComplete).await.unwrap();
        // After TurnComplete, a new tool call should send another hint.
        fe.on_event(AgentEvent::ToolCallRequested {
            tool_call_id: "tc2".into(),
            name: "bash".into(),
            arguments: "{}".into(),
        }).await.unwrap();
        let hints: Vec<_> = sender
            .sent_texts()
            .into_iter()
            .filter(|t| t.contains("正在"))
            .collect();
        assert_eq!(hints.len(), 2);
    }

    #[tokio::test]
    async fn no_hint_in_manual_mode() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), false).await; // manual confirm
        fe.on_event(AgentEvent::ToolCallRequested {
            tool_call_id: "tc1".into(),
            name: "bash".into(),
            arguments: "{}".into(),
        }).await.unwrap();
        // No progress hint in manual mode (Confirmer sends the prompt instead).
        assert!(!sender.sent_texts().iter().any(|t| t.contains("正在")));
    }

    #[tokio::test]
    async fn hint_policy_off_suppresses_hint_but_keeps_answer() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), true)
            .await
            .with_hint_policy(HintPolicy::Off);
        fe.on_event(AgentEvent::ToolCallRequested {
            tool_call_id: "tc1".into(),
            name: "bash".into(),
            arguments: "{}".into(),
        }).await.unwrap();
        fe.on_event(AgentEvent::TextDelta("答案".into())).await.unwrap();
        fe.on_event(AgentEvent::TurnComplete).await.unwrap();

        let texts = sender.sent_texts();
        assert!(!texts.iter().any(|t| t.contains("正在")), "hint must not be sent: {texts:?}");
        assert_eq!(texts.len(), 1, "the final answer is still delivered");
        assert!(texts[0].contains("答案"));
    }

    #[tokio::test]
    async fn hint_policy_best_effort_skips_when_throttled() {
        let sender = MockSender::new_full(false, true); // reports "skipped"
        let fe = make_frontend(sender.clone(), true)
            .await
            .with_hint_policy(HintPolicy::BestEffort);
        fe.on_event(AgentEvent::ToolCallRequested {
            tool_call_id: "tc1".into(),
            name: "bash".into(),
            arguments: "{}".into(),
        }).await.unwrap();
        fe.on_event(AgentEvent::TextDelta("答案".into())).await.unwrap();
        fe.on_event(AgentEvent::TurnComplete).await.unwrap();

        let texts = sender.sent_texts();
        assert!(!texts.iter().any(|t| t.contains("正在")), "hint must be skipped: {texts:?}");
        assert_eq!(texts.len(), 1);
        assert!(texts[0].contains("答案"));
    }

    #[tokio::test]
    async fn hint_policy_best_effort_sends_when_budget_allows() {
        let sender = MockSender::new_full(false, false); // "send" succeeds
        let fe = make_frontend(sender.clone(), true)
            .await
            .with_hint_policy(HintPolicy::BestEffort);
        fe.on_event(AgentEvent::ToolCallRequested {
            tool_call_id: "tc1".into(),
            name: "bash".into(),
            arguments: "{}".into(),
        }).await.unwrap();
        assert!(sender.sent_texts().iter().any(|t| t.contains("正在")));
    }

    // ------------------------------------------------------------------
    // Outbound segmentation (platforms without in-place edit)
    // These tests exercise the same logic via the public `outbound` module.
    // ------------------------------------------------------------------

    fn non_edit_test_caps(max_len: usize) -> PlatformCaps {
        PlatformCaps {
            returns_msg_id: true,
            markdown_features: MarkdownFeatures::wechat(),
            max_message_length: max_len,
            supports_images: true,
            supports_files: true,
            max_upload_size: 20 * 1024 * 1024,
            ..Default::default()
        }
    }

    #[test]
    fn split_keeps_short_text_in_one_segment() {
        let caps = non_edit_test_caps(10);
        let result = prepare_outbound("你好", &caps);
        assert_eq!(result.segments, vec!["你好".to_string()]);
        assert!(!result.truncated);
    }

    #[test]
    fn split_prefers_paragraph_boundaries() {
        let caps = non_edit_test_caps(200);
        let text = format!("{}\n\n{}", "A".repeat(120), "B".repeat(120));
        let result = prepare_outbound(&text, &caps);
        assert!(!result.truncated);
        assert_eq!(result.segments.len(), 2);
        assert!(result.segments[0].ends_with("\n\n"));
        assert_eq!(result.segments.concat(), text);
        assert!(result.segments.iter().all(|s| s.chars().count() <= 200));
    }

    #[test]
    fn split_caps_segment_count_and_marks_truncation() {
        let caps = non_edit_test_caps(100);
        let text = "X".repeat(700);
        let result = prepare_outbound(&text, &caps);
        assert!(result.truncated);
        assert_eq!(result.segments.len(), 3);
        assert!(result.segments.last().unwrap().ends_with("已截断）"));
        assert!(result.segments.iter().all(|s| s.chars().count() <= 100));
    }

    #[test]
    fn split_does_not_cut_inside_a_closed_code_fence() {
        // QQ features keep fences through projection; the split heuristic
        // must still avoid cutting inside them.
        let caps = PlatformCaps {
            markdown_features: MarkdownFeatures::qq(),
            ..non_edit_test_caps(220)
        };
        let text = format!("```\n{}\n```\n\n{}", "x".repeat(170), "z".repeat(100));
        let result = prepare_outbound(&text, &caps);
        assert!(
            result.segments[0].contains("```"),
            "fence stays in the first segment"
        );
        assert!(result.segments[0].ends_with("\n\n"));
    }

    #[test]
    fn split_hard_cuts_unbroken_text() {
        let caps = non_edit_test_caps(200);
        let text = "A".repeat(500);
        let result = prepare_outbound(&text, &caps);
        assert_eq!(result.segments[0].chars().count(), 200);
        // All original content preserved across segments (minus truncation marker).
        let concat: String = result.segments.iter()
            .map(|s| s.replace(crate::outbound::TRUNCATION_MARKER, ""))
            .collect();
        assert_eq!(concat, text);
    }

    #[test]
    fn split_empty_text_yields_single_empty_segment() {
        let caps = non_edit_test_caps(200);
        let result = prepare_outbound("", &caps);
        assert_eq!(result.segments, vec!["".to_string()]);
        assert!(!result.truncated);
    }

    #[tokio::test]
    async fn long_reply_is_split_into_bounded_segments_without_edit() {
        let sender = MockSender::new_with_edit(false); // 2000-char cap, no edit
        let fe = make_frontend(sender.clone(), false).await;
        fe.on_event(AgentEvent::TextDelta("a".repeat(7000))).await.unwrap();
        fe.on_event(AgentEvent::TurnComplete).await.unwrap();

        let texts = sender.sent_texts();
        assert_eq!(texts.len(), 3, "7000 chars must not become a message storm");
        assert_eq!(texts[0].chars().count(), 2000);
        assert!(texts.last().unwrap().contains("已截断"));
        assert!(texts.iter().all(|t| t.chars().count() <= 2000));
    }

    #[tokio::test]
    async fn long_reply_stays_one_message_when_edit_is_supported() {
        let sender = MockSender::new_with_edit(true);
        let fe = make_frontend(sender.clone(), false).await;
        fe.on_event(AgentEvent::TextDelta("a".repeat(7000))).await.unwrap();
        fe.on_event(AgentEvent::TurnComplete).await.unwrap();

        let texts = sender.sent_texts();
        // QQ-style platforms keep the single-message behavior (zero regression).
        assert_eq!(texts.len(), 1);
        assert!(texts[0].chars().count() <= 2000);
        assert!(texts[0].ends_with('…'));
        assert!(sender.edits.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn edit_platform_merges_progress_hint_into_the_answer() {
        let sender = MockSender::new_with_edit(true);
        let fe = make_frontend(sender.clone(), true).await; // auto-approve → hint
        fe.on_event(AgentEvent::ToolCallRequested {
            tool_call_id: "tc1".into(),
            name: "bash".into(),
            arguments: "{}".into(),
        }).await.unwrap();
        fe.on_event(AgentEvent::TextDelta("最终答案".into())).await.unwrap();
        fe.on_event(AgentEvent::TurnComplete).await.unwrap();

        // One message (the hint), edited in place into the answer: no extra
        // message is produced, which is why hints are free on these platforms.
        assert_eq!(sender.sent_texts().len(), 1);
        let edits = sender.edits.lock().unwrap();
        assert_eq!(edits.len(), 1);
        assert!(edits[0].2.contains("最终答案"));
    }

    // ------------------------------------------------------------------
    // Plain-text platform projection (T2: WeChat no longer skips the
    // projection; all-false features = plain-text projection).
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn wechat_table_reply_is_linearized() {
        let sender = MockSender::new_wechat();
        let fe = make_frontend(sender.clone(), false).await;
        let md = "| 持仓 | 现价 | 盈亏 | 仓位 |\n|---|---|---|---|\n| 贵州茅台 | 105.00 | -4.59% | 28.0% |\n";
        fe.on_event(AgentEvent::TextDelta(md.into())).await.unwrap();
        fe.on_event(AgentEvent::TurnComplete).await.unwrap();

        let texts = sender.sent_texts();
        assert_eq!(texts.len(), 1);
        assert_eq!(
            texts[0],
            "贵州茅台 — 现价: 105.00 · 盈亏: -4.59% · 仓位: 28.0%\n"
        );
        assert!(!texts[0].contains('|'), "raw pipe syntax must not leak");
    }

    #[tokio::test]
    async fn wechat_plain_reply_is_byte_exact() {
        // Plain text takes the fast path: no trailing newline, no changes.
        let sender = MockSender::new_wechat();
        let fe = make_frontend(sender.clone(), false).await;
        fe.on_event(AgentEvent::TextDelta("你好世界".into())).await.unwrap();
        fe.on_event(AgentEvent::TurnComplete).await.unwrap();
        assert_eq!(sender.sent_texts(), vec!["你好世界".to_string()]);
    }

    #[tokio::test]
    async fn wechat_markdown_reply_is_projected_to_plain_text() {
        let sender = MockSender::new_wechat();
        let fe = make_frontend(sender.clone(), false).await;
        let md = "# 标题\n\n**加粗** 与 `code`\n\n```\nlet x = 1;\n```\n";
        fe.on_event(AgentEvent::TextDelta(md.into())).await.unwrap();
        fe.on_event(AgentEvent::TurnComplete).await.unwrap();

        let texts = sender.sent_texts();
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0], "标题\n\n加粗 与 code\n\nlet x = 1;\n");
    }

    #[tokio::test]
    async fn error_sends_error_message() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), false).await;
        fe.on_event(AgentEvent::Error(robit_agent::AgentError::ToolError("boom".into())))
            .await
            .unwrap();
        assert!(sender.sent_texts().iter().any(|t| t.contains("Error") && t.contains("boom")));
    }

    // ------------------------------------------------------------------
    // Typing (busy-state signal) lifecycle
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn typing_lights_on_turn_start_and_off_on_complete() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), false).await.with_typing_enabled(true);
        fe.notify_turn_started().await;
        fe.on_event(AgentEvent::TextDelta("你好".into())).await.unwrap();
        fe.on_event(AgentEvent::TextDelta("世界".into())).await.unwrap();
        fe.on_event(AgentEvent::TurnComplete).await.unwrap();
        assert_eq!(
            sender.typing_calls(),
            vec![("group:1".to_string(), true), ("group:1".to_string(), false)]
        );
    }

    #[tokio::test]
    async fn typing_off_on_error() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), false).await.with_typing_enabled(true);
        fe.notify_turn_started().await;
        fe.on_event(AgentEvent::Error(robit_agent::AgentError::ToolError("boom".into())))
            .await
            .unwrap();
        assert_eq!(
            sender.typing_calls(),
            vec![("group:1".to_string(), true), ("group:1".to_string(), false)]
        );
    }

    #[tokio::test]
    async fn typing_off_even_when_answer_is_empty() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), false).await.with_typing_enabled(true);
        fe.notify_turn_started().await;
        // No deltas at all: flush sends nothing, but the indicator must still
        // be cancelled — turn end must not depend on a reply being delivered.
        fe.on_event(AgentEvent::TurnComplete).await.unwrap();
        assert!(sender.sent_texts().is_empty());
        assert_eq!(
            sender.typing_calls(),
            vec![("group:1".to_string(), true), ("group:1".to_string(), false)]
        );
    }

    #[tokio::test]
    async fn typing_dedupes_repeated_events() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), true).await.with_typing_enabled(true);
        fe.notify_turn_started().await;
        for i in 0..5 {
            fe.on_event(AgentEvent::TextDelta(format!("d{i}"))).await.unwrap();
            fe.on_event(AgentEvent::ToolCallRequested {
                tool_call_id: format!("tc{i}"),
                name: "bash".into(),
                arguments: "{}".into(),
            }).await.unwrap();
        }
        fe.on_event(AgentEvent::TurnComplete).await.unwrap();
        // Exactly one transition each way despite 10 re-asserting events.
        assert_eq!(
            sender.typing_calls(),
            vec![("group:1".to_string(), true), ("group:1".to_string(), false)]
        );
    }

    #[tokio::test]
    async fn typing_disabled_never_calls_the_hook() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), true).await; // default: disabled
        fe.notify_turn_started().await;
        fe.on_event(AgentEvent::TextDelta("答案".into())).await.unwrap();
        fe.on_event(AgentEvent::TurnComplete).await.unwrap();
        assert!(sender.typing_calls().is_empty());
        // The answer path is untouched.
        assert_eq!(sender.sent_texts().len(), 1);
    }

    #[tokio::test]
    async fn typing_hook_error_does_not_fail_the_turn() {
        let sender = MockSender::new_failing_typing();
        let fe = make_frontend(sender.clone(), false).await.with_typing_enabled(true);
        fe.notify_turn_started().await;
        fe.on_event(AgentEvent::TextDelta("答案".into())).await.unwrap();
        fe.on_event(AgentEvent::TurnComplete).await.unwrap();
        // Hook errors are swallowed; the answer is still delivered.
        assert_eq!(sender.sent_texts(), vec!["答案".to_string()]);
        assert_eq!(sender.typing_calls().len(), 2);
    }

    #[tokio::test]
    async fn typing_no_pause_in_auto_approve_mode() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), true).await.with_typing_enabled(true);
        fe.notify_turn_started().await;
        let info = ToolCallInfo {
            id: "tc1".into(),
            name: "bash".into(),
            arguments: "{}".into(),
        };
        assert!(fe.request_tool_confirmation(&info).await.unwrap());
        // Immediate decision: no cancel/re-light flicker.
        assert_eq!(sender.typing_calls(), vec![("group:1".to_string(), true)]);
    }

    #[tokio::test]
    async fn typing_pauses_while_awaiting_manual_confirmation() {
        let sender = MockSender::new_with_edit(false);
        let fe = Arc::new(
            make_frontend(sender.clone(), false).await.with_typing_enabled(true),
        );
        fe.notify_turn_started().await;
        let info = ToolCallInfo {
            id: "tc1".into(),
            name: "bash".into(),
            arguments: "{}".into(),
        };
        let fe2 = Arc::clone(&fe);
        let handle = tokio::spawn(async move { fe2.request_tool_confirmation(&info).await });

        // Wait for the confirmation prompt; by then typing must be paused.
        let mut prompt_seen = false;
        for _ in 0..400 {
            if !sender.sent_texts().is_empty() {
                prompt_seen = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(prompt_seen, "confirmation prompt must be sent");
        assert_eq!(
            sender.typing_calls(),
            vec![("group:1".to_string(), true), ("group:1".to_string(), false)],
            "typing must be off while waiting for the user"
        );

        // Approve; the decision re-lights typing (turn end turns it off later).
        let mut approved = None;
        for _ in 0..400 {
            if let Some(v) = fe.confirmer.check_confirmation_response("group:1", "确认") {
                approved = Some(v);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(approved, Some(true));
        assert!(handle.await.unwrap().unwrap());
        assert_eq!(
            sender.typing_calls(),
            vec![
                ("group:1".to_string(), true),
                ("group:1".to_string(), false),
                ("group:1".to_string(), true),
            ]
        );
    }

    #[test]
    fn platform_caps_default_baseline_and_constructors() {
        let d = PlatformCaps::default();
        assert!(!d.supports_edit && !d.supports_typing);
        assert_eq!(d.progress_hints, HintPolicy::Off);
        assert_eq!(d.max_message_length, 2000);
        // Capability declarations (regression pin for QQ/Feishu/WeChat).
        assert!(!PlatformCaps::qq().supports_typing);
        assert!(!PlatformCaps::feishu().supports_typing);
        assert!(PlatformCaps::wechat().supports_typing);
        assert!(PlatformCaps::qq().supports_edit);
        assert!(!PlatformCaps::wechat().supports_edit);
    }

}
