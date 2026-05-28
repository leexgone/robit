//! robit-ai: LLM API layer for the Robit framework.
//!
//! Provides a unified `LlmClient` for interacting with multiple LLM providers
//! through the OpenAI-compatible protocol. Configuration is loaded from
//! `~/.robit/llms.toml` and `~/.robit/settings.toml`.

pub mod client;
pub mod config;
pub mod error;

// Re-export async-openai core types so downstream crates don't need to depend on async-openai directly.
pub use async_openai::types::{
    ChatCompletionMessageToolCall, ChatCompletionRequestAssistantMessage,
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestUserMessage, ChatCompletionResponseStream, ChatCompletionTool,
    CompletionUsage, CreateChatCompletionResponse, CreateChatCompletionStreamResponse, Role,
};

pub use client::LlmClient;
pub use config::{
    ContextConfig, LlmConfig, ModelConfig, ProviderConfig, ResolvedModel, RetryConfig,
    SettingsConfig,
};
pub use error::LlmError;
