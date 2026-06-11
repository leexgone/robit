use serde::{Serialize, Deserialize};

/// Events pushed from Rust backend to React frontend via Tauri events.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum UiEvent {
    /// Streaming text delta from LLM response.
    TextDelta {
        session_id: String,
        delta: String,
    },
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
    TurnComplete {
        session_id: String,
    },
    /// An error occurred.
    Error {
        session_id: String,
        message: String,
    },
    /// A skill was triggered.
    SkillTriggered {
        session_id: String,
        name: String,
        description: String,
    },
}

/// Session metadata returned to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub model: String,
    pub status: String,       // "idle" | "ready" | "running"
    pub created_at: String,
    pub updated_at: String,
}

/// Message data returned to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageData {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_info: Option<serde_json::Value>,
    pub created_at: String,
}

/// Tool call info for storage in message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallInfoData {
    pub tool_call_id: String,
    pub name: String,
    pub arguments: String,
    pub status: String,
    pub output: Option<String>,
    pub requires_confirm: bool,
}

/// Non-sensitive configuration exposed to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigInfo {
    pub model: String,
    pub version: String,
    pub tools_enabled: usize,
    pub tools_total: usize,
    pub auto_approve: bool,
}
