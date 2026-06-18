//! Platform adapter trait and supporting types.
//!
//! Platform crates (e.g. `robit-qq`) implement [`PlatformAdapter`] to bridge
//! a chat platform's connection, message sending, and event receiving into
//! the platform-agnostic `ChatbotManager`.

// NOTE: Full implementation lands in Phase 3. This stub keeps the crate compiling.

use async_trait::async_trait;
use robit_agent::error::Result;

/// Capabilities that vary by platform.
#[derive(Debug, Clone)]
pub struct PlatformCaps {
    pub supports_edit: bool,
    pub returns_msg_id: bool,
    pub supports_markdown: bool,
    pub markdown_features: MarkdownFeatures,
    pub max_message_length: usize,
}

/// Supported Markdown features for a platform.
#[derive(Debug, Clone, Default)]
pub struct MarkdownFeatures {
    pub headings: bool,
    pub bold: bool,
    pub italic: bool,
    pub code_blocks: bool,
    pub inline_code: bool,
    pub links: bool,
    pub unordered_lists: bool,
    pub ordered_lists: bool,
    pub blockquotes: bool,
    pub tables: bool,
    pub task_lists: bool,
    pub images: bool,
    pub strikethrough: bool,
}

impl MarkdownFeatures {
    /// QQ Official Bot Markdown feature subset.
    pub fn qq() -> Self {
        Self {
            headings: true,
            bold: true,
            italic: true,
            code_blocks: true,
            inline_code: true,
            links: true,
            unordered_lists: true,
            ordered_lists: true,
            blockquotes: true,
            tables: false,
            task_lists: false,
            images: false,
            strikethrough: true,
        }
    }

    /// Feishu Markdown features (reserved for future use).
    pub fn feishu() -> Self {
        Self {
            headings: true,
            bold: true,
            italic: true,
            code_blocks: true,
            inline_code: true,
            links: true,
            unordered_lists: true,
            ordered_lists: true,
            blockquotes: true,
            tables: true,
            task_lists: true,
            images: true,
            strikethrough: true,
        }
    }
}

impl PlatformCaps {
    /// QQ Official Bot capabilities.
    pub fn qq() -> Self {
        Self {
            supports_edit: true,
            returns_msg_id: true,
            supports_markdown: true,
            markdown_features: MarkdownFeatures::qq(),
            max_message_length: 2000,
        }
    }

    /// Feishu capabilities (reserved for future use).
    pub fn feishu() -> Self {
        Self {
            supports_edit: true,
            returns_msg_id: true,
            supports_markdown: true,
            markdown_features: MarkdownFeatures::feishu(),
            max_message_length: 30000,
        }
    }
}

/// Result of sending a message.
#[derive(Debug, Clone)]
pub struct SendResult {
    pub msg_id: String,
}

/// Sender information extracted from a platform event.
#[derive(Debug, Clone)]
pub struct SenderInfo {
    pub user_id: String,
    pub chat_id: String,
    pub chat_type: ChatType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatType {
    Private,
    Group,
}

/// A parsed chat message from the platform.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub text: String,
    pub sender: SenderInfo,
}

/// Platform events that `ChatbotManager` processes.
#[derive(Debug)]
pub enum PlatformEvent {
    Message(ChatMessage),
    Disconnected,
    Other(serde_json::Value),
}

/// The trait every chat platform must implement.
///
/// Connection lifecycle (WebSocket/HTTP setup, auth, background tasks) is the
/// platform crate's responsibility — it constructs a connected adapter and
/// hands it to [`ChatbotManager`](crate::manager::ChatbotManager) as an
/// `Arc<Self>`. This avoids the tension between a `connect() -> Self` trait
/// method and adapters that spawn background tasks holding `Arc<Self>`.
#[async_trait]
pub trait PlatformAdapter: Send + Sync + 'static {
    /// Platform capabilities. Used by `ChatbotFrontend` for streaming strategy
    /// and by the Markdown sanitizer.
    fn capabilities() -> PlatformCaps;

    /// Send a text message to a chat. Returns the platform message ID.
    async fn send_message(&self, chat_id: &str, text: &str) -> Result<SendResult>;

    /// Edit a previously-sent message. Default implementation falls back
    /// to `send_message` for platforms that don't support editing.
    async fn edit_message(&self, chat_id: &str, _msg_id: &str, text: &str) -> Result<()> {
        let _ = self.send_message(chat_id, text).await;
        Ok(())
    }

    /// Receive the next platform event. Blocks until an event arrives.
    async fn recv_event(&self) -> Result<PlatformEvent>;
}
