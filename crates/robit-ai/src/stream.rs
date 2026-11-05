//! Streaming chat completion paths: legacy typed adapter + raw SSE bypass.
//!
//! Two coexisting paths, selected per-model by the `thinking` config
//! (`ThinkingMode`):
//!
//! - `auto` (default) → [`typed_stream`]: async-openai's own `create_stream`,
//!   wrapped so the frame type is uniform. Behavior is byte-identical to
//!   pre-bypass releases; `delta.reasoning_content`, if a provider sends one,
//!   is dropped inside async-openai as before.
//! - `on` / `off` → the raw SSE bypass ([`sse_stream`] + [`parse_frame`]):
//!   robit-ai owns framing, injects `enable_thinking` into the request body
//!   (impossible through the typed struct), and captures `reasoning_content`.
//!
//! Why the bypass exists at all: async-openai's
//! `ChatCompletionStreamResponseDelta` cannot carry vendor-extension fields —
//! no such field and no `#[serde(flatten)]` catch-all, so serde silently
//! drops the key *inside* the dependency's deserialization, before any robit
//! code can observe the frame. Upstream has repeatedly closed additive PRs
//! for non-official-spec fields as out of scope (see
//! docs/robit-design-thinking-output.md §D1), so this module is the permanent
//! home for OpenAI-compatible vendor extensions.
//!
//! Design: on the bypass, standard fields are still deserialized by the
//! async-openai typed model (`CreateChatCompletionStreamResponse`) so frame
//! semantics and the error classification paths
//! (`LlmError::from_openai_error` over `JSONDeserialize(raw)`) stay identical
//! to the legacy path; non-standard per-frame deltas are additionally
//! extracted from the raw JSON.

use std::ops::Deref;
use std::pin::Pin;

use async_openai::types::chat::CreateChatCompletionStreamResponse;
use eventsource_stream::Eventsource;
use futures_util::stream::{Stream, StreamExt};

use crate::error::LlmError;

/// One streamed chat completion frame.
///
/// `Deref` to the async-openai typed frame keeps existing access paths
/// (`chunk.usage`, `chunk.choices`, …) working unchanged for consumers that
/// never care about reasoning; `reasoning` carries the vendor extension.
#[derive(Debug, Clone)]
pub struct StreamChunk {
    /// The standard frame as parsed by async-openai's typed model.
    pub response: CreateChatCompletionStreamResponse,
    /// `delta.reasoning_content` of the first choice, if this frame carried
    /// one (thinking-phase increments from QWen/DeepSeek-R1/Kimi compatible
    /// surfaces). Display-only: never write back to conversation history.
    pub reasoning: Option<String>,
}

impl Deref for StreamChunk {
    type Target = CreateChatCompletionStreamResponse;

    fn deref(&self) -> &Self::Target {
        &self.response
    }
}

/// Boxed stream of parsed chat completion frames.
pub type RobitChatStream =
    Pin<Box<dyn Stream<Item = Result<StreamChunk, LlmError>> + Send>>;

/// Parse one SSE `data:` payload.
///
/// Returns `Ok(None)` for the `[DONE]` sentinel (stream terminator) and for
/// blank keep-alive payloads. A frame that fails the typed deserialization
/// goes through the *same* classification the old typed pass-through applied —
/// providers push `{"error": ...}` events into the stream (e.g. DashScope
/// content moderation mid-stream) which would otherwise surface as a bare
/// JSON deserialization failure instead of the friendly error.
///
/// Pure function: unit-testable without any server.
pub(crate) fn parse_frame(raw: &str) -> Result<Option<StreamChunk>, LlmError> {
    if raw.trim() == "[DONE]" {
        return Ok(None);
    }
    match serde_json::from_str::<CreateChatCompletionStreamResponse>(raw) {
        Ok(response) => {
            // Second, cheap parse to peek the vendor field the typed struct
            // cannot hold. Missing/null/non-string all normalize to None.
            let reasoning = serde_json::from_str::<serde_json::Value>(raw)
                .ok()
                .and_then(|v| first_choice_reasoning(&v));
            Ok(Some(StreamChunk { response, reasoning }))
        }
        Err(e) => Err(LlmError::from_openai_error(
            async_openai::error::OpenAIError::JSONDeserialize(e, raw.to_string()),
        )),
    }
}

