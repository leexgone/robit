//! LlmClient: a thin wrapper around async-openai with unified config support.

use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionMessageToolCalls, ChatCompletionRequestMessage,
    ChatCompletionRequestToolMessage, ChatCompletionResponseStream,
    ChatCompletionTools, CreateChatCompletionRequest, CreateChatCompletionResponse,
};

use crate::config::{resolve_profile, ResolvedModel, RobitConfig};
use crate::error::LlmError;

/// Validate that all messages are valid before sending to LLM.
/// Returns a filtered list of messages with invalid messages removed.
fn validate_and_filter_messages(mut messages: Vec<ChatCompletionRequestMessage>) -> Vec<ChatCompletionRequestMessage> {
    let original_len = messages.len();
    messages.retain(|msg| {
        match msg {
            ChatCompletionRequestMessage::Assistant(assistant_msg) => {
                // Assistant message must have either content or tool_calls
                let has_content = assistant_msg.content.is_some();
                let has_tool_calls = assistant_msg.tool_calls.is_some();
                if !has_content && !has_tool_calls {
                    tracing::warn!("Filtering out invalid assistant message (has neither content nor tool_calls)");
                    false
                } else {
                    true
                }
            }
            _ => true
        }
    });
    let filtered_len = messages.len();
    if filtered_len < original_len {
        tracing::info!("Filtered {} invalid messages from history", original_len - filtered_len);
    }
    messages
}

