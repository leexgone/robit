//! `read` tool — reads file contents with line numbers.

use std::path::Path;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use super::{resolve_path, Tool, ToolContext, ToolImage, ToolResult};
use crate::error::Result;
use crate::media;

pub struct ReadTool {
    /// Max output lines before truncation.
    max_output_lines: usize,
    /// Max output bytes before truncation.
    max_output_bytes: usize,
    /// Whether the configured LLM supports image inputs.
    /// Controls whether image files are encoded and whether the description
    /// advertises image support to the LLM.
    supports_images: bool,
    /// Max dimension (longest side, px) for images encoded into the context;
    /// larger images are downscaled and re-encoded as JPEG (0 disables).
    max_image_dimension: u32,
}

#[derive(Debug, Deserialize)]
struct ReadArgs {
    file_path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

impl ReadTool {
    pub fn new(
        max_output_lines: usize,
        max_output_bytes: usize,
        supports_images: bool,
        max_image_dimension: u32,
    ) -> Self {
        Self {
            max_output_lines,
            max_output_bytes,
            supports_images,
            max_image_dimension,
        }
    }

    /// Read an image file. When the model supports images, encode as base64
    /// for the vision model; otherwise return a text description only.
    async fn read_image(&self, path: &Path, ctx: &ToolContext) -> ToolResult {
        let metadata = match tokio::fs::metadata(path).await {
            Ok(m) => m,
            Err(e) => return ToolResult::error(format!("Failed to read image metadata: {}", e)),
        };
        let size = metadata.len();
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let format = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_string();

        let description = format!(
            "Image file: {} ({} bytes, format: {})",
            filename, size, format
        );

        if !ctx.supports_images {
            return ToolResult::success(description);
        }

        // Size limit: 20MB. OpenAI-compatible APIs typically allow up to ~20MB
        // base64-encoded images; 2K PNGs frequently exceed 5MB.
        const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
        if size > MAX_IMAGE_BYTES {
            return ToolResult::error(format!(
                "Image too large: {} bytes (max {} bytes)",
                size, MAX_IMAGE_BYTES
            ));
        }

        match media::encode_file_base64(path, self.max_image_dimension).await {
            Ok(encoded) => {
                // Tell the model what happened to the image so it can judge
                // how much detail is actually available.
                let content = if encoded.compression.kept_original {
                    description
                } else {
                    format!(
                        "{} [已压缩: {}×{} → {}×{} JPEG]",
                        description,
                        encoded.compression.orig_dims.0,
                        encoded.compression.orig_dims.1,
                        encoded.compression.new_dims.0,
                        encoded.compression.new_dims.1
                    )
                };
                ToolResult {
                    content,
                    is_error: false,
                    images: vec![ToolImage {
                        data_url: encoded.data_url,
                        label: filename,
                    }],
                    is_pending: false,
                    pending_task_id: None,
                }
            }
            Err(e) => ToolResult::error(format!("Failed to read image: {}", e)),
        }
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        if self.supports_images {
            "Read file contents. Supports text files (with line numbers and offset/limit) \
             and image files (PNG, JPEG, GIF, WebP - read image content will be understood \
             by the vision model). Large text files can be read in segments using \
             offset/limit. Output includes line numbers."
        } else {
            "Read file contents. Supports text files. Large files can be read in segments \
             using offset/limit. Output includes line numbers."
        }
    }

