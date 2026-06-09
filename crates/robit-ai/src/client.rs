//! LlmClient: a thin wrapper around async-openai with unified config support.

use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionResponseStream, ChatCompletionTools,
    CreateChatCompletionRequest, CreateChatCompletionResponse,
};

use crate::config::{resolve_profile, ResolvedModel, RobitConfig};
use crate::error::LlmError;

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
}
