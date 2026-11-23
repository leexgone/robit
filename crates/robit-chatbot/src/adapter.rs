//! Platform adapter trait and supporting types.
//!
//! Platform crates (e.g. `robit-qq`) implement [`PlatformAdapter`] to bridge
//! a chat platform's connection, message sending, and event receiving into
//! the platform-agnostic `ChatbotManager`.

// NOTE: Full implementation lands in Phase 3. This stub keeps the crate compiling.

use async_trait::async_trait;
use robit_agent::error::Result;

/// How a platform wants the "bot is working" progress hint to be delivered.
///
/// The hint is a plain text message sent once per turn, before the first tool
/// call, so the user is not staring at silence during a long tool run. Its
/// cost differs per platform: on a platform with in-place edit the hint message
/// is later *edited* into the final answer (net zero extra messages), while on
/// a platform without edit it stays in the chat as an extra message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HintPolicy {
    /// Always send it (typical for platforms that support edit).
    #[default]
    Always,
    /// Send only when the outbound budget has room; skip silently otherwise.
    BestEffort,
    /// Never send it (typical for platforms where it would be a real extra
    /// message and count against an anti-flood budget).
    Off,
}

/// Capabilities that vary by platform.
///
/// `#[non_exhaustive]` + [`Default`]: construct via the platform constructors
/// (`qq()` / `feishu()` / `wechat()`) or `Default` + field overrides. Adding
/// capability fields is non-breaking from now on.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PlatformCaps {
    pub supports_edit: bool,
    pub returns_msg_id: bool,
    /// Whether the platform natively renders Markdown. **Descriptive only**:
    /// the outbound projection (`outbound::prepare_outbound`) runs on every
    /// platform regardless — all-false `markdown_features` means plain-text
    /// projection, not "skip projection".
    pub supports_markdown: bool,
    pub markdown_features: MarkdownFeatures,
    pub max_message_length: usize,
    /// Whether the platform supports image upload and sending.
    pub supports_images: bool,
    /// Whether the platform supports file upload and sending.
    pub supports_files: bool,
    /// Maximum upload file size in bytes (0 = no limit).
    pub max_upload_size: u64,
    /// How progress hints should be delivered on this platform.
    pub progress_hints: HintPolicy,
    /// Native ephemeral "busy/typing" status (e.g. WeChat iLink sendtyping).
    /// Descriptive capability; the per-channel policy is
    /// [`PlatformAdapter::typing_enabled`].
    pub supports_typing: bool,
}

impl Default for PlatformCaps {
    /// Conservative baseline: plain text, no edit, no media, hints off,
    /// no typing. Platform constructors override what they support.
    fn default() -> Self {
        Self {
            supports_edit: false,
            returns_msg_id: false,
            supports_markdown: false,
            markdown_features: MarkdownFeatures::default(),
            max_message_length: 2000,
            supports_images: false,
            supports_files: false,
            max_upload_size: 0,
            progress_hints: HintPolicy::Off,
            supports_typing: false,
        }
    }
}