/// Extract `choices[0].delta.reasoning_content` (see [`StreamChunk::reasoning`]).
fn first_choice_reasoning(value: &serde_json::Value) -> Option<String> {
    value
        .get("choices")?
        .get(0)?
        .get("delta")?
        .get("reasoning_content")?
        .as_str()
        .map(str::to_string)
}

/// Adapt async-openai's legacy typed stream to the uniform chunk type.
///
/// Used when `thinking = auto` (the default): framing, deserialization and
/// error surfaces remain exactly what async-openai produced before the
/// bypass existed; `reasoning` is always `None` because the typed delta
/// cannot carry it.
pub(crate) fn typed_stream(
    stream: async_openai::types::chat::ChatCompletionResponseStream,
) -> RobitChatStream {
    Box::pin(stream.map(|item| {
        item.map(|response| StreamChunk {
            response,
            reasoning: None,
        })
        .map_err(LlmError::from_openai_error)
    }))
}

/// Wrap a reqwest byte stream into the parsed chat completion stream.
pub(crate) fn sse_stream<S>(bytes: S) -> RobitChatStream
where
    S: Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
{
    Box::pin(
        bytes.eventsource().filter_map(|event| async move {
            match event {
                // Transport-level failure mid-stream (connection dropped etc.).
                Err(e) => Some(Err(LlmError::ConnectionError(format!(
                    "SSE stream read failed: {e}"
                )))),
                Ok(ev) => {
                    let data = ev.data;
                    if data.is_empty() {
                        // Blank `data:` payload (heartbeat-like); no frame.
                        return None;
                    }
                    parse_frame(&data).transpose()
                }
            }
        }),
    )
}

