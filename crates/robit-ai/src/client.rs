//! LlmClient: a thin wrapper around async-openai with unified config support.

use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionResponseStream, ChatCompletionTools,
    CreateChatCompletionRequest, CreateChatCompletionResponse,
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
        // Validate and filter messages before sending to LLM
        let messages = validate_and_filter_messages(messages);

        let request = CreateChatCompletionRequest {
            model: self.model.clone(),
            messages,
            tools,
            stream: Some(true),
            max_completion_tokens: self.resolved.max_tokens,
            temperature: self.resolved.temperature,
            ..Default::default()
        };

        let stream = self.client.chat().create_stream(request).await?;
        Ok(stream)
    }

    /// Non-streaming chat completion. Returns the full response.
    pub async fn chat(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Option<Vec<ChatCompletionTools>>,
    ) -> Result<CreateChatCompletionResponse, LlmError> {
        // Validate and filter messages before sending to LLM
        let messages = validate_and_filter_messages(messages);

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
