//! Agent event and message types for Frontend <-> Agent communication.

use crate::tool::ToolResult;

/// Unique session identifier (UUID v4).
pub type SessionId = String;

/// Create a new random session ID.
pub fn new_session_id() -> SessionId {
    uuid::Uuid::new_v4().to_string()
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
    /// User typed a new message.
    UserInput(String),

    /// User wants to cancel the current operation.
    Cancel,

    /// User responded to a tool confirmation request.
    ConfirmationResponse {
        tool_call_id: String,
        approved: bool,
    },
}