/// Classify a non-2xx streaming response body.
///
/// Mirrors the old typed path: async-openai parsed the error object into its
/// `ApiError` and surfaced it as `OpenAIError::ApiError`, which
/// `LlmError::from_openai_error` then turned into friendly moderation errors
/// (e.g. DashScope rejecting the *input* with HTTP 400). Anything outside that
/// shape (gateway HTML pages, plain-text bodies) falls back to
/// [`LlmError::from_sse_payload`].
pub(crate) fn classify_http_status(status: reqwest::StatusCode, raw: &str) -> LlmError {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(err_obj) = value.get("error") {
            if let Ok(api_error) =
                serde_json::from_value::<async_openai::error::ApiError>(err_obj.clone())
            {
                return LlmError::from_openai_error(async_openai::error::OpenAIError::ApiError(
                    async_openai::error::ApiErrorResponse {
                        status_code: status,
                        api_error,
                    },
                ));
            }
        }
    }
    LlmError::from_sse_payload(Some(status.as_u16()), raw)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn done_sentinel_ends_stream() {
        assert!(parse_frame("[DONE]").unwrap().is_none());
        assert!(parse_frame(" [DONE] ").unwrap().is_none());
    }

    #[test]
    fn thinking_phase_frame_carries_reasoning_and_empty_content() {
        // Exact DashScope compatible-mode shape: content:"" + reasoning_content
        // in the same frame (the root cause of the old blank-text_delta frames).
        let raw = json!({
            "id": "c1", "object": "chat.completion.chunk", "created": 1,
            "model": "qwen3-max",
            "choices": [{"index": 0, "delta": {"content": "", "reasoning_content": "思考甲"}, "finish_reason": null}]
        })
        .to_string();
        let chunk = parse_frame(&raw).unwrap().expect("frame");
        assert_eq!(chunk.reasoning.as_deref(), Some("思考甲"));
        // Deref compatibility: typed access paths keep working.
        assert_eq!(chunk.response.choices[0].delta.content.as_deref(), Some(""));
        assert_eq!(chunk.choices[0].delta.role, None);
    }

    #[test]
    fn content_frame_with_null_reasoning_yields_none() {
        let raw = json!({
            "id": "c1", "object": "chat.completion.chunk", "created": 1,
            "model": "qwen3-max",
            "choices": [{"index": 0, "delta": {"content": "正文乙", "reasoning_content": null}, "finish_reason": null}]
        })
        .to_string();
        let chunk = parse_frame(&raw).unwrap().expect("frame");
        assert_eq!(chunk.reasoning, None);
        assert_eq!(chunk.response.choices[0].delta.content.as_deref(), Some("正文乙"));
    }

    #[test]
    fn frame_without_reasoning_key_yields_none() {
        let raw = json!({
            "id": "c1", "object": "chat.completion.chunk", "created": 1,
            "model": "deepseek-chat",
            "choices": [{"index": 0, "delta": {"content": "hi"}, "finish_reason": null}]
        })
        .to_string();
        let chunk = parse_frame(&raw).unwrap().expect("frame");
        assert_eq!(chunk.reasoning, None);
    }

    #[test]
    fn usage_tail_chunk_without_delta_parses() {
        // DeepSeek/OpenAI style: final usage chunk with empty choices array.
        let raw = json!({
            "id": "c1", "object": "chat.completion.chunk", "created": 1,
            "model": "deepseek-chat", "choices": [],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        })
        .to_string();
        let chunk = parse_frame(&raw).unwrap().expect("frame");
        assert_eq!(chunk.usage.as_ref().map(|u| u.total_tokens), Some(15));
        assert_eq!(chunk.reasoning, None);
    }

    #[test]
    fn in_band_moderation_error_keeps_friendly_surface() {
        // Real DashScope payload (same corpus as error.rs tests): no `choices`,
        // typed parse fails, classification must still yield ContentModeration.
        const DASHSCOPE_MODERATION: &str = "{\"error\":{\"message\":\"Output data may contain inappropriate content. For details, see: https://help.aliyun.com/zh/model-studio/error-code#inappropriate-content\",\"type\":\"data_inspection_failed\",\"param\":null,\"code\":\"data_inspection_failed\"},\"id\":\"chatcmpl-aaadc65d\",\"request_id\":\"aaadc65d\"}";
        let err = parse_frame(DASHSCOPE_MODERATION).unwrap_err();
        match &err {
            LlmError::ContentModeration { detail } => {
                assert!(detail.contains("data_inspection_failed"));
            }
            other => panic!("expected ContentModeration, got {other:?}"),
        }
        assert!(err.to_string().contains("内容审核拦截"));
    }

    #[test]
    fn garbled_frame_is_parse_error() {
        let err = parse_frame("not json at all").unwrap_err();
        // Unrecognized raw payloads pass through as OpenAiError(JSONDeserialize),
        // same as the old typed path.
        assert!(matches!(err, LlmError::OpenAiError(_)));
    }

    #[test]
    fn classify_http_status_moderation_input_maps_to_input_moderation() {
        let raw = "{\"error\":{\"message\":\"Input data may contain inappropriate content.\",\"type\":\"data_inspection_failed\",\"param\":null,\"code\":\"data_inspection_failed\"}}";
        let err = classify_http_status(reqwest::StatusCode::BAD_REQUEST, raw);
        assert!(matches!(err, LlmError::ContentModerationInput { .. }));
    }

    #[test]
    fn classify_http_status_gateway_html_maps_to_gateway_error() {
        let raw = "<html><head><title>413 Request Entity Too Large</title></head><body>…</body></html>";
        let err = classify_http_status(reqwest::StatusCode::from_u16(413).unwrap(), raw);
        match &err {
            LlmError::GatewayError { title, hint } => {
                assert!(title.contains("413"));
                assert!(hint.contains("请求体过大"));
            }
            other => panic!("expected GatewayError, got {other:?}"),
        }
    }

    #[test]
    fn classify_http_status_unauthorized_maps_to_auth_error() {
        let err = classify_http_status(reqwest::StatusCode::UNAUTHORIZED, "{\"detail\":\"bad key\"}");
        assert!(matches!(err, LlmError::AuthenticationError));
    }
}
