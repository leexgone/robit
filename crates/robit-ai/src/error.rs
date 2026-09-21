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

    /// The provider's content moderation blocked the model OUTPUT, pushed as
    /// an error event into the SSE stream (no `choices` field, so async-openai
    /// reports a JSON deserialization failure with the cause buried inside).
    #[error("内容审核拦截：模型输出被服务商判定为不适宜内容，请调整提问或稍后重试（{detail}）")]
    ContentModeration { detail: String },

    /// The provider's content moderation blocked the INPUT (the request /
    /// conversation context was rejected with e.g. HTTP 400). Every subsequent
    /// turn resends the same context, so the session stays blocked until the
    /// context is cleared.
    #[error("内容审核拦截：输入内容被服务商判定为不适宜内容，请调整提问或清空上下文/新建会话后重试（{detail}）")]
    ContentModerationInput { detail: String },

    #[error("服务商返回错误（{code}）：{message}")]
    ProviderError { code: String, message: String },
}

impl LlmError {
    /// Map an async-openai error to a friendlier one where possible.
    ///
    /// Two cases are improved:
    /// - `ApiError` whose code/type is a known content-moderation code
    ///   (e.g. DashScope `data_inspection_failed`) — the provider rejects the
    ///   request outright with HTTP 400.
    /// - `JSONDeserialize` where the raw payload is actually a provider error
    ///   event `{"error": {...}}` pushed into the SSE stream — async-openai
    ///   cannot parse it as a chunk (no `choices`), so the real cause would
    ///   otherwise be buried in the deserialize error.
    ///
    /// Everything else is passed through unchanged.
    pub fn from_openai_error(err: async_openai::error::OpenAIError) -> Self {
        use async_openai::error::OpenAIError;
        match err {
            OpenAIError::ApiError(resp) => {
                let api = &resp.api_error;
                match api
                    .code
                    .as_deref()
                    .or_else(|| api.r#type.as_deref())
                {
                    Some(code) if is_content_moderation_code(code) => {
                        moderation_error(code, &api.message)
                    }
                    // Other API errors keep their structured display
                    // ("{status} {type}: {message} (code: ...)").
                    _ => LlmError::OpenAiError(OpenAIError::ApiError(resp)),
                }
            }
            OpenAIError::JSONDeserialize(_, ref raw) => match extract_provider_error(raw) {
                Some((Some(code), message)) => {
                    if is_content_moderation_code(&code) {
                        moderation_error(&code, &message)
                    } else {
                        LlmError::ProviderError { code, message }
                    }
                }
                Some((None, message)) => LlmError::ProviderError {
                    code: "unknown".to_string(),
                    message,
                },
                None => LlmError::OpenAiError(err),
            },
            other => LlmError::OpenAiError(other),
        }
    }
}

/// Build the right moderation error for a provider payload, distinguishing
/// input-side from output-side blocks (DashScope uses the same code for both;
/// only the message differs, e.g. "Input data may contain ..." vs "Output").
fn moderation_error(code: &str, message: &str) -> LlmError {
    let detail = format!("{}: {}", code, message);
    if message.contains("Input data") {
        LlmError::ContentModerationInput { detail }
    } else {
        LlmError::ContentModeration { detail }
    }
}

/// Whether a provider error code means content moderation blocked the output.
fn is_content_moderation_code(code: &str) -> bool {
    matches!(
        code,
        // DashScope (QWen): output data inspection
        "data_inspection_failed" | "DataInspectionFailed" |
        // OpenAI-compatible content filters
        "content_filter" | "content_policy_violation"
    )
}

/// Extract `(code, message)` from a provider error payload like
/// `{"error": {"code": "...", "message": "..."}}`.
fn extract_provider_error(raw: &str) -> Option<(Option<String>, String)> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let err = value.get("error")?;
    let message = err.get("message")?.as_str()?.to_string();
    let code = err
        .get("code")
        .and_then(|c| c.as_str())
        .or_else(|| err.get("type").and_then(|t| t.as_str()))
        .map(|s| s.to_string());
    Some((code, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real-world DashScope payload: moderation blocked the model output
    /// mid-stream (no `choices` field, so async-openai fails deserialization).
    const DASHSCOPE_MODERATION: &str = "{\"error\":{\"message\":\"Output data may contain inappropriate content. For details, see: https://help.aliyun.com/zh/model-studio/error-code#inappropriate-content\",\"type\":\"data_inspection_failed\",\"param\":null,\"code\":\"data_inspection_failed\"},\"id\":\"chatcmpl-aaadc65d\",\"request_id\":\"aaadc65d\"}";

    fn json_deserialize_err(raw: &str) -> async_openai::error::OpenAIError {
        use serde::de::Error as _;
        let serde_err = serde_json::Error::custom("missing field `choices`");
        async_openai::error::OpenAIError::JSONDeserialize(serde_err, raw.to_string())
    }

    fn api_err(code: &str, message: &str) -> async_openai::error::OpenAIError {
        let api_error = async_openai::error::ApiError {
            message: message.to_string(),
            r#type: Some("data_inspection_failed".to_string()),
            param: None,
            code: Some(code.to_string()),
            misalignment: None,
        };
        async_openai::error::OpenAIError::ApiError(async_openai::error::ApiErrorResponse {
            status_code: reqwest::StatusCode::BAD_REQUEST,
            api_error,
        })
    }

    #[test]
    fn stream_moderation_error_becomes_friendly_content_moderation() {
        let err = json_deserialize_err(DASHSCOPE_MODERATION);
        let llm = LlmError::from_openai_error(err);
        match &llm {
            LlmError::ContentModeration { detail } => {
                assert!(detail.contains("data_inspection_failed"));
                assert!(detail.contains("inappropriate content"));
            }
            other => panic!("expected ContentModeration, got {:?}", other),
        }
        // Display is friendly and contains the provider's own message.
        let text = llm.to_string();
        assert!(text.contains("内容审核拦截"));
        assert!(text.contains("请调整提问或稍后重试"));
    }

    #[test]
    fn api_input_moderation_error_becomes_friendly_input_moderation() {
        // HTTP 400 rejection: the conversation context (input) is flagged.
        let err = api_err(
            "data_inspection_failed",
            "Input data may contain inappropriate content. For details, see: https://help.aliyun.com/zh/model-studio/error-code#inappropriate-content",
        );
        let llm = LlmError::from_openai_error(err);
        match &llm {
            LlmError::ContentModerationInput { detail } => {
                assert!(detail.contains("data_inspection_failed"));
            }
            other => panic!("expected ContentModerationInput, got {:?}", other),
        }
        let text = llm.to_string();
        assert!(text.contains("输入内容"));
        assert!(text.contains("清空上下文"));
    }

    #[test]
    fn api_output_moderation_error_becomes_friendly_output_moderation() {
        let err = api_err(
            "data_inspection_failed",
            "Output data may contain inappropriate content. For details, see: https://help.aliyun.com/zh/model-studio/error-code#inappropriate-content",
        );
        let llm = LlmError::from_openai_error(err);
        assert!(matches!(llm, LlmError::ContentModeration { .. }));
    }

    #[test]
    fn api_other_error_keeps_structured_display() {
        let api_error = async_openai::error::ApiError {
            message: "Model not found".to_string(),
            r#type: Some("invalid_request_error".to_string()),
            param: None,
            code: Some("model_not_found".to_string()),
            misalignment: None,
        };
        let err = async_openai::error::OpenAIError::ApiError(async_openai::error::ApiErrorResponse {
            status_code: reqwest::StatusCode::NOT_FOUND,
            api_error,
        });
        // Non-moderation API errors pass through unchanged.
        assert!(matches!(LlmError::from_openai_error(err), LlmError::OpenAiError(_)));
    }

    #[test]
    fn stream_provider_error_keeps_code_and_message() {
        let raw = "{\"error\":{\"message\":\"boom\",\"code\":\"internal_error\"}}";
        let llm = LlmError::from_openai_error(json_deserialize_err(raw));
        match &llm {
            LlmError::ProviderError { code, message } => {
                assert_eq!(code, "internal_error");
                assert_eq!(message, "boom");
            }
            other => panic!("expected ProviderError, got {:?}", other),
        }
    }

    #[test]
    fn stream_unparseable_payload_falls_back_to_openai_error() {
        let raw = "not json at all";
        let llm = LlmError::from_openai_error(json_deserialize_err(raw));
        assert!(matches!(llm, LlmError::OpenAiError(_)));
    }
}
