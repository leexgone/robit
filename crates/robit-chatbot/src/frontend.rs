//! Per-session Frontend implementation for Bot platforms.
//!
//! [`ChatbotFrontend`] implements [`robit_agent::frontend::Frontend`] for a
//! single chat (group or private). It buffers streaming `TextDelta` events and
//! flushes them in natural-boundary segments (so Markdown and code blocks
//! aren't cut mid-construct), sends a rate-limited progress hint when tools
//! run, and delegates tool confirmation to the shared [`Confirmer`].
//!
//! On platforms with edit support, the first message is sent then edited in
//! place to create a "growing message" effect; otherwise segments are sent as
//! separate messages.

use std::sync::Arc;

use async_trait::async_trait;
use robit_agent::error::Result;
use robit_agent::event::AgentEvent;
use robit_agent::frontend::Frontend;
use robit_agent::tool::ToolCallInfo;
use tokio::sync::Mutex;

use crate::adapter::{PlatformCaps, SendResult, UploadResult};
use crate::confirmer::Confirmer;
use crate::markdown::prepare_markdown_for_platform;
/// Abstracted message sending capability (platform-agnostic).
///
/// `ChatbotFrontend` talks to the platform through this trait rather than
/// `PlatformAdapter` directly, so the manager can supply a bridge that wraps
/// the concrete adapter.
#[async_trait]
pub trait PlatformSender: Send + Sync {
    /// Send a text message to a chat; returns the platform message ID.
    async fn send(&self, chat_id: &str, text: &str) -> Result<SendResult>;
    /// Edit a previously-sent message in place.
    async fn edit(&self, chat_id: &str, msg_id: &str, text: &str) -> Result<()>;
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
/// `TextDelta` events are buffered and flushed once at TurnComplete with
/// full Markdown sanitization to ensure clean output on platforms like QQ.
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
    /// Auto-approve all tool calls.
    pub auto_approve: bool,
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
            auto_approve,
        }
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
        let db = self.db.lock().await;
        match robit_agent::storage::insert_message(
            &db,
            &self.session_id,
            "user",
            content,
            None,
            None,
            None,
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

    /// Flush the buffer: take all accumulated text, sanitize it, send it, and save to DB.
    /// This ensures Markdown is parsed as a whole and we only send once per turn.
    async fn flush_buffer(&self) {
        let mut buffer = self.buffer.lock().await;
        if buffer.is_empty() {
            return;
        }
        let text = std::mem::take(&mut *buffer);
        drop(buffer);

        let caps = self.platform_sender.capabilities();

        let prepared = if caps.supports_markdown {
            prepare_markdown_for_platform(&text, &caps.markdown_features)
        } else {
            text.clone()
        };
        let prepared = truncate_to_max(&prepared, caps.max_message_length);

        // Just send once, no edit tricks - simple and reliable
        let mut last_msg_id = self.last_msg_id.lock().await;
        if caps.supports_edit && last_msg_id.is_some() {
            // Edit if we already sent something this turn (e.g., progress hint)
            let msg_id = last_msg_id.clone().unwrap();
            if self.platform_sender.edit(&self.chat_id, &msg_id, &prepared).await.is_err() {
                // Edit failed, fall back to send
                if let Ok(res) = self.platform_sender.send(&self.chat_id, &prepared).await {
                    *last_msg_id = Some(res.msg_id);
                }
            }
        } else {
            // No previous message this turn, just send
            if let Ok(res) = self.platform_sender.send(&self.chat_id, &prepared).await {
                *last_msg_id = Some(res.msg_id);
            }
        }

        // Save the original (un-truncated) message to database
        if !text.is_empty() {
            self.save_assistant_message(&text).await;
        }
    }

    /// Send a brief progress hint. Only sends once per turn to avoid spam.
    async fn send_progress_hint(&self, tool_name: &str) {
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
        if let Ok(res) = self.platform_sender.send(&self.chat_id, &hint).await {
            *self.last_msg_id.lock().await = Some(res.msg_id);
        }
    }
}

