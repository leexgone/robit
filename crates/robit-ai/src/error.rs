//! Error types for the robit-ai crate.

use thiserror::Error;

/// Unified error type for LLM operations.
#[derive(Debug, Error)]
pub enum LlmError {
    #[error("配置错误: {0}")]
    ConfigError(String),

    #[error("网络连接失败: {0}")]
    ConnectionError(String),

    #[error("认证失败，请检查 API Key 配置")]
    AuthenticationError,

    #[error("请求速率受限，请稍后重试")]
    RateLimitError { retry_after: Option<u64> },

    #[error("模型不可用: {model}")]
    ModelNotFound { model: String },

    #[error("服务端错误 ({status}): {message}")]
    ServerError { status: u16, message: String },

    #[error("响应格式错误: {0}")]
    ParseError(String),

    #[error(transparent)]
    OpenAiError(#[from] async_openai::error::OpenAIError),
}
