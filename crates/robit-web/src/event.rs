//! Official JSON wire projection of [`AgentEvent`].

use robit_agent::event::AgentEvent;
use serde::Serialize;

/// A browser-facing projection of an `AgentEvent`, plus the bridge-generated
/// `confirm_request` frame. Serialized with an internal `type` tag in
/// snake_case, e.g. `{"type":"text_delta","text":"hi"}`.
///
/// This is the stable contract between a robit web frontend (this crate) and
/// a web client (JS/TS). Host-specific frames — e.g. a `connected` hello with
/// model/tool metadata the endpoint knows — are the host's own to serialize;
/// merge them into the stream before serving.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WebEvent {
    /// Streaming text delta from the LLM response.
    TextDelta { text: String },

    /// The LLM requested a tool call. Clients may display name/arguments.
    ToolCall {
        tool_call_id: String,
        name: String,
        arguments: String,
    },

    /// A tool finished. `content` is the tool's text output fed back to the LLM.
    ToolResult {
        tool_call_id: String,
        is_error: bool,
        content: String,
    },

    /// A skill was triggered (display as a system notice).
    SkillTriggered { name: String, description: String },

    /// An async background task finished (success, failure or cancellation).
    AsyncToolCompleted {
        task_id: String,
        tool_call_id: String,
        is_error: bool,
    },

    /// A tool call needs user confirmation. The agent task is suspended until
    /// the host calls `WebBridge::resolve_confirm` with this `tool_call_id`.
    ConfirmRequest {
        tool_call_id: String,
        name: String,
        arguments: String,
    },

    /// Current turn is complete.
    TurnComplete,

    /// An error occurred during agent execution (message is display-safe).
    Error { message: String },
}

impl WebEvent {
    /// Project an agent event into its wire form.
    pub fn from_agent(event: &AgentEvent) -> Self {
        match event {
            AgentEvent::TextDelta(text) => WebEvent::TextDelta { text: text.clone() },
            AgentEvent::ToolCallRequested {
                tool_call_id,
                name,
                arguments,
            } => WebEvent::ToolCall {
                tool_call_id: tool_call_id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
            },
            AgentEvent::ToolCallResult { tool_call_id, result } => WebEvent::ToolResult {
                tool_call_id: tool_call_id.clone(),
                is_error: result.is_error,
                content: result.content.clone(),
            },
            AgentEvent::SkillTriggered { name, description } => WebEvent::SkillTriggered {
                name: name.clone(),
                description: description.clone(),
            },
            AgentEvent::AsyncToolCompleted {
                task_id,
                tool_call_id,
                result,
            } => WebEvent::AsyncToolCompleted {
                task_id: task_id.clone(),
                tool_call_id: tool_call_id.clone(),
                is_error: result.is_error,
            },
            AgentEvent::TurnComplete => WebEvent::TurnComplete,
            AgentEvent::Error(e) => WebEvent::Error {
                message: e.to_string(),
            },
        }
    }

    /// Serialize to a JSON string (never fails for this type).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|e| {
            format!(r#"{{"type":"error","message":"WebEvent serialization failed: {e}"}}"#)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use robit_agent::tool::ToolResult;
    use serde_json::json;

    #[test]
    fn text_delta_wire_shape() {
        let e = WebEvent::from_agent(&AgentEvent::TextDelta("你好".into()));
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({"type": "text_delta", "text": "你好"})
        );
    }

    #[test]
    fn every_agent_event_variant_projects() {
        let cases = [
            (
                AgentEvent::ToolCallRequested {
                    tool_call_id: "tc1".into(),
                    name: "bash".into(),
                    arguments: "{}".into(),
                },
                json!({"type": "tool_call", "tool_call_id": "tc1", "name": "bash", "arguments": "{}"}),
            ),
            (
                AgentEvent::ToolCallResult {
                    tool_call_id: "tc1".into(),
                    result: ToolResult::success("done"),
                },
                json!({"type": "tool_result", "tool_call_id": "tc1", "is_error": false, "content": "done"}),
            ),
            (
                AgentEvent::SkillTriggered {
                    name: "review".into(),
                    description: "d".into(),
                },
                json!({"type": "skill_triggered", "name": "review", "description": "d"}),
            ),
            (
                AgentEvent::AsyncToolCompleted {
                    task_id: "t1".into(),
                    tool_call_id: "tc1".into(),
                    result: ToolResult::error("boom"),
                },
                json!({"type": "async_tool_completed", "task_id": "t1", "tool_call_id": "tc1", "is_error": true}),
            ),
            (
                AgentEvent::TurnComplete,
                json!({"type": "turn_complete"}),
            ),
        ];
        for (event, expected) in cases {
            let e = WebEvent::from_agent(&event);
            assert_eq!(serde_json::to_value(&e).unwrap(), expected);
        }
    }

    #[test]
    fn error_variant_carries_display_message() {
        let e = WebEvent::from_agent(&AgentEvent::Error(robit_agent::AgentError::ToolError(
            "boom".into(),
        )));
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({"type": "error", "message": "Tool execution error: boom"})
        );
    }
}
