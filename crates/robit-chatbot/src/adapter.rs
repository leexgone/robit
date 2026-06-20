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
    /// Whether the platform supports image upload and sending.
    pub supports_images: bool,
    /// Whether the platform supports file upload and sending.
    pub supports_files: bool,
    /// Maximum upload file size in bytes (0 = no limit).
    pub max_upload_size: u64,
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
    /// QQ Bot only supports very limited Markdown - use plain text fallback for most features.
    pub fn qq() -> Self {
        Self {
            headings: false,      // Not supported
            bold: false,          // Not supported
            italic: false,        // Not supported
            code_blocks: true,    // Supported
            inline_code: false,   // Not supported
            links: false,         // Not supported
            unordered_lists: false,
            ordered_lists: false,
            blockquotes: false,
            tables: false,
            task_lists: false,
            images: false,
            strikethrough: false,
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
    /// Note: QQ Bot has very limited Markdown support - we use the sanitizer to convert
    /// Markdown to readable plain text with only minimal formatting preserved.
    pub fn qq() -> Self {
        Self {
            supports_edit: true,
            returns_msg_id: true,
            supports_markdown: true, // Enable sanitizer
            markdown_features: MarkdownFeatures::qq(),
            max_message_length: 2000,
            supports_images: true,
            supports_files: true,
            max_upload_size: 20 * 1024 * 1024, // 20MB
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
            supports_images: true,
            supports_files: true,
            max_upload_size: 50 * 1024 * 1024, // 50MB
        }
    }
}

/// Result of sending a message.
#[derive(Debug, Clone)]
pub struct SendResult {
    pub msg_id: String,
}

/// Result of uploading a file to the platform.
#[derive(Debug, Clone)]
pub struct UploadResult {
    /// Platform-assigned file identifier (used for referencing in messages).
    pub file_id: String,
    /// Direct URL to the uploaded file.
    pub url: String,
}

/// A media attachment (image, file, etc.) received from the platform.
#[derive(Debug, Clone)]
pub struct MediaAttachment {
    /// MIME type: "image/jpeg", "image/png", "application/pdf", etc.
    pub content_type: String,
    /// Media URL (platform CDN download address).
    pub url: String,
    /// Original filename (if available).
    pub filename: Option<String>,
    /// File size in bytes (if available).
    pub size: Option<u64>,
    /// Image width in pixels (if available).
    pub width: Option<u32>,
    /// Image height in pixels (if available).
    pub height: Option<u32>,
}

impl MediaAttachment {
    /// Whether this attachment is an image.
    pub fn is_image(&self) -> bool {
        self.content_type.starts_with("image/")
    }

    /// A human-readable description for the LLM.
    pub fn describe(&self) -> String {
        let kind = if self.is_image() { "图片" } else { "文件" };
        let name = self
            .filename
            .as_deref()
            .unwrap_or_else(|| if self.is_image() { "image" } else { "file" });
        let size_str = self
            .size
            .map(|s| format!(" ({:.1}KB)", s as f64 / 1024.0))
            .unwrap_or_default();
        format!("[用户发送了{}: {}{}]", kind, name, size_str)
    }
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
    /// Media attachments (images, files) included in the message.
    pub attachments: Vec<MediaAttachment>,
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

    /// Upload a file to the platform. Returns the platform file URL/ID
    /// that can be referenced in subsequent messages.
    ///
    /// `file_path` is a local filesystem path to the file.
    /// `media_type` is a hint: "image" or "file".
    ///
    /// The default implementation returns an error — platforms that don't
    /// support file uploads can keep the default.
    async fn upload_file(
        &self,
        _chat_id: &str,
        _file_path: &str,
        _media_type: &str,
    ) -> Result<UploadResult> {
        Err(robit_agent::error::AgentError::InternalError(
            "File upload not supported on this platform".into(),
        ))
    }

    /// Send a media message (image/file) to a chat. Default implementation
    /// falls back to `send_message` with the URL as text.
    async fn send_media_message(
        &self,
        chat_id: &str,
        file_url: &str,
        file_name: &str,
        media_type: &str,
    ) -> Result<SendResult> {
        let _ = file_url;
        let label = if media_type == "image" { "📷" } else { "📎" };
        let text = format!("{} {}", label, file_name);
        self.send_message(chat_id, &text).await
    }

    /// Receive the next platform event. Blocks until an event arrives.
    async fn recv_event(&self) -> Result<PlatformEvent>;
}
