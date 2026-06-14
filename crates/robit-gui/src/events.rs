#[allow(unused_imports)]
pub use robit_agent::storage::{MessageData, SessionInfo, ToolCallInfoData};
use serde::Serialize;

/// Events pushed from Rust backend to React frontend via Tauri events.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum UiEvent {
    /// Streaming text delta from LLM response.
    TextDelta { session_id: String, delta: String },
    /// LLM requested a tool call.
    ToolCallRequested {
        session_id: String,
        tool_call_id: String,
        name: String,
        arguments: String,
        requires_confirm: bool,
    },
    /// Tool execution completed.
    ToolCallResult {
        session_id: String,
        tool_call_id: String,
        content: String,
        is_error: bool,
    },
    /// Current turn is complete.
    TurnComplete { session_id: String },
    /// An error occurred.
    Error { session_id: String, message: String },
    /// A skill was triggered.
    SkillTriggered {
        session_id: String,
        name: String,
        description: String,
    },
    /// Session title was updated (auto-renamed or manual rename).
    SessionRenamed { session_id: String, title: String },
}

/// Non-sensitive configuration exposed to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigInfo {
    pub model: String,
    pub version: String,
    pub tools_enabled: usize,
    pub tools_total: usize,
    pub skills_enabled: usize,
    pub skills_total: usize,
    pub auto_approve: bool,
    pub working_dir: String,
}
