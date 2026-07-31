//! Image generation client: unified interface for multiple providers.
//!
//! Supports two protocols:
//! - `Openai`: OpenAI-compatible Images API (`POST /images/generations`).
//! - `Dashscope`: DashScope native protocol (Wanxiang), with sync and async
//!   call modes. Async mode submits a task then polls until completion.
//!
//! The model used is resolved from config (`default_image_model`) and is not
//! exposed to callers - the client uses `provider.model_id` internally.

use std::time::Duration;

use robit_ai::config::{
    ImageCallMode, ImageProtocol, ResolvedImageProvider,
};
use serde_json::{json, Value};
use thiserror::Error;
use tokio::time::{sleep, timeout};

/// HTTP request timeout for all API calls (sync generation + async submit/poll).
const HTTP_TIMEOUT_SECS: u64 = 120;

// ============================================================================
// Error type
// ============================================================================

#[derive(Debug, Error)]
pub enum ImageGenError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// API returned a non-success status or an error body.
    #[error("API error: {code} - {message}")]
    Api { code: String, message: String },

    /// Async task did not complete within the polling timeout.
    #[error("Task timed out after {0}s")]
    Timeout(u64),

    /// Async task ended in a non-success terminal state (e.g. FAILED).
    #[error("Task failed: {0}")]
    TaskFailed(String),

    /// Unexpected response shape that could not be parsed.
    #[error("Response parse error: {0}")]
    ParseError(String),
}

/// Structured error info extracted from an [`ImageGenError`], for returning
/// to the LLM in a machine-parseable form so it can decide whether to retry.
#[derive(Debug, Clone)]
pub struct ImageGenErrorInfo {
    /// Error category.
    /// One of: `api_error`, `http_error`, `timeout`, `task_failed`, `parse_error`.
    pub kind: &'static str,
    /// Provider-specific error code (only present for `api_error`).
    pub code: Option<String>,
    /// Human-readable error message.
    pub message: String,
    /// Whether retrying the same request might succeed (e.g. rate limit,
    /// transient network error). `false` for permission/parameter errors.
    pub retryable: bool,
}

impl ImageGenError {
    /// Extract structured error info for the LLM.
    pub fn to_error_info(&self) -> ImageGenErrorInfo {
        match self {
            ImageGenError::Api { code, message } => ImageGenErrorInfo {
                kind: "api_error",
                code: Some(code.clone()),
                message: message.clone(),
                retryable: is_retryable_api_error(code, message),
            },
            ImageGenError::Http(e) => ImageGenErrorInfo {
                kind: "http_error",
                code: None,
                message: e.to_string(),
                retryable: true,
            },
            ImageGenError::Timeout(secs) => ImageGenErrorInfo {
                kind: "timeout",
                code: None,
                message: format!("Task timed out after {}s", secs),
                retryable: true,
            },
            ImageGenError::TaskFailed(status) => ImageGenErrorInfo {
                kind: "task_failed",
                code: None,
                message: format!("Task ended in non-success state: {}", status),
                retryable: true,
            },
            ImageGenError::ParseError(msg) => ImageGenErrorInfo {
                kind: "parse_error",
                code: None,
                message: msg.clone(),
                retryable: false,
            },
        }
    }
}

/// Heuristic: whether an API error is likely transient and worth retrying.
///
/// Returns `true` for rate-limit / busy / server-error conditions. Returns
/// `false` for permission, authentication, and parameter errors (retrying
/// the same request would just fail the same way).
fn is_retryable_api_error(code: &str, message: &str) -> bool {
    let combined = format!("{} {}", code, message).to_lowercase();
    combined.contains("throttl")
        || combined.contains("rate limit")
        || combined.contains("ratelimit")
        || combined.contains("busy")
        || combined.contains("please retry")
        || combined.contains("try again")
        || combined.contains("service unavailable")
        || combined.contains("internal error")
        || combined.contains("timeout")
}

// ============================================================================
// Request / response types
// ============================================================================

/// Parameters for an image generation request.
pub struct ImageGenRequest {
    pub prompt: String,
    pub size: Option<String>,
    pub n: Option<u32>,
    /// Extra parameters passed through to the provider (e.g. `watermark`).
    pub extra_params: Value,
}

