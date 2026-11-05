//! Integration tests for the streaming chat-completion paths (legacy typed
//! adapter + raw SSE bypass) against a mock provider.
//!
//! Covers: `enable_thinking` injection per `ThinkingMode`, end-to-end
//! `reasoning_content` capture (RFC acceptance 1), legacy-path invariance for
//! `auto`, and the error-classification surfaces (HTTP-level and in-band).

use futures_util::StreamExt;
use robit_ai::{
    ChatCompletionRequestMessage, ChatCompletionRequestUserMessage, LlmClient, LlmError,
    ResolvedModel, ThinkingMode,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(base_url: String, thinking: ThinkingMode) -> LlmClient {
    LlmClient::from_resolved(ResolvedModel {
        profile_name: "test".into(),
        model_id: "test-model".into(),
        base_url,
        api_key: "test-key".into(),
        max_tokens: None,
        temperature: None,
        context_window: None,
        supports_images: false,
        supports_tools: true,
        thinking,
    })
}

fn user_msg() -> Vec<ChatCompletionRequestMessage> {
    vec![ChatCompletionRequestMessage::User(
        ChatCompletionRequestUserMessage {
            content: "hi".into(),
            name: None,
        },
    )]
}

/// Build an SSE body from `data:` payloads, terminated by `[DONE]`.
fn sse(frames: &[&str]) -> String {
    let mut s = String::new();
    for f in frames {
        s.push_str("data: ");
        s.push_str(f);
        s.push_str("\n\n");
    }
    s.push_str("data: [DONE]\n\n");
    s
}

async fn mock_sse_server(body: String) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    server
}

async fn request_body(server: &MockServer) -> serde_json::Value {
    let requests = server.received_requests().await.expect("requests recorded");
    serde_json::from_slice(&requests[0].body).expect("json body")
}

/// RFC acceptance 1, verbatim: two-phase frames —
/// `{"content":"","reasoning_content":"思考甲"}` then
/// `{"content":"正文乙","reasoning_content":null}` — must reach the consumer
/// lossless and in order, without disturbing `content` parsing.
#[tokio::test]
async fn thinking_on_injects_field_and_captures_reasoning_end_to_end() {
    let frames = [
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"role":"assistant","content":"","reasoning_content":"思考甲"},"finish_reason":null}]}"#,
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"content":"正文乙","reasoning_content":null},"finish_reason":null}]}"#,
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}"#,
    ];
    let server = mock_sse_server(sse(&frames)).await;
    let client = client(server.uri(), ThinkingMode::On);

    let mut stream = client.chat_stream(user_msg(), None).await.expect("stream");
    let chunks: Vec<_> = stream
        .by_ref()
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|r| r.expect("chunk parses"))
        .collect();

    // Request-side: enable_thinking injected, include_usage preserved.
    let body = request_body(&server).await;
    assert_eq!(
        body["enable_thinking"],
        serde_json::json!(true),
        "thinking=on must inject enable_thinking:true"
    );
    assert_eq!(
        body["stream_options"]["include_usage"],
        serde_json::json!(true),
        "usage tail-frame request must survive the bypass"
    );

    // Response-side: lossless, in-order capture; content path untouched.
    assert_eq!(chunks.len(), 3, "[DONE] must terminate the stream");
    assert_eq!(chunks[0].reasoning.as_deref(), Some("思考甲"));
    assert_eq!(chunks[0].choices[0].delta.content.as_deref(), Some(""));
    assert_eq!(chunks[1].reasoning, None);
    assert_eq!(chunks[1].choices[0].delta.content.as_deref(), Some("正文乙"));
    assert_eq!(
        chunks[2].usage.as_ref().map(|u| u.total_tokens),
        Some(5),
        "usage tail frame reachable through Deref"
    );
}

#[tokio::test]
async fn thinking_off_injects_false() {
    let frames = [
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]}"#,
    ];
    let server = mock_sse_server(sse(&frames)).await;
    let client = client(server.uri(), ThinkingMode::Off);
    let mut stream = client.chat_stream(user_msg(), None).await.expect("stream");
    let chunk = stream.next().await.expect("frame").expect("parses");
    assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("ok"));

    let body = request_body(&server).await;
    assert_eq!(
        body["enable_thinking"],
        serde_json::json!(false),
        "thinking=off must inject enable_thinking:false"
    );
}

