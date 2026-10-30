//! Error types for the robit-agent crate.

use thiserror::Error;

/// Unified error type for Agent operations.
#[derive(Debug, Error)]
pub enum AgentError {
    #[error(transparent)]
    LlmError(#[from] robit_ai::LlmError),

    #[error(transparent)]
    DbError(#[from] rusqlite::Error),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Tool execution error: {0}")]
    ToolError(String),

    /// Host-domain error propagated out of a host tool's `execute` via `?`.
    ///
    /// robit provides `From<anyhow::Error>` so hosts can use `?` on their
    /// `anyhow`-based fallible calls without a local conversion macro (they
    /// can't `impl From` themselves — `AgentError` belongs to robit). The
    /// `ToolRegistry` turns this into an error `ToolResult` the LLM can read.
    #[error(transparent)]
    External(#[from] anyhow::Error),

    #[error("Context overflow: current {current} tokens, max {max} tokens")]
    ContextOverflow { current: usize, max: usize },

    #[error("Agent internal error: {0}")]
    InternalError(String),
}

pub type Result<T> = std::result::Result<T, AgentError>;

#[cfg(test)]
mod tests {
    use super::*;

    /// `From<anyhow::Error> for AgentError` — the enabler for `?` on
    /// anyhow-based calls inside a host tool's `execute`.
    #[test]
    fn agent_error_from_anyhow_enables_question_mark() {
        let err: AgentError = anyhow::anyhow!("boom").into();
        assert!(matches!(err, AgentError::External(_)));

        // The host pattern this unlocks:
        fn fallible() -> anyhow::Result<()> {
            Err(anyhow::anyhow!("boom"))
        }
        fn host_tool_body() -> Result<()> {
            fallible()?;
            Ok(())
        }
        assert!(host_tool_body().is_err());
    }
}