/// Supported Markdown features for a platform.
///
/// The projection ([`prepare_markdown_for_platform`](crate::markdown::prepare_markdown_for_platform))
/// consumes these flags: supported syntax passes through, unsupported syntax
/// degrades to readable plain text (tables linearize into records, code
/// fences strip while content is kept, links keep their URL as `text (url)`,
/// headings/quotes/strikethrough lose their markers). List markers `- ` /
/// `N. ` are always emitted as a natural plain-text convention regardless of
/// the list flags.
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

    /// WeChat iLink Bot renders plain text, so no Markdown feature is
    /// advertised. Note the protocol SDK's `filter_markdown` does **not**
    /// strip everything — it deliberately preserves tables, code fences and
    /// bold — so robit projects Markdown to plain text *before* the SDK sees
    /// it (all-false features = plain-text projection in `markdown.rs`).
    pub fn wechat() -> Self {
        Self::default()
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
            supports_markdown: true, // limited native rendering; projection still runs
            markdown_features: MarkdownFeatures::qq(),
            max_message_length: 2000,
            supports_images: true,
            supports_files: true,
            max_upload_size: 20 * 1024 * 1024, // 20MB
            // QQ supports in-place edit: the hint message becomes the answer,
            // so it costs no extra message.
            progress_hints: HintPolicy::Always,
            // No typing API: "正在输入" is a human-client feature, not
            // exposed to official bots.
            supports_typing: false,
            ..Default::default()
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
            progress_hints: HintPolicy::Always,
            // No typing API; streaming cards are the future native path.
            supports_typing: false,
            ..Default::default()
        }
    }

    /// WeChat iLink Bot capabilities: plain-text rendering, no in-place edit
    /// (a reply is a new message), media upload/send supported via the
    /// protocol SDK's CDN path.
    pub fn wechat() -> Self {
        Self {
            supports_edit: false,
            returns_msg_id: true,
            supports_markdown: false, // plain text; robit projects before the SDK
            markdown_features: MarkdownFeatures::wechat(),
            max_message_length: 2000,
            supports_images: true,
            supports_files: true,
            max_upload_size: 20 * 1024 * 1024, // 20MB
            // No in-place edit: a hint stays as a separate chat message and
            // consumes one unit of the server-side anti-flood budget.
            progress_hints: HintPolicy::Off,
            // iLink sendtyping: native ephemeral busy indicator, no message
            // quota. Per-channel policy: `typing_enabled()` (config gate).
            supports_typing: true,
            ..Default::default()
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
    /// Local copy saved by the manager after download. Platform adapters
    /// leave this `None`; the manager fills it in before routing to the Agent.
    pub local_path: Option<std::path::PathBuf>,
}

impl MediaAttachment {
    /// Whether this attachment is an image.
    pub fn is_image(&self) -> bool {
        self.content_type.starts_with("image/")
    }
}

// Convert to agent's MediaAttachment
impl From<MediaAttachment> for robit_agent::event::MediaAttachment {
    fn from(att: MediaAttachment) -> Self {
        Self {
            content_type: att.content_type,
            url: att.url,
            filename: att.filename,
            size: att.size,
            width: att.width,
            height: att.height,
            local_path: att.local_path,
        }
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

    /// Platform identifier recorded as the session `source` in the archive
    /// (e.g. `"qq"`, `"feishu"`). Must be stable across restarts.
    fn platform_name() -> &'static str;

    /// Send a text message to a chat. Returns the platform message ID.
    ///
    /// # Outbound preparation contract (content projection + length guard)
    ///
    /// Implementations **must** prepare outbound text by calling
    /// [`prepare_outbound`](crate::outbound::prepare_outbound) before sending.
    /// The guard applies two layers, in order:
    ///
    /// 1. **Content projection** — outbound text is treated as Markdown
    ///    source and projected to [`PlatformCaps::markdown_features`]:
    ///    tables become linearized records, unsupported syntax degrades to
    ///    readable plain text. Text without Markdown syntax passes through
    ///    byte-for-byte, and already-projected text is stable (idempotent).
    ///    Hosts pushing literal plain text should avoid Markdown-significant
    ///    character pairs (e.g. `2*3*4`), which are interpreted as emphasis
    ///    by design.
    /// 2. **Length guard** — [`PlatformCaps::max_message_length`] is enforced
    ///    on the projected text:
    ///    - **Non-edit platforms** (`supports_edit == false`): long text is
    ///      automatically split into at most
    ///      [`MAX_OUTBOUND_SEGMENTS`](crate::outbound::MAX_OUTBOUND_SEGMENTS)
    ///      boundary-aware segments.  Content beyond the budget carries an
    ///      explicit truncation marker.
    ///    - **Edit platforms** (`supports_edit == true`): long text is
    ///      truncated to the cap with an ellipsis, preserving the
    ///      single-message editing model.
    ///
    /// Callers (including host direct-push paths) can rely on these
    /// guarantees and do **not** need to pre-project, pre-truncate or
    /// pre-split text themselves.
    async fn send_message(&self, chat_id: &str, text: &str) -> Result<SendResult>;

    /// Send a text message that may be **skipped** when the platform cannot
    /// take it right now (outbound budget saturated, cooldown active).
    ///
    /// `Ok(None)` means "not sent, and that is acceptable" — used for
    /// non-essential traffic (progress hints, notices) so that optional
    /// messages never delay or displace the ones that must be delivered.
    ///
    /// The same outbound preparation contract (content projection + length
    /// guard) as [`send_message`](Self::send_message) applies.
    ///
    /// The default implementation is an ordinary [`Self::send_message`], so
    /// platforms without a send budget keep their previous behavior exactly.
    async fn send_message_best_effort(
        &self,
        chat_id: &str,
        text: &str,
    ) -> Result<Option<SendResult>> {
        self.send_message(chat_id, text).await.map(Some)
    }

    /// How progress hints should be delivered on this platform.
    ///
    /// Defaults to the platform's capability declaration; adapters that expose
    /// a per-channel override (e.g. WeChat's config) override this method.
    fn hint_policy(&self) -> HintPolicy {
        Self::capabilities().progress_hints
    }

    /// Signal the chat's busy state (native "typing" indicator). Idempotent
    /// and best-effort.
    ///
    /// Contract:
    /// - Must return promptly: spawn internal work, never block on HTTP or
    ///   send-budget queues — callers sit on the turn's critical path.
    /// - `active = true`: show the native busy indicator (the adapter owns
    ///   any refresh cadence). `active = false`: cancel it. Repeated calls
    ///   with the same value are no-ops.
    /// - Failures are logged by the adapter, never surfaced as turn errors.
    ///
    /// Default: no-op. Platforms without a native indicator (QQ, Feishu)
    /// keep their existing behavior exactly.
    async fn set_typing(&self, _chat_id: &str, _active: bool) -> Result<()> {
        Ok(())
    }

    /// Per-channel typing policy. Defaults to the capability declaration;
    /// adapters with a config override (e.g. WeChat's `typing` switch)
    /// override this method. Mirrors [`Self::hint_policy`].
    fn typing_enabled(&self) -> bool {
        Self::capabilities().supports_typing
    }

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
