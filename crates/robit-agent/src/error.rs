//! Error types for the robit-agent crate.

use thiserror::Error;

/// Unified error type for Agent operations.
#[derive(Debug, Error)]
pub enum AgentError {
    #[error(transparent)]
    LlmError(#[from] robit_ai::LlmError),

    #[error("配置错误: {0}")]
    ConfigError(String),

    #[error("工具执行错误: {0}")]
    ToolError(String),

    #[error("上下文溢出: 当前 {current} tokens，上限 {max} tokens")]
    ContextOverflow { current: usize, max: usize },

    #[error("Agent 内部错误: {0}")]
    InternalError(String),
}

pub type Result<T> = std::result::Result<T, AgentError>;
