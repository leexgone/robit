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

use super::{resolve_path, Tool, ToolContext, ToolResult};
use crate::error::Result;
use crate::image_gen::{ImageGenClient, ImageGenRequest};
use crate::media::download_media;

/// Maximum number of images that can be generated in one call.
const MAX_N: u32 = 4;

#[derive(Debug, Deserialize)]
struct GenerateImageArgs {
    prompt: String,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    output_path: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    n: Option<u32>,
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
                "size": {
                    "type": "string",
                    "description": "Output image size, e.g. '2K', '4K' or '1024x1024'. \
                                    Leave empty for the model default."
                },
                "n": {
                    "type": "integer",
                    "description": "Number of images to generate (1-4). Defaults to 1.",
                    "minimum": 1,
                    "maximum": MAX_N
                }
            },
            "required": ["prompt"]
        })
    }

    fn requires_confirmation(&self) -> bool {
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

        // Build request and call the provider
        let req = ImageGenRequest {
            prompt: parsed.prompt.clone(),
            size: parsed.size.clone(),
            n: Some(n),
            extra_params: Value::Null,
        };

        tracing::info!(
            "[generate_image] requesting {} image(s), size={:?}",
            n,
            parsed.size
        );

        let images = match self.client.generate(&req).await {
            Ok(imgs) => imgs,
            Err(e) => {
                tracing::warn!("[generate_image] image generation failed: {}", e);
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
                return Ok(ToolResult::error(
                    serde_json::to_string_pretty(&err_json).unwrap_or_else(|_| err_json.to_string()),
                ));
            }
        };

        if images.is_empty() {
            let err_json = json!({
                "status": "failed",
                "error": "Provider returned no images"
            });
            return Ok(ToolResult::error(
                serde_json::to_string_pretty(&err_json).unwrap_or_else(|_| err_json.to_string()),
            ));
        }

        // Download and save each image. All images are attempted even if some
        // fail, so partial results are preserved.
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
                        "file": display_path(&path, &ctx.working_dir),
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
            Ok(ToolResult::error(content))
        } else {
            Ok(ToolResult::success(content))
        }
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
}
