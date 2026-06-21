//! robit-ai: LLM API layer for the Robit framework.
//!
//! Provides a unified `LlmClient` for interacting with multiple LLM providers
//! through the OpenAI-compatible protocol. Configuration is loaded from
//! `.robit/config.toml` (project-local) or `~/.robit/config.toml` (global).

pub mod client;
pub mod config;
pub mod error;

// Re-export async-openai core types so downstream crates don't need to depend on async-openai directly.
pub use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionRequestAssistantMessage,
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestUserMessage, ChatCompletionRequestUserMessageContent,
    ChatCompletionRequestUserMessageContentPart,
    ChatCompletionRequestMessageContentPartText,
    ChatCompletionRequestMessageContentPartImage,
    ChatCompletionResponseStream, ChatCompletionTools,
    CompletionUsage, CreateChatCompletionResponse, CreateChatCompletionStreamResponse, Role,
};

pub use client::LlmClient;
pub use config::{
    load_config, load_env, AppConfig, BotConfig, ChannelsConfig, ConfirmKeywordsConfig,
    ContextConfig, ModelConfig, ProviderConfig, QqBotConfig, ResolvedModel, RetryConfig,
    RobitConfig,
};
pub use error::LlmError;
