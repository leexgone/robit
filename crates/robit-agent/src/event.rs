//! Agent event and message types for Frontend <-> Agent communication.

use crate::tool::ToolResult;

/// Unique session identifier (UUID v4).
pub type SessionId = String;

/// Create a new random session ID.
pub fn new_session_id() -> SessionId {
    uuid::Uuid::new_v4().to_string()
}

/// A media attachment (image, file, etc.) included in a user message.
#[derive(Debug, Clone)]
pub struct MediaAttachment {
    /// MIME type (e.g. "image/jpeg", "application/pdf").
    pub content_type: String,
    /// URL to access the media.
    pub url: String,
    /// Original filename if available.
    pub filename: Option<String>,
    /// File size in bytes if available.
    pub size: Option<u64>,
    /// Image width in pixels if available.
    pub width: Option<u32>,
    /// Image height in pixels if available.
    pub height: Option<u32>,
    /// Local copy saved by the host (e.g. the bot manager's media directory).
    /// `None` when no local copy exists (GUI/TUI paths, or download failed).
    /// Encoding prefers this over `url` so CDN expiry cannot break vision.
    pub local_path: Option<std::path::PathBuf>,
}

impl MediaAttachment {
    /// Whether this attachment is an image.
    pub fn is_image(&self) -> bool {
        self.content_type.starts_with("image/")
    }

    /// A human-readable description for the LLM (no local path).
    ///
    /// Single source of truth for attachment annotation: the agent appends it
    /// to model-bound text exactly once (`build_user_message`); the bot manager
    /// reuses it for archive text and session titles.
    pub fn describe(&self) -> String {
        self.describe_inner(None)
    }

    /// Like [`describe`](Self::describe), but appends the saved local copy as
    /// `本地路径: <path>` when `local_path` is set — the hook that lets the
    /// model point host domain tools at the file (M1 completion). The path is
    /// rendered relative to `base` (the working directory) when it lives under
    /// it, falling back to the absolute form otherwise.
    pub fn describe_with_local(&self, base: Option<&std::path::Path>) -> String {
        self.describe_inner(base)
    }

    fn describe_inner(&self, base: Option<&std::path::Path>) -> String {
        let filename = self
            .filename
            .as_deref()
            .unwrap_or_else(|| if self.is_image() { "image" } else { "file" });
        let type_desc = if self.is_image() { "图片" } else { "文件" };
        let size_str = self
            .size
            .map(|s| format!(" ({:.1}KB)", s as f64 / 1024.0))
            .unwrap_or_default();
        let local_str = match &self.local_path {
            Some(path) => {
                let display = base
                    .and_then(|b| path.strip_prefix(b).ok())
                    .unwrap_or(path.as_path());
                format!(", 本地路径: {}", display.display())
            }
            None => String::new(),
        };
        format!("[用户发送了{}: {}{}{}]", type_desc, filename, size_str, local_str)
    }
}

/// Events pushed from Agent to Frontend.
#[derive(Debug)]
pub enum AgentEvent {
    /// Streaming text delta from LLM response.
    TextDelta(String),

    /// Streaming thinking/reasoning delta (`delta.reasoning_content` from
    /// thinking models). Display-only: never written back to session history
    /// and never counted into response text. May arrive before or interleaved
    /// with `TextDelta` in the same assistant segment; there is no explicit
    /// end marker — consumers close any open thinking segment on
    /// `TurnComplete` (rendering layers may additionally close on the first
    /// `TextDelta`/`ToolCallRequested`). Frontends that do not render
    /// thinking may safely ignore this event.
    ThinkingDelta(String),

    /// LLM requested a tool call. Frontend should display and optionally wait for confirmation.
    ToolCallRequested {
        tool_call_id: String,
        name: String,
        arguments: String,
    },

    /// Tool execution completed with result.
    ToolCallResult {
        tool_call_id: String,
        result: ToolResult,
    },

    /// Current turn is complete (LLM finished responding, no more tool calls).
    TurnComplete,

    /// An error occurred during agent execution.
    Error(crate::error::AgentError),

    /// A skill was triggered. Frontend can display this as a system notice.
    SkillTriggered { name: String, description: String },

    /// An async background task finished (success, failure, or cancellation).
    /// `result` is the final outcome, already reinjected into the conversation
    /// as a system-notification message, and the LLM is being woken to act on
    /// it. Frontends use this to update task-progress UI; no further action is
    /// required for the conversation itself.
    ///
    /// Task *start* is signalled by the regular `ToolCallResult` event whose
    /// `result.is_pending` is `true` (with `result.pending_task_id` set).
    AsyncToolCompleted {
        task_id: String,
        tool_call_id: String,
        result: ToolResult,
    },
}

/// Messages sent from Frontend to Agent.
#[derive(Debug)]
pub enum FrontendMessage {
    /// User typed a new message with optional media attachments.
    UserInput {
        text: String,
        attachments: Vec<MediaAttachment>,
    },

    /// User wants to cancel the current operation.
    Cancel,

    /// User responded to a tool confirmation request.
    ConfirmationResponse {
        tool_call_id: String,
        approved: bool,
    },

    /// User requested cancellation of a specific async background task.
    CancelTask { task_id: String },
}

// Keep backward compatibility: from String to UserInput with no attachments.
impl From<String> for FrontendMessage {
    fn from(text: String) -> Self {
        Self::UserInput {
            text,
            attachments: vec![],
        }
    }
}

// Also support &str for convenience.
impl From<&str> for FrontendMessage {
    fn from(text: &str) -> Self {
        Self::UserInput {
            text: text.to_string(),
            attachments: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(content_type: &str, filename: Option<&str>, size: Option<u64>) -> MediaAttachment {
        MediaAttachment {
            content_type: content_type.to_string(),
            url: "http://example.invalid/f".to_string(),
            filename: filename.map(|s| s.to_string()),
            size,
            width: None,
            height: None,
            local_path: None,
        }
    }

    #[test]
    fn describe_formats_name_and_size() {
        let a = att("text/csv", Some("对账单.csv"), Some(51_200));
        assert_eq!(a.describe(), "[用户发送了文件: 对账单.csv (50.0KB)]");
    }

    #[test]
    fn describe_unnamed_defaults_by_kind() {
        assert_eq!(
            att("image/png", None, None).describe(),
            "[用户发送了图片: image]"
        );
        assert_eq!(
            att("application/pdf", None, None).describe(),
            "[用户发送了文件: file]"
        );
    }

    #[test]
    fn describe_with_local_appends_path_relative_to_base() {
        let mut a = att("text/csv", Some("对账单.csv"), Some(51_200));
        a.local_path = Some(std::path::PathBuf::from("/srv/bot/media/对账单.csv"));
        assert_eq!(
            a.describe_with_local(Some(std::path::Path::new("/srv/bot"))),
            "[用户发送了文件: 对账单.csv (50.0KB), 本地路径: media/对账单.csv]"
        );

        // Outside the base (or without one) → absolute form.
        a.local_path = Some(std::path::PathBuf::from("/elsewhere/x.csv"));
        assert!(a
            .describe_with_local(Some(std::path::Path::new("/srv/bot")))
            .contains("本地路径: /elsewhere/x.csv"));
        assert!(a
            .describe_with_local(None)
            .contains("本地路径: /elsewhere/x.csv"));

        // No local copy → identical to describe().
        a.local_path = None;
        assert_eq!(
            a.describe_with_local(Some(std::path::Path::new("/srv/bot"))),
            a.describe()
        );
    }
}