/// A single generated image, as returned by the provider API.
pub struct GeneratedImage {
    /// Original image URL returned by the API (valid for ~24h).
    pub url: String,
    /// Image resolution string from the API response (e.g. "2048*2048").
    pub size: Option<String>,
}

// ============================================================================
// Client
// ============================================================================

pub struct ImageGenClient {
    provider: ResolvedImageProvider,
    http: reqwest::Client,
}

impl ImageGenClient {
    pub fn new(provider: ResolvedImageProvider) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build()
            .expect("reqwest client should build");
        Self { provider, http }
    }

    /// Generate images from a text prompt.
    ///
    /// Returns one `GeneratedImage` per image the API actually produced.
    /// The number returned may be less than the requested `n` (e.g. due to
    /// content filtering); it is always taken from the API response.
    pub async fn generate(&self, req: &ImageGenRequest) -> Result<Vec<GeneratedImage>, ImageGenError> {
        tracing::info!(
            "[image_gen] generate: protocol={:?}, mode={:?}, provider={}, model={}, base_url={}, api_key={}",
            self.provider.protocol,
            self.provider.mode,
            self.provider.provider_name,
            self.provider.model_id,
            self.provider.base_url,
            mask_key(&self.provider.api_key),
        );
        tracing::debug!(
            "[image_gen] request: prompt={:?}, size={:?}, n={:?}, extra_params={}",
            req.prompt,
            req.size,
            req.n,
            req.extra_params,
        );
        match self.provider.protocol {
            ImageProtocol::Openai => self.generate_openai(req).await,
            ImageProtocol::Dashscope => match self.provider.mode {
                ImageCallMode::Sync => self.generate_dashscope_sync(req).await,
                ImageCallMode::Async => self.generate_dashscope_async(req).await,
            },
        }
    }

    // ----------------------------------------------------------------------
    // OpenAI-compatible Images API
    // ----------------------------------------------------------------------

    async fn generate_openai(&self, req: &ImageGenRequest) -> Result<Vec<GeneratedImage>, ImageGenError> {
        let url = format!("{}/images/generations", self.provider.base_url.trim_end_matches('/'));

        let mut body = json!({
            "model": self.provider.model_id,
            "prompt": req.prompt,
            "response_format": "url",
        });
        if let Some(n) = req.n {
            body["n"] = json!(n);
        }
        if let Some(ref size) = req.size {
            body["size"] = json!(size);
        }
        // Merge any extra params (caller-provided overrides)
        if let Value::Object(ref extra) = req.extra_params {
            if let Value::Object(body_map) = &mut body {
                for (k, v) in extra {
                    body_map.insert(k.clone(), v.clone());
                }
            }
        }

        tracing::debug!("[image_gen] openai POST {} | auth: Bearer {} | body: {}", url, mask_key(&self.provider.api_key), body);

        let resp = self.http.post(&url).bearer_auth(&self.provider.api_key).json(&body).send().await?;
        let status = resp.status();
        let text = resp.text().await?;

        tracing::debug!("[image_gen] openai response: status={} | body: {}", status, truncate_str(&text, 2000));

        let json: Value = serde_json::from_str(&text)
            .map_err(|e| ImageGenError::ParseError(format!("openai response: {e} (body: {text})")))?;

        if !status.is_success() {
            tracing::warn!(
                "[image_gen] openai request failed: status={}, url={}, body={}",
                status, url, truncate_str(&text, 2000)
            );
            return Err(openai_error(&json).unwrap_or(ImageGenError::Api {
                code: status.as_u16().to_string(),
                message: text,
            }));
        }

        let data = json.get("data").and_then(|d| d.as_array()).ok_or_else(|| {
            ImageGenError::ParseError(format!("openai response missing 'data' array (body: {text})"))
        })?;

        let images = data
            .iter()
            .filter_map(|item| {
                item.get("url")
                    .and_then(|u| u.as_str())
                    .map(|u| GeneratedImage { url: u.to_string(), size: req.size.clone() })
            })
            .collect::<Vec<_>>();

        Ok(images)
    }

    // ----------------------------------------------------------------------
    // DashScope (Wanxiang) - synchronous call
    // ----------------------------------------------------------------------

    async fn generate_dashscope_sync(
        &self,
        req: &ImageGenRequest,
    ) -> Result<Vec<GeneratedImage>, ImageGenError> {
        let url = format!(
            "{}/services/aigc/multimodal-generation/generation",
            self.provider.base_url.trim_end_matches('/')
        );
        let body = self.build_dashscope_body(req);
        let json = self.dashscope_post(&url, &body, false).await?;
        self.parse_dashscope_result(&json)
    }

    // ----------------------------------------------------------------------
    // DashScope (Wanxiang) - asynchronous call (submit + poll)
    // ----------------------------------------------------------------------

    async fn generate_dashscope_async(
        &self,
        req: &ImageGenRequest,
    ) -> Result<Vec<GeneratedImage>, ImageGenError> {
        let submit_url = format!(
            "{}/services/aigc/image-generation/generation",
            self.provider.base_url.trim_end_matches('/')
        );
        let body = self.build_dashscope_body(req);

        // Some providers (e.g. Token Plan) reject async calls with an
        // "AccessDenied: does not support asynchronous calls" error. In that
        // case, transparently fall back to synchronous mode so callers don't
        // need to know whether their provider supports async.
        let submit_resp = match self.dashscope_post(&submit_url, &body, true).await {
            Ok(resp) => resp,
            Err(ImageGenError::Api { ref code, ref message })
                if message.to_lowercase().contains("asynchronous") =>
            {
                tracing::warn!(
                    "[image_gen] provider does not support async calls ({}: {}), falling back to sync mode",
                    code, message
                );
                return self.generate_dashscope_sync(req).await;
            }
            Err(e) => return Err(e),
        };

        let task_id = submit_resp
            .pointer("/output/task_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ImageGenError::ParseError(format!(
                    "dashscope async response missing task_id (body: {submit_resp})"
                ))
            })?
            .to_string();

        tracing::info!("[image_gen] async task submitted: {}", task_id);

        let poll_url = format!(
            "{}/tasks/{}",
            self.provider.base_url.trim_end_matches('/'),
            task_id
        );

        let poll_timeout = Duration::from_secs(self.provider.poll_timeout_secs);
        let result = timeout(poll_timeout, self.poll_task(&poll_url)).await;

        match result {
            Ok(Ok(json)) => self.parse_dashscope_result(&json),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(ImageGenError::Timeout(self.provider.poll_timeout_secs)),
        }
    }

    /// Poll the task endpoint until a terminal state is reached.
    async fn poll_task(&self, poll_url: &str) -> Result<Value, ImageGenError> {
        let interval = Duration::from_secs(self.provider.poll_interval_secs);
        loop {
            sleep(interval).await;
            tracing::debug!(
                "[image_gen] dashscope poll GET {} | auth: Bearer {}",
                poll_url, mask_key(&self.provider.api_key)
            );
            let resp = self
                .http
                .get(poll_url)
                .bearer_auth(&self.provider.api_key)
                .send()
                .await?;
            let status = resp.status();
            let text = resp.text().await?;

            tracing::debug!(
                "[image_gen] dashscope poll response: status={} | body: {}",
                status, truncate_str(&text, 2000)
            );

            let json: Value = serde_json::from_str(&text).map_err(|e| {
                ImageGenError::ParseError(format!("dashscope poll response: {e} (body: {text})"))
            })?;

            if !status.is_success() {
                tracing::warn!(
                    "[image_gen] dashscope poll failed: status={}, url={}, body={}",
                    status, poll_url, truncate_str(&text, 2000)
                );
                return Err(dashscope_error(&json).unwrap_or(ImageGenError::Api {
                    code: status.as_u16().to_string(),
                    message: text,
                }));
            }

            let task_status = json.pointer("/output/task_status").and_then(|v| v.as_str());
            match task_status {
                Some("SUCCEEDED") => {
                    tracing::info!("[image_gen] async task succeeded");
                    return Ok(json);
                }
                Some("FAILED") | Some("CANCELED") | Some("UNKNOWN") => {
                    return Err(ImageGenError::TaskFailed(
                        task_status.unwrap_or("UNKNOWN").to_string(),
                    ));
                }
                // PENDING / RUNNING -> keep polling
                _ => {
                    tracing::debug!("[image_gen] task status: {:?}", task_status);
                }
            }
        }
    }

    // ----------------------------------------------------------------------
    // DashScope helpers
    // ----------------------------------------------------------------------

    /// Build the Wanxiang request body from the unified request.
    fn build_dashscope_body(&self, req: &ImageGenRequest) -> Value {
        let mut parameters = json!({});
        if let Some(n) = req.n {
            parameters["n"] = json!(n);
        }
        if let Some(ref size) = req.size {
            parameters["size"] = json!(size);
        }
        // Merge extra params into parameters (e.g. watermark, thinking_mode)
        if let Value::Object(ref extra) = req.extra_params {
            if let Value::Object(p) = &mut parameters {
                for (k, v) in extra {
                    p.insert(k.clone(), v.clone());
                }
            }
        }

        json!({
            "model": self.provider.model_id,
            "input": {
                "messages": [
                    {
                        "role": "user",
                        "content": [ { "text": req.prompt } ]
                    }
                ]
            },
            "parameters": parameters,
        })
    }

    /// POST a DashScope request and return the parsed JSON, checking for errors.
    /// When `async_mode` is true, the `X-DashScope-Async: enable` header is set.
    async fn dashscope_post(
        &self,
        url: &str,
        body: &Value,
        async_mode: bool,
    ) -> Result<Value, ImageGenError> {
        tracing::debug!(
            "[image_gen] dashscope POST {} | async={} | auth: Bearer {} | body: {}",
            url, async_mode, mask_key(&self.provider.api_key), body
        );

        let mut req_builder = self
            .http
            .post(url)
            .bearer_auth(&self.provider.api_key)
            .header("Content-Type", "application/json");
        if async_mode {
            req_builder = req_builder.header("X-DashScope-Async", "enable");
        }
        let resp = req_builder.json(body).send().await?;
        let status = resp.status();
        let text = resp.text().await?;

        tracing::debug!(
            "[image_gen] dashscope response: status={} | url={} | body: {}",
            status, url, truncate_str(&text, 2000)
        );

        let json: Value = serde_json::from_str(&text)
            .map_err(|e| ImageGenError::ParseError(format!("dashscope response: {e} (body: {text})")))?;

        if !status.is_success() {
            tracing::warn!(
                "[image_gen] dashscope request failed: status={}, url={}, body={}",
                status, url, truncate_str(&text, 2000)
            );
            return Err(dashscope_error(&json).unwrap_or(ImageGenError::Api {
                code: status.as_u16().to_string(),
                message: text,
            }));
        }
        // DashScope may return 200 with an error code in the body
        if let Some(err) = dashscope_error(&json) {
            tracing::warn!(
                "[image_gen] dashscope API error in 200 response: url={}, body={}",
                url, truncate_str(&text, 2000)
            );
            return Err(err);
        }
        Ok(json)
    }

    /// Extract generated images from a DashScope success response (sync result
    /// or the final polled task result).
    fn parse_dashscope_result(&self, json: &Value) -> Result<Vec<GeneratedImage>, ImageGenError> {
        let size = json
            .pointer("/usage/size")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let choices = json.pointer("/output/choices").and_then(|c| c.as_array()).ok_or_else(|| {
            ImageGenError::ParseError(format!("dashscope response missing output.choices (body: {json})"))
        })?;

        let mut images = Vec::new();
        for choice in choices {
            let content = choice
                .pointer("/message/content")
                .and_then(|c| c.as_array())
                .ok_or_else(|| {
                    ImageGenError::ParseError(format!(
                        "dashscope choice missing message.content (body: {json})"
                    ))
                })?;
            for item in content {
                if let Some(url) = item.get("image").and_then(|u| u.as_str()) {
                    images.push(GeneratedImage {
                        url: url.to_string(),
                        size: size.clone(),
                    });
                }
            }
        }

        if images.is_empty() {
            return Err(ImageGenError::ParseError(format!(
                "dashscope response contained no images (body: {json})"
            )));
        }
        Ok(images)
    }
}