#[async_trait]
impl Frontend for ChatbotFrontend {
    async fn on_event(&self, event: AgentEvent) -> Result<()> {
        match event {
            AgentEvent::TextDelta(delta) => {
                self.append_delta(&delta).await;
            }
            AgentEvent::ToolCallRequested { name, .. } => {
                // Flush any buffered text before showing tool progress.
                self.flush_buffer().await;
                // In auto-approve mode, send a progress hint so the user knows
                // the bot is working. In manual mode, the Confirmer already
                // sends a confirm prompt — no extra hint needed.
                if self.auto_approve {
                    self.send_progress_hint(&name).await;
                }
            }
            AgentEvent::ToolCallResult { .. } => {
                // Silent: tool outputs are internal; the user only sees the
                // final text reply. Any progress hint is replaced by the reply
                // on TurnComplete.
            }
            AgentEvent::TurnComplete => {
                self.flush_buffer().await;
                // Reset per-turn state.
                *self.progress_hint_sent.lock().await = false;
                *self.last_msg_id.lock().await = None;
            }
            AgentEvent::Error(e) => {
                self.flush_buffer().await;
                let msg = format!("❌ Error: {}", e);
                let _ = self.platform_sender.send(&self.chat_id, &msg).await;
            }
            AgentEvent::SkillTriggered { .. } => {
                // Silent: skill trigger is internal; the skill's own output
                // arrives as TextDelta events.
            }
        }
        Ok(())
    }

    async fn request_tool_confirmation(&self, info: &ToolCallInfo) -> Result<bool> {
        self.confirmer
            .request(&self.chat_id, info, self.auto_approve)
            .await
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

/// Truncate `text` to `max` characters, appending an ellipsis if cut.
fn truncate_to_max(text: &str, max: usize) -> String {
    if max == 0 {
        return text.to_string();
    }
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::MarkdownFeatures;
    use std::sync::Mutex as StdMutex;

    struct MockSender {
        sent: StdMutex<Vec<(String, String)>>,
        edits: StdMutex<Vec<(String, String, String)>>,
        caps: PlatformCaps,
    }

    impl MockSender {
        fn new_with_edit(edit: bool) -> Arc<Self> {
            Arc::new(Self {
                sent: StdMutex::new(Vec::new()),
                edits: StdMutex::new(Vec::new()),
                caps: PlatformCaps {
                    supports_edit: edit,
                    returns_msg_id: true,
                    supports_markdown: true,
                    markdown_features: MarkdownFeatures::qq(),
                    max_message_length: 2000,
                    supports_images: true,
                    supports_files: true,
                    max_upload_size: 20 * 1024 * 1024,
                },
            })
        }
        fn sent_texts(&self) -> Vec<String> {
            self.sent.lock().unwrap().iter().map(|(_, t)| t.clone()).collect()
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
        async fn edit(&self, chat_id: &str, msg_id: &str, text: &str) -> Result<()> {
            self.edits.lock().unwrap().push((
                chat_id.to_string(),
                msg_id.to_string(),
                text.to_string(),
            ));
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

    fn make_frontend(sender: Arc<dyn PlatformSender>, auto_approve: bool) -> ChatbotFrontend {
        let confirmer = Arc::new(Confirmer::new(sender.clone(), std::time::Duration::from_secs(60)));
        // 创建一个内存中的 SQLite 连接用于测试
        let db = Arc::new(Mutex::new(rusqlite::Connection::open_in_memory().unwrap()));
        // 初始化数据库 schema
        {
            let db = db.blocking_lock();
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
        let fe = make_frontend(sender.clone(), false);
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
        let fe = make_frontend(sender.clone(), false);
        fe.on_event(AgentEvent::TextDelta("一段未被刷新的文本".to_string())).await.unwrap();
        assert!(sender.sent_texts().is_empty());
        fe.on_event(AgentEvent::TurnComplete).await.unwrap();
        assert_eq!(sender.sent_texts().len(), 1);
        assert!(sender.sent_texts()[0].contains("一段未被刷新的文本"));
    }

    #[tokio::test]
    async fn progress_hint_rate_limited_per_turn() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), true); // auto_approve
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
        let fe = make_frontend(sender.clone(), true);
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
        let fe = make_frontend(sender.clone(), false); // manual confirm
        fe.on_event(AgentEvent::ToolCallRequested {
            tool_call_id: "tc1".into(),
            name: "bash".into(),
            arguments: "{}".into(),
        }).await.unwrap();
        // No progress hint in manual mode (Confirmer sends the prompt instead).
        assert!(!sender.sent_texts().iter().any(|t| t.contains("正在")));
    }

    #[tokio::test]
    async fn error_sends_error_message() {
        let sender = MockSender::new_with_edit(false);
        let fe = make_frontend(sender.clone(), false);
        fe.on_event(AgentEvent::Error(robit_agent::AgentError::ToolError("boom".into())))
            .await
            .unwrap();
        assert!(sender.sent_texts().iter().any(|t| t.contains("Error") && t.contains("boom")));
    }

}
