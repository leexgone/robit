//! `generate_image` tool - generates images from text prompts.
//!
//! Uses the configured `default_image_model` provider (Wanxiang/DashScope or
//! any OpenAI-compatible image API). The model is configured server-side and
//! is not exposed to the LLM. Generated images are downloaded and saved to
//! disk; the tool returns a JSON summary with saved paths and source URLs.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use time::macros::format_description;
use time::OffsetDateTime;

use super::async_runner::AsyncTaskWork;
use super::{resolve_path, Tool, ToolContext, ToolResult};
use crate::error::Result;
use crate::image_gen::{ImageGenClient, ImageGenRequest};
use crate::media::download_media;

/// Maximum number of images that can be generated in one call.
const MAX_N: u32 = 4;

/// Maximum seed value accepted by the DashScope (Wanxiang) API.
const MAX_SEED: u64 = 2_147_483_647;

#[derive(Debug, Deserialize)]
struct GenerateImageArgs {
    prompt: String,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    output_path: Option<String>,
    #[serde(default)]
    n: Option<u32>,
    /// Output resolution as "width*height" (e.g. "1280*1280").
    #[serde(default)]
    size: Option<String>,
    /// Negative prompt: content to exclude from the image.
    #[serde(default)]
    negative_prompt: Option<String>,
    /// Whether to enable smart prompt rewriting (provider default: true).
    #[serde(default)]
    prompt_extend: Option<bool>,
    /// Random seed in [0, 2147483647] for reproducible generation.
    #[serde(default)]
    seed: Option<u64>,
}

/// Build the provider pass-through parameters from the optional tool args.
///
/// Returns `Value::Null` when no optional parameter was given, so the request
/// body stays identical to before these options existed. For the DashScope
/// protocol these keys are merged into `parameters`; for the OpenAI protocol
/// into the top-level request body (unsupported keys are rejected by the
/// provider, which surfaces as an API error the LLM can react to).
fn build_extra_params(args: &GenerateImageArgs) -> Value {
    let mut extra = serde_json::Map::new();
    if let Some(size) = args
        .size
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        extra.insert("size".to_string(), json!(size));
    }
    if let Some(np) = args
        .negative_prompt
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        extra.insert("negative_prompt".to_string(), json!(np));
    }
    if let Some(pe) = args.prompt_extend {
        extra.insert("prompt_extend".to_string(), json!(pe));
    }
    if let Some(seed) = args.seed {
        extra.insert("seed".to_string(), json!(seed));
    }
    if extra.is_empty() {
        Value::Null
    } else {
        Value::Object(extra)
    }
}

pub struct GenerateImageTool {
    client: ImageGenClient,
}

