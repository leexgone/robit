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
}

impl MediaAttachment {
    /// Whether this attachment is an image.
    pub fn is_image(&self) -> bool {
        self.content_type.starts_with("image/")
    }

    /// A human-readable description for the LLM.
    pub fn describe(&self) -> String {
        let filename = self.filename.as_deref().unwrap_or("unknown");
        let type_desc = if self.is_image() { "图片" } else { "文件" };
        let size_str = self
            .size
            .map(|s| format!(" ({:.1}KB)", s as f64 / 1024.0))
            .unwrap_or_default();
        format!("[用户发送了{}: {}{}]", type_desc, filename, size_str)
    }
}

/// Events pushed from Agent to Frontend.
#[derive(Debug)]
pub enum AgentEvent {
    /// Streaming text delta from LLM response.
    TextDelta(String),

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