// ============================================================================
// Error extraction helpers
// ============================================================================

/// Extract a DashScope error from the response body, if present.
/// DashScope errors look like `{ "code": "...", "message": "..." }`.
fn dashscope_error(json: &Value) -> Option<ImageGenError> {
    let code = json.get("code").and_then(|v| v.as_str())?;
    // An empty code string means success (DashScope sometimes returns "" on success)
    if code.is_empty() {
        return None;
    }
    let message = json.get("message").and_then(|v| v.as_str()).unwrap_or("");
    Some(ImageGenError::Api {
        code: code.to_string(),
        message: message.to_string(),
    })
}

/// Extract an OpenAI-style error from the response body, if present.
fn openai_error(json: &Value) -> Option<ImageGenError> {
    let err = json.get("error")?;
    let message = err.get("message").and_then(|v| v.as_str()).unwrap_or("");
    let code = err.get("code").and_then(|v| v.as_str()).unwrap_or("error");
    Some(ImageGenError::Api {
        code: code.to_string(),
        message: message.to_string(),
    })
}

// ============================================================================
// Logging helpers
// ============================================================================

/// Mask an API key for logging: show only the first 8 and last 4 characters.
/// Returns "<empty>" / "<unset>" for edge cases so the log is unambiguous.
fn mask_key(key: &str) -> String {
    if key.is_empty() {
        return "<empty>".to_string();
    }
    let len = key.len();
    if len <= 12 {
        return format!("{}***", &key[..len.min(4)]);
    }
    format!("{}...{}", &key[..8], &key[len - 4..])
}

