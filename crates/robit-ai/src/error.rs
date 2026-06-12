//! Error types for the robit-ai crate.

use thiserror::Error;

/// Unified error type for LLM operations.
#[derive(Debug, Error)]
pub enum LlmError {
    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Network connection failed: {0}")]
    ConnectionError(String),

    #[error("Authentication failed, please check your API key configuration")]
    AuthenticationError,

    #[error("Rate limit exceeded, please retry later")]
    RateLimitError { retry_after: Option<u64> },

    #[error("Model not available: {model}")]
    ModelNotFound { model: String },

    #[error("Server error ({status}): {message}")]
    ServerError { status: u16, message: String },

    #[error("Response format error: {0}")]
    ParseError(String),

    #[error(transparent)]
    OpenAiError(#[from] async_openai::error::OpenAIError),
}