impl GenerateImageTool {
    pub fn new(client: ImageGenClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for GenerateImageTool {
    fn name(&self) -> &str {
        "generate_image"
    }

    fn description(&self) -> &str {
        "Generate images from a text prompt using AI image generation. \
         The model is configured server-side and cannot be changed by the caller. \
         Generated images are saved as PNG files and the paths are returned."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Text description of the image to generate. Supports Chinese and English."
                },
                "filename": {
                    "type": "string",
                    "description": "Base filename (without extension) for saved images. \
                                    If omitted, a timestamp-based name is generated. \
                                    For multiple images, a '-1', '-2' suffix is appended."
                },
                "output_path": {
                    "type": "string",
                    "description": "Directory to save images (relative or absolute). \
                                    Defaults to {working_dir}/images."
                },
                "n": {
                    "type": "integer",
                    "description": "Number of images to generate (1-4). Defaults to 1.",
                    "minimum": 1,
                    "maximum": MAX_N
                },
                "size": {
                    "type": "string",
                    "description": "Output image resolution as 'width*height' (e.g. '1280*1280'). \
                                    Omit to use the provider default. Common ratios (Wanxiang wan2.5+): \
                                    1:1 '1280*1280', 3:4 '1104*1472', 4:3 '1472*1104', \
                                    9:16 '960*1696', 16:9 '1696*960'. \
                                    Constraints depend on the configured model; an invalid size is \
                                    rejected by the provider as an API error."
                },
                "negative_prompt": {
                    "type": "string",
                    "description": "Optional negative prompt: content to avoid in the generated \
                                    image (e.g. '低分辨率，肢体畸形'). Max 500 characters."
                },
                "prompt_extend": {
                    "type": "boolean",
                    "description": "Optional. Enable smart prompt rewriting (provider default: true). \
                                    Set to false if generation fails with IPInfringementSuspect or \
                                    DataInspectionFailed caused by the rewritten prompt."
                },
                "seed": {
                    "type": "integer",
                    "description": "Optional random seed in [0, 2147483647]. Same seed keeps \
                                    results relatively stable across calls.",
                    "minimum": 0,
                    "maximum": MAX_SEED
                }
            },
            "required": ["prompt"]
        })
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn supports_async(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let parsed: GenerateImageArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("Argument parsing failed: {}", e))),
        };

        if parsed.prompt.trim().is_empty() {
            return Ok(ToolResult::error("prompt cannot be empty".to_string()));
        }

        // Validate and clamp n
        let n = parsed.n.unwrap_or(1).clamp(1, MAX_N);

        // Validate seed against the provider's accepted range (schema also
        // declares it, but the LLM may still send an out-of-range value).
        if let Some(seed) = parsed.seed {
            if seed > MAX_SEED {
                return Ok(ToolResult::error(format!(
                    "seed must be in [0, {}], got {}",
                    MAX_SEED, seed
                )));
            }
        }

        let extra_params = build_extra_params(&parsed);

        // Resolve save directory (default: {working_dir}/images)
        let save_dir = match parsed.output_path.as_deref() {
            Some(p) => resolve_path(p, &ctx.working_dir),
            None => ctx.working_dir.join("images"),
        };

        // Determine base filename (default: image_{YYYYMMDD_HHMMSS})
        let base_filename = parsed
            .filename
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(default_filename);

        // The actual generation + download can take 30-60s (or minutes for
        // video), so it runs in a background task. We validate args above
        // (cheap, gives immediate feedback on bad input) and move the heavy
        // work into `work`, returning a pending placeholder.
        let client = self.client.clone();
        let working_dir = ctx.working_dir.clone();
        let prompt = parsed.prompt.clone();

        let work: AsyncTaskWork = Box::pin(async move {
            let req = ImageGenRequest {
                prompt,
                n: Some(n),
                extra_params,
            };

            tracing::info!(
                "[generate_image] requesting {} image(s) (background), extra_params={}",
                n,
                req.extra_params
            );

            let images = match client.generate(&req).await {
                Ok(imgs) => imgs,
                Err(e) => {
                    tracing::error!(
                        "[generate_image] image generation failed: {}. \
                         The error will be reported to the Agent as a task result.",
                        e
                    );
                    let info = e.to_error_info();
                    let err_json = json!({
                        "status": "failed",
                        "error": {
                            "kind": info.kind,
                            "code": info.code,
                            "message": info.message,
                            "retryable": info.retryable,
                        }
                    });
                    return ToolResult::error(
                        serde_json::to_string_pretty(&err_json)
                            .unwrap_or_else(|_| err_json.to_string()),
                    );
                }
            };

            if images.is_empty() {
                let err_json = json!({
                    "status": "failed",
                    "error": "Provider returned no images"
                });
                return ToolResult::error(
                    serde_json::to_string_pretty(&err_json)
                        .unwrap_or_else(|_| err_json.to_string()),
                );
            }

            // Download and save each image. All images are attempted even if
            // some fail, so partial results are preserved.
            let multi = images.len() > 1;
            let mut results: Vec<Value> = Vec::with_capacity(images.len());
            let mut success_count: usize = 0;

            for (i, img) in images.iter().enumerate() {
                let index = i + 1;
                let filename = if multi {
                    format!("{}-{}.png", base_filename, index)
                } else {
                    format!("{}.png", base_filename)
                };

                let saved_path = download_media(&img.url, Some(&filename), &save_dir).await;
                match saved_path {
                    Ok(path) => {
                        success_count += 1;
                        results.push(json!({
                            "index": index,
                            "file": display_path(&path, &working_dir),
                            "size": img.size.clone().unwrap_or_else(|| "unknown".to_string()),
                            "url": img.url,
                        }));
                    }
                    Err(e) => {
                        results.push(json!({
                            "index": index,
                            "file": null,
                            "size": img.size.clone().unwrap_or_else(|| "unknown".to_string()),
                            "url": img.url,
                            "error": format!("Download failed: {}", e),
                        }));
                    }
                }
            }

            let status = if success_count == images.len() {
                "success"
            } else {
                "partial"
            };

            let response = json!({
                "status": status,
                "generated_count": success_count,
                "images": results,
            });

            let content = serde_json::to_string_pretty(&response)
                .unwrap_or_else(|_| response.to_string());

            if success_count == 0 {
                // All downloads failed - report as error
                ToolResult::error(content)
            } else {
                ToolResult::success(content)
            }
        });

        // Submit the background task and return a placeholder. The Agent tracks
        // the task id and reinjects the final result when `work` completes.
        let task_id = ctx.async_runner.submit(
            ctx.tool_call_id.clone(),
            ctx.session_id.clone(),
            self.name().to_string(),
            work,
            ctx.cancel_token.clone(),
        );

        let placeholder = format!(
            "图片生成中(异步任务 task_id={})。预计耗时 30-60 秒,完成后会自动通知结果。\
             你可以继续其他工作,完成后我会收到通知并告知你。",
            task_id
        );
        Ok(ToolResult::pending(placeholder, task_id))
    }
}