    fn parameters_schema(&self) -> Value {
        let file_path_desc = if self.supports_images {
            "File path (relative or absolute). Supports text files and image files (PNG, JPEG, GIF, WebP)."
        } else {
            "File path (relative or absolute)"
        };
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": file_path_desc
                },
                "offset": {
                    "type": "integer",
                    "description": "Starting line number (0-based, default 0)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max number of lines to read (default: read all)"
                }
            },
            "required": ["file_path"]
        })
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let parsed: ReadArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("Argument parsing failed: {}", e))),
        };

        // Resolve file path
        let path = resolve_path(&parsed.file_path, &ctx.working_dir);

        // Check if file exists
        if !path.exists() {
            return Ok(ToolResult::error(format!("File not found: {}", path.display())));
        }

        if path.is_dir() {
            return Ok(ToolResult::error(format!(
                "'{}' is a directory, not a file",
                path.display()
            )));
        }

        // Image files: encode as base64 for the vision model (if supported).
        // Text files: fall through to the read_to_string path below.
        let is_image = matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .as_deref(),
            Some("png" | "jpg" | "jpeg" | "gif" | "webp")
        );

        if is_image {
            return Ok(self.read_image(&path, ctx).await);
        }

        // Read file content
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult::error(format!(
                    "Failed to read file '{}': {}",
                    path.display(),
                    e
                )));
            }
        };

        let all_lines: Vec<&str> = content.lines().collect();
        let total_lines = all_lines.len();
        let offset = parsed.offset.unwrap_or(0);
        let limit = parsed.limit.unwrap_or(total_lines);

        // Validate offset
        if offset > total_lines {
            return Ok(ToolResult::error(format!(
                "offset {} is out of range, file has {} lines",
                offset, total_lines
            )));
        }

        let end = (offset + limit).min(total_lines);
        let selected_lines = &all_lines[offset..end];

        // Build output with line numbers
        let mut output = String::new();
        let mut byte_count = 0;

        for (i, line) in selected_lines.iter().enumerate() {
            let line_num = offset + i + 1; // 1-based line numbers
            let formatted = format!("{:>6}\t{}\n", line_num, line);

            // Check byte limit
            if byte_count + formatted.len() > self.max_output_bytes {
                output.push_str(&format!(
                    "\n... (Output truncated, byte limit of {} bytes reached)\n",
                    self.max_output_bytes
                ));
                return Ok(ToolResult::success(output));
            }

            // Check line limit
            if i >= self.max_output_lines {
                output.push_str(&format!(
                    "\n... (Output truncated, {} lines total, showing first {}. Use offset/limit to read more)\n",
                    total_lines, self.max_output_lines
                ));
                return Ok(ToolResult::success(output));
            }

            byte_count += formatted.len();
            output.push_str(&formatted);
        }

        // Add summary if only part of file was shown
        if offset > 0 || end < total_lines {
            output.push_str(&format!(
                "\n(Showing lines {}-{} of {})",
                offset + 1,
                end,
                total_lines
            ));
        }

        Ok(ToolResult::success(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{Tool, ToolContext};
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    /// Minimal ToolContext for a vision-capable model.
    fn image_ctx() -> ToolContext {
        struct DummyFrontend;
        #[async_trait]
        impl crate::frontend::Frontend for DummyFrontend {
            async fn on_event(&self, _: crate::event::AgentEvent) -> Result<()> {
                Ok(())
            }
            async fn request_tool_confirmation(
                &self,
                _: &crate::tool::ToolCallInfo,
            ) -> Result<bool> {
                Ok(true)
            }
        }
        let (done_tx, _done_rx) = mpsc::channel(1);
        ToolContext {
            working_dir: std::path::PathBuf::from("."),
            session_id: "sess".to_string(),
            tool_call_id: "tc".to_string(),
            frontend: Arc::new(DummyFrontend),
            extensions: std::collections::HashMap::new(),
            supports_images: true,
            async_runner: crate::tool::async_runner::AsyncTaskRunner::new(done_tx),
            cancel_token: CancellationToken::new(),
            task_registry: crate::tool::task_registry::TaskRegistry::new(),
        }
    }

    /// Deterministic noise PNG (same generator as media tests).
    async fn write_noise_png(dir: &std::path::Path, w: u32, h: u32) -> std::path::PathBuf {
        let mut img = image::RgbImage::new(w, h);
        let mut seed: u32 = 0x1234_5678;
        for (_, _, p) in img.enumerate_pixels_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *p = image::Rgb([(seed >> 16) as u8, (seed >> 8) as u8, seed as u8]);
        }
        let path = dir.join(format!("img-{}-{}.png", w, h));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        tokio::fs::write(&path, buf.into_inner()).await.unwrap();
        path
    }

    #[tokio::test]
    async fn read_image_compresses_big_png_to_jpeg() {
        let dir = std::env::temp_dir().join(format!("robit-read-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = write_noise_png(&dir, 2048, 2048).await;

        let tool = ReadTool::new(500, 51200, true, 1024);
        let result = tool
            .execute(json!({"file_path": path.to_string_lossy()}), &image_ctx())
            .await
            .unwrap();
        assert!(!result.is_error, "content: {}", result.content);
        assert_eq!(result.images.len(), 1);
        assert!(
            result.images[0].data_url.starts_with("data:image/jpeg;base64,"),
            "2048px PNG should be re-encoded as JPEG, got prefix: {:?}",
            &result.images[0].data_url[..25.min(result.images[0].data_url.len())]
        );
        assert!(
            result.content.contains("1024×1024"),
            "description should mention the downscaled dimensions, got: {}",
            result.content
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn read_image_zero_dimension_disables_compression() {
        let dir = std::env::temp_dir().join(format!("robit-read-0-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = write_noise_png(&dir, 2048, 2048).await;

        let tool = ReadTool::new(500, 51200, true, 0);
        let result = tool
            .execute(json!({"file_path": path.to_string_lossy()}), &image_ctx())
            .await
            .unwrap();
        assert!(!result.is_error, "content: {}", result.content);
        assert_eq!(result.images.len(), 1);
        assert!(
            result.images[0].data_url.starts_with("data:image/png;base64,"),
            "max_image_dimension=0 must keep the original PNG bytes"
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
