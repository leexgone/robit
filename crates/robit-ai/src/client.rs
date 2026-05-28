//! LlmClient: a thin wrapper around async-openai with multi-provider config support.

use async_openai::config::OpenAIConfig;
use async_openai::types::{
    ChatCompletionRequestMessage, ChatCompletionResponseStream, ChatCompletionTool,
    CreateChatCompletionRequest, CreateChatCompletionResponse,
};

use crate::config::{LlmConfig, ResolvedModel, SettingsConfig, resolve_model};
use crate::error::LlmError;

pub struct LlmClient {
    client: async_openai::Client<OpenAIConfig>,
    model: String,
    resolved: ResolvedModel,
}

impl LlmClient {
    /// Create a new LlmClient from loaded configuration.
    pub fn from_config(
        llm_config: &LlmConfig,
        settings: &SettingsConfig,
    ) -> Result<Self, LlmError> {
        let resolved = resolve_model(llm_config, settings)?;

        let config = OpenAIConfig::new()
            .with_api_base(&resolved.base_url)
            .with_api_key(&resolved.api_key);

        let client = async_openai::Client::with_config(config);

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
        tools: Option<Vec<ChatCompletionTool>>,
    ) -> Result<ChatCompletionResponseStream, LlmError> {
        let request = CreateChatCompletionRequest {
            model: self.model.clone(),
            messages,
            tools,
            stream: Some(true),
            ..Default::default()
        };

        let stream = self.client.chat().create_stream(request).await?;
        Ok(stream)
    }

    /// Non-streaming chat completion. Returns the full response.
    pub async fn chat(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Option<Vec<ChatCompletionTool>>,
    ) -> Result<CreateChatCompletionResponse, LlmError> {
        let request = CreateChatCompletionRequest {
            model: self.model.clone(),
            messages,
            tools,
            ..Default::default()
        };

        let response = self.client.chat().create(request).await?;
        Ok(response)
    }

    /// Get the current model ID (e.g. "deepseek-chat").
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Get the provider key (e.g. "deepseek").
    pub fn provider(&self) -> &str {
        &self.resolved.provider_key
    }
}