/// Generate a timestamp-based default filename: `image_{YYYYMMDD_HHMMSS}`.
fn default_filename() -> String {
    const FMT: &[time::format_description::FormatItem<'_>] =
        format_description!("image_[year][month][day]_[hour][minute][second]");
    OffsetDateTime::now_utc()
        .format(FMT)
        .unwrap_or_else(|_| "image".to_string())
}

/// Render a saved path relative to the working directory when possible,
/// otherwise fall back to the absolute path.
fn display_path(path: &Path, working_dir: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(working_dir) {
        // Use forward slashes for display consistency across platforms.
        rel.to_string_lossy().replace('\\', "/")
    } else {
        path.to_string_lossy().replace('\\', "/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_default_filename_format() {
        let name = default_filename();
        assert!(name.starts_with("image_"), "filename was: {name}");
        // image_ + 8 digits + _ + 6 digits
        assert!(name.len() >= "image_YYYYMMDD_HHMMSS".len(), "filename was: {name}");
    }

    #[test]
    fn test_display_path_relative() {
        let working_dir = PathBuf::from("/home/user/project");
        let saved = PathBuf::from("/home/user/project/images/cat.png");
        assert_eq!(display_path(&saved, &working_dir), "images/cat.png");
    }

    #[test]
    fn test_display_path_outside_working_dir() {
        let working_dir = PathBuf::from("/home/user/project");
        let saved = PathBuf::from("/tmp/images/cat.png");
        assert_eq!(display_path(&saved, &working_dir), "/tmp/images/cat.png");
    }

    fn args(prompt: &str) -> GenerateImageArgs {
        serde_json::from_value(json!({ "prompt": prompt })).unwrap()
    }

    #[test]
    fn test_extra_params_all_absent_is_null() {
        assert_eq!(build_extra_params(&args("a cat")), Value::Null);
    }

    #[test]
    fn test_extra_params_blank_strings_filtered() {
        let mut a = args("a cat");
        a.size = Some("  ".to_string());
        a.negative_prompt = Some("".to_string());
        assert_eq!(build_extra_params(&a), Value::Null);
    }

    #[test]
    fn test_extra_params_all_present() {
        let mut a = args("a cat");
        a.size = Some(" 1696*960 ".to_string());
        a.negative_prompt = Some("低分辨率".to_string());
        a.prompt_extend = Some(false);
        a.seed = Some(42);
        let extra = build_extra_params(&a);
        assert_eq!(extra["size"], json!("1696*960"));
        assert_eq!(extra["negative_prompt"], json!("低分辨率"));
        assert_eq!(extra["prompt_extend"], json!(false));
        assert_eq!(extra["seed"], json!(42));
        assert_eq!(extra.as_object().unwrap().len(), 4);
    }

    #[test]
    fn test_args_deserialize_optional_fields() {
        let a: GenerateImageArgs = serde_json::from_value(json!({
            "prompt": "a cat",
            "size": "1280*1280",
            "seed": 2147483647u64
        }))
        .unwrap();
        assert_eq!(a.size.as_deref(), Some("1280*1280"));
        assert_eq!(a.seed, Some(2_147_483_647));
        assert_eq!(a.negative_prompt, None);
        assert_eq!(a.prompt_extend, None);
    }
}