/// Truncate a string to `max` characters, appending "..." if truncated.
/// Keeps log output bounded for large response bodies.
fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}... (truncated, {} bytes total)", &s[..max], s.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_key_normal() {
        let masked = mask_key("sk-sp-abcdef1234567890");
        assert!(masked.starts_with("sk-sp-ab"));
        assert!(masked.ends_with("7890"));
        assert!(!masked.contains("1234567"));
    }

    #[test]
    fn test_mask_key_short() {
        // Keys <= 12 chars show only first 4 + ***
        let masked = mask_key("sk-sp-abc");
        assert_eq!(masked, "sk-s***");
    }

    #[test]
    fn test_mask_key_empty() {
        assert_eq!(mask_key(""), "<empty>");
    }

    #[test]
    fn test_truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_str_long() {
        let result = truncate_str("abcdefghijklmnopqrstuvwxyz", 10);
        assert!(result.starts_with("abcdefghij"));
        assert!(result.contains("truncated"));
    }

    #[test]
    fn test_error_info_api_not_retryable() {
        let e = ImageGenError::Api {
            code: "AccessDenied".to_string(),
            message: "current user api does not support asynchronous calls".to_string(),
        };
        let info = e.to_error_info();
        assert_eq!(info.kind, "api_error");
        assert_eq!(info.code.as_deref(), Some("AccessDenied"));
        assert!(!info.retryable, "permission errors should not be retryable");
    }

    #[test]
    fn test_error_info_api_retryable() {
        let e = ImageGenError::Api {
            code: "Throttling".to_string(),
            message: "Rate limit exceeded, please retry later".to_string(),
        };
        let info = e.to_error_info();
        assert!(info.retryable, "rate limit errors should be retryable");
    }

    #[test]
    fn test_error_info_timeout_retryable() {
        let info = ImageGenError::Timeout(300).to_error_info();
        assert_eq!(info.kind, "timeout");
        assert!(info.retryable);
        assert!(info.message.contains("300"));
    }

    #[test]
    fn test_error_info_parse_error_not_retryable() {
        let info = ImageGenError::ParseError("bad json".to_string()).to_error_info();
        assert_eq!(info.kind, "parse_error");
        assert!(!info.retryable);
    }

    #[test]
    fn test_error_info_task_failed_retryable() {
        let info = ImageGenError::TaskFailed("FAILED".to_string()).to_error_info();
        assert_eq!(info.kind, "task_failed");
        assert!(info.retryable);
    }
}
