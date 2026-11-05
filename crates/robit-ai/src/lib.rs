//! robit-ai: LLM API layer for the Robit framework.
//!
//! Provides a unified `LlmClient` for interacting with multiple LLM providers
//! through the OpenAI-compatible protocol. Configuration is loaded from
//! `.robit/config.toml` (project-local) or `~/.robit/config.toml` (global).

pub mod client;
pub mod config;
pub mod error;
pub mod logging;
pub mod stream;

// Re-export async-openai core types so downstream crates don't need to depend on async-openai directly.
pub use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionRequestAssistantMessage,
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestUserMessage, ChatCompletionRequestUserMessageContent,
    ChatCompletionRequestUserMessageContentPart,
    ChatCompletionRequestMessageContentPartText,
    ChatCompletionRequestMessageContentPartImage,
    ChatCompletionTools,
    CompletionUsage, CreateChatCompletionResponse, CreateChatCompletionStreamResponse, Role,
};

pub use client::LlmClient;
pub use config::{
    load_config, load_env, AppConfig, BotConfig, ChannelsConfig, ConfirmKeywordsConfig,
    ContextConfig, ImageCallMode, ImageModelConfig, ImageProtocol, ImageProviderConfig, ModelConfig,
    ProviderConfig, QqBotConfig, ResolvedImageProvider, ResolvedModel, RetryConfig, RobitConfig,
    ThinkingMode, resolve_image_provider, resolve_vision_model,
};
pub use stream::{RobitChatStream, StreamChunk};
pub use error::LlmError;
pub use logging::{
    build_log_filter, init_logging, init_logging_silent, install_panic_hook, log_file_layer,
};