/// Repair tool-message pairing so the history satisfies the OpenAI-protocol
/// invariants enforced by providers (including DeepSeek, which rejects
/// violations with a 400 "Messages with role 'tool' must be a response to a
/// preceding message with 'tool_calls'"):
///
/// 1. Every `tool` message must reference a `tool_call_id` declared by a
///    preceding assistant message's `tool_calls`. Orphaned tool messages
///    (e.g. history restored from a database that persisted tool results but
///    not the assistant message that requested them) are dropped.
/// 2. Every `tool_calls` entry in an assistant message must have a matching
///    `tool` response. Missing responses (e.g. the process was killed
///    mid-step before all results were recorded) are synthesized as
///    placeholder tool messages right after the assistant message.
fn repair_tool_pairing(
    messages: Vec<ChatCompletionRequestMessage>,
) -> Vec<ChatCompletionRequestMessage> {
    use std::collections::HashSet;

    // Pass 1 (forward scan): decide which tool messages are matched.
    let mut open_ids: HashSet<String> = HashSet::new();
    let mut keep: Vec<bool> = Vec::with_capacity(messages.len());
    let mut dropped = 0usize;
    for msg in &messages {
        match msg {
            ChatCompletionRequestMessage::Assistant(a) => {
                if let Some(tool_calls) = &a.tool_calls {
                    for tc in tool_calls {
                        if let ChatCompletionMessageToolCalls::Function(f) = tc {
                            open_ids.insert(f.id.clone());
                        }
                    }
                }
                keep.push(true);
            }
            ChatCompletionRequestMessage::Tool(t) => {
                if open_ids.remove(&t.tool_call_id) {
                    keep.push(true);
                } else {
                    tracing::trace!(
                        "repair_tool_pairing: dropping orphaned tool message \
                         (tool_call_id='{}' not declared by any preceding assistant tool_calls)",
                        t.tool_call_id
                    );
                    keep.push(false);
                    dropped += 1;
                }
            }
            _ => keep.push(true),
        }
    }
    // Ids still in `open_ids` were declared but never got a tool response.
    let mut missing = open_ids;

    if dropped == 0 && missing.is_empty() {
        return messages; // fast path: nothing to repair
    }
    if dropped > 0 {
        // Warn once per call (not per message): orphaned tool messages are a
        // sign of incomplete history persistence and would otherwise spam
        // the log on every LLM call of a restored session.
        tracing::warn!(
            "repair_tool_pairing: dropped {} orphaned tool message(s) not declared by any assistant tool_calls",
            dropped
        );
    }

    // Pass 2: rebuild, inserting placeholder responses for missing ids.
    let mut synthesized = 0usize;
    let mut result: Vec<ChatCompletionRequestMessage> = Vec::with_capacity(messages.len());
    for (msg, keep) in messages.into_iter().zip(keep.into_iter()) {
        if !keep {
            continue;
        }
        // Collect the still-missing ids declared by this assistant message.
        let missing_here: Vec<String> = match &msg {
            ChatCompletionRequestMessage::Assistant(a) => a
                .tool_calls
                .as_ref()
                .map(|tool_calls| {
                    tool_calls
                        .iter()
                        .filter_map(|tc| {
                            if let ChatCompletionMessageToolCalls::Function(f) = tc {
                                // `remove` guarantees at most one placeholder per id.
                                if missing.remove(&f.id) {
                                    Some(f.id.clone())
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        })
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        result.push(msg);
        for id in missing_here {
            tracing::trace!(
                "repair_tool_pairing: synthesizing missing tool response for tool_call_id='{}'",
                id
            );
            synthesized += 1;
            result.push(ChatCompletionRequestMessage::Tool(
                ChatCompletionRequestToolMessage {
                    content: "[Tool result unavailable — session history was restored without this result]"
                        .to_string()
                        .into(),
                    tool_call_id: id,
                }
                .into(),
            ));
        }
    }
    if synthesized > 0 {
        tracing::warn!(
            "repair_tool_pairing: synthesized {} missing tool response(s) for declared tool_calls",
            synthesized
        );
    }
    result
}

pub struct LlmClient {
    client: async_openai::Client<OpenAIConfig>,
    model: String,
    resolved: ResolvedModel,
}

impl LlmClient {
    /// Create a new LlmClient from loaded configuration.
    ///
    /// `profile_name`: which profile to use. If `None`, uses the default profile.
    pub fn from_config(
        config: &RobitConfig,
        profile_name: Option<&str>,
    ) -> Result<Self, LlmError> {
        let resolved = resolve_profile(config, profile_name)?;

        let oc = OpenAIConfig::new()
            .with_api_base(&resolved.base_url)
            .with_api_key(&resolved.api_key);

        let client = async_openai::Client::with_config(oc);

        Ok(Self {
            client,
            model: resolved.model_id.clone(),
            resolved,
        })
    }

    /// Streaming chat completion. Returns an async stream of response chunks.
    pub async fn chat_stream(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Option<Vec<ChatCompletionTools>>,
    ) -> Result<ChatCompletionResponseStream, LlmError> {
        // Validate and repair messages before sending to LLM
        let messages = validate_and_filter_messages(messages);
        let messages = repair_tool_pairing(messages);
        let msg_count = messages.len();

        tracing::trace!("Creating chat stream for model={}, messages={}", self.model, msg_count);

        let request = CreateChatCompletionRequest {
            model: self.model.clone(),
            messages,
            tools,
            stream: Some(true),
            // Request usage stats in streaming response.
            // Supported by OpenAI and DeepSeek (extra chunk before [DONE]).
            // DeepSeek docs confirm: https://api-docs.deepseek.com/zh-cn/api/create-chat-completion
            stream_options: Some(async_openai::types::chat::ChatCompletionStreamOptions {
                include_usage: Some(true),
                include_obfuscation: None,
            }),
            max_completion_tokens: self.resolved.max_tokens,
            temperature: self.resolved.temperature,
            ..Default::default()
        };
        let stream = self.client.chat().create_stream(request).await;
        if let Err(e) = &stream {
            tracing::error!("Chat stream creation failed: {:?}", e);
        }
        let stream = stream?;
        Ok(stream)
    }

    /// Non-streaming chat completion. Returns the full response.
    pub async fn chat(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Option<Vec<ChatCompletionTools>>,
    ) -> Result<CreateChatCompletionResponse, LlmError> {
        // Validate and repair messages before sending to LLM
        let messages = validate_and_filter_messages(messages);
        let messages = repair_tool_pairing(messages);

        let request = CreateChatCompletionRequest {
            model: self.model.clone(),
            messages,
            tools,
            max_completion_tokens: self.resolved.max_tokens,
            temperature: self.resolved.temperature,
            ..Default::default()
        };

        let response = self.client.chat().create(request).await?;
        Ok(response)
    }

    /// Get the current model ID (e.g. "deepseek-chat").
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Get the profile name (e.g. "default").
    pub fn profile(&self) -> &str {
        &self.resolved.profile_name
    }

    /// Get the resolved model info.
    pub fn resolved(&self) -> &ResolvedModel {
        &self.resolved
    }

    /// Whether the current model supports image inputs.
    pub fn supports_images(&self) -> bool {
        self.resolved.supports_images
    }

    /// Whether the current model supports tool calling.
    pub fn supports_tools(&self) -> bool {
        self.resolved.supports_tools
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use async_openai::types::chat::{
        ChatCompletionMessageToolCall, ChatCompletionRequestAssistantMessage,
        ChatCompletionRequestUserMessage, FunctionCall,
    };

    fn user_msg(text: &str) -> ChatCompletionRequestMessage {
        ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
            content: text.to_string().into(),
            name: None,
        })
    }

    fn assistant_text_msg(text: &str) -> ChatCompletionRequestMessage {
        ChatCompletionRequestMessage::Assistant(ChatCompletionRequestAssistantMessage {
            content: Some(text.to_string().into()),
            name: None,
            tool_calls: None,
            refusal: None,
            audio: None,
            #[allow(deprecated)]
            function_call: None,
        })
    }

    fn assistant_tool_call_msg(id: &str, name: &str) -> ChatCompletionRequestMessage {
        ChatCompletionRequestMessage::Assistant(ChatCompletionRequestAssistantMessage {
            content: None,
            name: None,
            tool_calls: Some(vec![ChatCompletionMessageToolCalls::Function(
                ChatCompletionMessageToolCall {
                    id: id.to_string(),
                    function: FunctionCall {
                        name: name.to_string(),
                        arguments: "{}".to_string(),
                    },
                },
            )]),
            refusal: None,
            audio: None,
            #[allow(deprecated)]
            function_call: None,
        })
    }

    fn tool_msg(id: &str, text: &str) -> ChatCompletionRequestMessage {
        ChatCompletionRequestMessage::Tool(ChatCompletionRequestToolMessage {
            content: text.to_string().into(),
            tool_call_id: id.to_string(),
        })
    }

    #[test]
    fn repair_drops_orphaned_tool_messages() {
        // History restored from a DB that saved tool results but not the
        // assistant message that declared the tool_calls.
        let messages = vec![
            user_msg("hello"),
            assistant_text_msg("hi"),
            user_msg("do something"),
            tool_msg("call_1", "orphaned result"),
            assistant_text_msg("done"),
        ];
        let repaired = repair_tool_pairing(messages);
        assert_eq!(repaired.len(), 4, "orphaned tool message should be dropped");
        assert!(
            !repaired
                .iter()
                .any(|m| matches!(m, ChatCompletionRequestMessage::Tool(_))),
            "no tool messages should remain"
        );
    }

    #[test]
    fn repair_synthesizes_missing_tool_responses() {
        // Assistant declared a tool call but the result was never recorded
        // (e.g. process killed mid-step).
        let messages = vec![
            user_msg("do something"),
            assistant_tool_call_msg("call_1", "bash"),
            user_msg("next question"),
        ];
        let repaired = repair_tool_pairing(messages);
        assert_eq!(repaired.len(), 4, "placeholder tool response should be added");
        // Placeholder must come right after the assistant message.
        assert!(matches!(repaired[2], ChatCompletionRequestMessage::Tool(_)));
        if let ChatCompletionRequestMessage::Tool(t) = &repaired[2] {
            assert_eq!(t.tool_call_id, "call_1");
        }
    }

    #[test]
    fn repair_keeps_valid_pairing_untouched() {
        let messages = vec![
            user_msg("do something"),
            assistant_tool_call_msg("call_1", "bash"),
            tool_msg("call_1", "ok"),
            assistant_text_msg("done"),
        ];
        let repaired = repair_tool_pairing(messages.clone());
        assert_eq!(repaired.len(), messages.len(), "valid history must not change");
    }

    #[test]
    fn repair_handles_mixed_valid_and_orphaned() {
        let messages = vec![
            user_msg("a"),
            assistant_tool_call_msg("call_1", "read"),
            tool_msg("call_1", "result 1"), // valid
            tool_msg("call_ghost", "ghost result"), // orphaned
            assistant_text_msg("done"),
        ];
        let repaired = repair_tool_pairing(messages);
        assert_eq!(repaired.len(), 4);
        let tool_ids: Vec<&str> = repaired
            .iter()
            .filter_map(|m| {
                if let ChatCompletionRequestMessage::Tool(t) = m {
                    Some(t.tool_call_id.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(tool_ids, vec!["call_1"]);
    }
}
