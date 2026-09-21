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

/// Repair histories where a non-tool message (e.g. a user message carrying
/// tool-result images) is interleaved between an assistant `tool_calls`
/// message and its tool responses.
///
/// Providers enforce that the messages immediately following an assistant
/// message with `tool_calls` are the `tool` responses for each declared
/// `tool_call_id`; DeepSeek rejects violations with a 400 "insufficient tool
/// messages following tool_calls message". Such histories can come from
/// sessions written by older builds that injected image user messages
/// per-tool-call, or from restored databases.
///
/// Deferred non-tool messages are re-inserted after the batch's last tool
/// response (or at the end of the history if the batch is truncated),
/// preserving their relative order.
fn repair_interleaved_tool_responses(
    messages: Vec<ChatCompletionRequestMessage>,
) -> Vec<ChatCompletionRequestMessage> {
    use std::collections::HashSet;

    // Quick pre-scan: is there any non-tool message inside a tool_calls →
    // tool-responses window? If not, pass through without rebuilding.
    {
        let mut pending: HashSet<&String> = HashSet::new();
        let mut interleaved = false;
        'scan: for msg in &messages {
            match msg {
                ChatCompletionRequestMessage::Assistant(a) => {
                    if let Some(tool_calls) = &a.tool_calls {
                        pending = tool_calls
                            .iter()
                            .filter_map(|tc| {
                                if let ChatCompletionMessageToolCalls::Function(f) = tc {
                                    Some(&f.id)
                                } else {
                                    None
                                }
                            })
                            .collect();
                    }
                }
                ChatCompletionRequestMessage::Tool(t) => {
                    pending.remove(&t.tool_call_id);
                }
                _ => {
                    if !pending.is_empty() {
                        interleaved = true;
                        break 'scan;
                    }
                }
            }
        }
        if !interleaved {
            return messages;
        }
    }

    tracing::warn!(
        "repair_interleaved_tool_responses: moving non-tool message(s) out of a \
         tool_calls → tool-responses window (providers reject interleaved messages \
         with a 400 error)"
    );

    // Rebuild: defer non-tool messages that arrive while a batch is still
    // unanswered; flush them after the batch's last tool response.
    let mut result: Vec<ChatCompletionRequestMessage> = Vec::with_capacity(messages.len());
    let mut deferred: Vec<ChatCompletionRequestMessage> = Vec::new();
    let mut pending: HashSet<String> = HashSet::new();

    for msg in messages {
        match &msg {
            ChatCompletionRequestMessage::Assistant(a) => {
                if let Some(tool_calls) = &a.tool_calls {
                    // A new batch while the previous one is still unanswered
                    // (truncated history): flush deferred messages before it.
                    if !deferred.is_empty() {
                        result.append(&mut deferred);
                    }
                    pending = tool_calls
                        .iter()
                        .filter_map(|tc| {
                            if let ChatCompletionMessageToolCalls::Function(f) = tc {
                                Some(f.id.clone())
                            } else {
                                None
                            }
                        })
                        .collect();
                    result.push(msg);
                } else if pending.is_empty() {
                    result.push(msg);
                } else {
                    deferred.push(msg);
                }
            }
            ChatCompletionRequestMessage::Tool(t) => {
                let responded = pending.remove(&t.tool_call_id);
                result.push(msg);
                if responded && pending.is_empty() && !deferred.is_empty() {
                    result.append(&mut deferred);
                }
            }
            _ => {
                if pending.is_empty() {
                    result.push(msg);
                } else {
                    deferred.push(msg);
                }
            }
        }
    }
    // A batch truncated mid-way (history cut before all responses): keep any
    // still-deferred messages at the end; `repair_tool_pairing` will
    // synthesize placeholders for the unanswered ids right after the
    // assistant message.
    result.append(&mut deferred);
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
        // Validate and repair messages before sending to LLM. Interleave
        // repair must run before tool-pairing repair: it restores the
        // assistant → tool×N window so the pairing pass can then match, drop,
        // or synthesize tool responses against a contiguous batch.
        let messages = validate_and_filter_messages(messages);
        let messages = repair_interleaved_tool_responses(messages);
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
        // Map to a friendly error (e.g. content-moderation rejections carry a
        // provider error code that deserves a clearer message than the raw
        // "400 Bad Request ..." display).
        let stream = stream.map_err(LlmError::from_openai_error)?;
        Ok(stream)
    }

    /// Non-streaming chat completion. Returns the full response.
    pub async fn chat(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Option<Vec<ChatCompletionTools>>,
    ) -> Result<CreateChatCompletionResponse, LlmError> {
        // Validate and repair messages before sending to LLM (same pipeline
        // as `chat_stream`; see the comment there for the repair order).
        let messages = validate_and_filter_messages(messages);
        let messages = repair_interleaved_tool_responses(messages);
        let messages = repair_tool_pairing(messages);

        let request = CreateChatCompletionRequest {
            model: self.model.clone(),
            messages,
            tools,
            max_completion_tokens: self.resolved.max_tokens,
            temperature: self.resolved.temperature,
            ..Default::default()
        };

        let response = self
            .client
            .chat()
            .create(request)
            .await
            .map_err(LlmError::from_openai_error)?;
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

    fn assistant_multi_tool_call_msg(
        calls: &[(&str, &str)],
    ) -> ChatCompletionRequestMessage {
        ChatCompletionRequestMessage::Assistant(ChatCompletionRequestAssistantMessage {
            content: None,
            name: None,
            tool_calls: Some(
                calls
                    .iter()
                    .map(|(id, name)| {
                        ChatCompletionMessageToolCalls::Function(ChatCompletionMessageToolCall {
                            id: id.to_string(),
                            function: FunctionCall {
                                name: name.to_string(),
                                arguments: "{}".to_string(),
                            },
                        })
                    })
                    .collect(),
            ),
            refusal: None,
            audio: None,
            #[allow(deprecated)]
            function_call: None,
        })
    }

    fn image_user_msg(label: &str) -> ChatCompletionRequestMessage {
        user_msg(&format!("[工具返回的图片] {}", label))
    }

    #[test]
    fn interleave_repair_moves_user_messages_after_tool_batch() {
        // The exact shape produced by the pre-fix image injection: one
        // multimodal user message right after EACH tool result of a parallel
        // tool_calls batch. Providers reject this with 400 "insufficient
        // tool messages following tool_calls message".
        let messages = vec![
            user_msg("generate images"),
            assistant_multi_tool_call_msg(&[
                ("call_0", "read"),
                ("call_1", "read"),
                ("call_2", "read"),
            ]),
            tool_msg("call_0", "Image file: a.png"),
            image_user_msg("a.png"),
            tool_msg("call_1", "Image file: b.png"),
            image_user_msg("b.png"),
            tool_msg("call_2", "Image file: c.png"),
            image_user_msg("c.png"),
        ];
        let repaired = repair_interleaved_tool_responses(messages);
        assert_eq!(repaired.len(), 8, "no message may be dropped");

        let role_kinds: Vec<&str> = repaired
            .iter()
            .map(|m| match m {
                ChatCompletionRequestMessage::User(_) => "user",
                ChatCompletionRequestMessage::Assistant(a) => {
                    if a.tool_calls.is_some() {
                        "assistant+tool_calls"
                    } else {
                        "assistant"
                    }
                }
                ChatCompletionRequestMessage::Tool(_) => "tool",
                _ => "other",
            })
            .collect();
        // Tool responses must directly follow the assistant tool_calls
        // message, with all image user messages moved after the batch.
        assert_eq!(
            role_kinds,
            vec![
                "user",
                "assistant+tool_calls",
                "tool",
                "tool",
                "tool",
                "user",
                "user",
                "user",
            ]
        );
        // Relative order of the deferred image messages is preserved.
        let user_texts: Vec<String> = repaired
            .iter()
            .filter_map(|m| {
                if let ChatCompletionRequestMessage::User(u) = m {
                    if let async_openai::types::chat::ChatCompletionRequestUserMessageContent::Text(t) = &u.content {
                        Some(t.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            user_texts.last().map(|t| t.contains("c.png")),
            Some(true),
            "deferred messages keep their original order (c.png last)"
        );
    }

    #[test]
    fn interleave_repair_keeps_valid_history_untouched() {
        let messages = vec![
            user_msg("look at this"),
            assistant_tool_call_msg("call_1", "read"),
            tool_msg("call_1", "Image file: a.png"),
            image_user_msg("a.png"),
            assistant_text_msg("looks great"),
        ];
        let repaired = repair_interleaved_tool_responses(messages.clone());
        assert_eq!(
            repaired.len(),
            messages.len(),
            "valid history must not change length"
        );
        // And not just length: a valid history must pass through unchanged.
        let summarize = |m: &ChatCompletionRequestMessage| match m {
            ChatCompletionRequestMessage::User(u) => format!("user:{:?}", u.content),
            ChatCompletionRequestMessage::Assistant(a) => format!(
                "assistant:{:?}:{:?}",
                a.content, a.tool_calls.as_ref().map(|tcs| tcs.len())
            ),
            ChatCompletionRequestMessage::Tool(t) => {
                format!("tool:{}:{:?}", t.tool_call_id, t.content)
            }
            _ => "other".to_string(),
        };
        let before: Vec<String> = messages.iter().map(summarize).collect();
        let after: Vec<String> = repaired.iter().map(summarize).collect();
        assert_eq!(before, after, "valid history must not be reordered");
    }

    #[test]
    fn interleave_repair_truncated_batch_defers_to_end() {
        // Batch never fully answered (e.g. truncated history): the interleaved
        // user message still moves after the last tool response of the batch.
        let messages = vec![
            user_msg("do something"),
            assistant_multi_tool_call_msg(&[("call_0", "read"), ("call_1", "read")]),
            tool_msg("call_0", "result 0"),
            image_user_msg("a.png"),
        ];
        let repaired = repair_interleaved_tool_responses(messages);
        assert_eq!(repaired.len(), 4);
        assert!(matches!(repaired[1], ChatCompletionRequestMessage::Assistant(_)));
        assert!(matches!(repaired[2], ChatCompletionRequestMessage::Tool(_)));
        assert!(matches!(repaired[3], ChatCompletionRequestMessage::User(_)));
    }

    #[test]
    fn interleave_repair_flushes_deferred_before_next_assistant_batch() {
        // A new assistant tool_calls message arriving while the previous batch
        // is still unanswered: deferred messages are flushed before it.
        let messages = vec![
            user_msg("start"),
            assistant_multi_tool_call_msg(&[("call_0", "read"), ("call_1", "read")]),
            tool_msg("call_0", "result 0"),
            image_user_msg("a.png"),
            assistant_tool_call_msg("call_2", "bash"),
            tool_msg("call_2", "ok"),
        ];
        let repaired = repair_interleaved_tool_responses(messages);
        let role_kinds: Vec<&str> = repaired
            .iter()
            .map(|m| match m {
                ChatCompletionRequestMessage::User(_) => "user",
                ChatCompletionRequestMessage::Assistant(_) => "assistant",
                ChatCompletionRequestMessage::Tool(_) => "tool",
                _ => "other",
            })
            .collect();
        assert_eq!(
            role_kinds,
            vec!["user", "assistant", "tool", "user", "assistant", "tool"]
        );
    }
}