/// `auto` must keep the legacy typed path: no `enable_thinking` in the body,
/// and `reasoning_content` dropped inside async-openai exactly as before
/// (this is the zero-regression guarantee for every unconfigured model).
#[tokio::test]
async fn thinking_auto_keeps_legacy_typed_path() {
    let frames = [
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"role":"assistant","content":"","reasoning_content":"思考甲"},"finish_reason":null}]}"#,
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"content":"正文乙"},"finish_reason":"stop"}]}"#,
    ];
    let server = mock_sse_server(sse(&frames)).await;
    let client = client(server.uri(), ThinkingMode::Auto);

    let stream = client.chat_stream(user_msg(), None).await.expect("stream");
    let chunks: Vec<_> = stream
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|r| r.expect("chunk parses"))
        .collect();

    let body = request_body(&server).await;
    assert!(
        body.get("enable_thinking").is_none(),
        "auto must not inject enable_thinking (gateways rejecting unknown fields stay safe)"
    );
    assert_eq!(
        chunks[0].reasoning, None,
        "legacy typed path drops reasoning_content, as before"
    );
    assert_eq!(chunks[0].choices[0].delta.content.as_deref(), Some(""));
    assert_eq!(chunks[1].choices[0].delta.content.as_deref(), Some("正文乙"));
}

/// HTTP-level content moderation (input side, DashScope 400) keeps its
/// friendly classification on the bypass path.
#[tokio::test]
async fn http_400_input_moderation_maps_to_friendly_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(400)
                .insert_header("content-type", "application/json")
                .set_body_string(
                    r#"{"error":{"message":"Input data may contain inappropriate content.","type":"data_inspection_failed","param":null,"code":"data_inspection_failed"}}"#,
                ),
        )
        .mount(&server)
        .await;

    let client = client(server.uri(), ThinkingMode::On);
    let err = match client.chat_stream(user_msg(), None).await {
        Err(e) => e,
        Ok(_) => panic!("expected a moderation error, got a stream"),
    };
    assert!(
        matches!(err, LlmError::ContentModerationInput { .. }),
        "expected ContentModerationInput, got {err:?}"
    );
    assert!(err.to_string().contains("输入内容"));
}

/// In-band `{"error": ...}` frames pushed into the SSE stream (DashScope
/// output moderation) surface as the friendly ContentModeration error,
/// matching the pre-bypass behavior tested in error.rs.
#[tokio::test]
async fn in_band_moderation_frame_surfaces_as_stream_error() {
    const DASHSCOPE_MODERATION: &str = r#"{"error":{"message":"Output data may contain inappropriate content. For details, see: https://help.aliyun.com/zh/model-studio/error-code#inappropriate-content","type":"data_inspection_failed","param":null,"code":"data_inspection_failed"},"id":"chatcmpl-aaadc65d","request_id":"aaadc65d"}"#;
    let server = mock_sse_server(sse(&[DASHSCOPE_MODERATION])).await;
    let client = client(server.uri(), ThinkingMode::On);

    let mut stream = client.chat_stream(user_msg(), None).await.expect("stream");
    let first = stream.next().await.expect("error frame");
    let err = first.unwrap_err();
    assert!(
        matches!(err, LlmError::ContentModeration { .. }),
        "expected ContentModeration, got {err:?}"
    );
    assert!(err.to_string().contains("内容审核拦截"));
}

/// Gateway HTML error pages (e.g. openresty 413 with large image contexts)
/// keep their friendly GatewayError classification on the bypass path.
#[tokio::test]
async fn gateway_html_error_maps_to_gateway_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(413)
                .insert_header("content-type", "text/html")
                .set_body_string(
                    "<html><head><title>413 Request Entity Too Large</title></head><body>x</body></html>",
                ),
        )
        .mount(&server)
        .await;

    let client = client(server.uri(), ThinkingMode::On);
    let err = match client.chat_stream(user_msg(), None).await {
        Err(e) => e,
        Ok(_) => panic!("expected a gateway error, got a stream"),
    };
    match &err {
        LlmError::GatewayError { title, hint } => {
            assert!(title.contains("413"));
            assert!(hint.contains("请求体过大"));
        }
        other => panic!("expected GatewayError, got {other:?}"),
    }
}
