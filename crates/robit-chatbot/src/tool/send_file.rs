//! `send_file` tool — sends a local file or image to the chat user.
//!
//! Only works on chatbot platforms (QQ, Feishu). The tool retrieves the
//! `PlatformExt` capability from `ToolContext.extensions` via the
//! `"chatbot.platform_ext"` key.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use robit_agent::error::Result;
use robit_agent::tool::{resolve_path, Tool, ToolContext, ToolResult};

use crate::extensions::keys;
use crate::extensions::PlatformExtWrapper;

/// Tool that sends a local file/image to the chat user.
pub struct SendFileTool;

#[derive(Debug, Deserialize)]
struct SendFileArgs {
    /// Path to the file (absolute or relative to working directory).
    file_path: String,
    /// Optional media type override: "image" or "file".
    /// When omitted, the type is inferred from the file extension.
    #[serde(default)]
    media_type: Option<String>,
}

#[async_trait]
impl Tool for SendFileTool {
    fn name(&self) -> &str {
        "send_file"
    }

    fn description(&self) -> &str {
        "Send a local image or file to the chat user. Use this tool when the user asks you \
         to send them a file. Provide the full path to the file (absolute or relative to \
         the working directory). Supported image formats: jpg, jpeg, png, gif, bmp, webp. \
         Other file types (pdf, txt, zip, etc.) are also supported."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to send (absolute or relative to working directory)"
                },
                "media_type": {
                    "type": "string",
                    "description": "Optional: 'image' or 'file'. When omitted, inferred from extension.",
                    "enum": ["image", "file"]
                }
            },
            "required": ["file_path"]
        })
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let parsed: SendFileArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("Invalid arguments: {}", e))),
        };

        // Resolve path
        let file_path = resolve_path(&parsed.file_path, &ctx.working_dir);
        let file_path_str = match file_path.to_str() {
            Some(p) => p,
            None => return Ok(ToolResult::error("Invalid file path encoding".to_string())),
        };

        // Validate file exists
        if !file_path.exists() {
            return Ok(ToolResult::error(format!("File not found: {}", file_path_str)));
        }
        if !file_path.is_file() {
            return Ok(ToolResult::error(format!("Not a file: {}", file_path_str)));
        }

        // Determine media type
        let media_type = parsed
            .media_type
            .unwrap_or_else(|| classify_media_type(file_path_str).to_string());

        // Get platform extension
        let ext = match ctx
            .extensions
            .get(keys::PLATFORM_EXT)
            .and_then(|e| e.downcast_ref::<PlatformExtWrapper>())
        {
            Some(wrapper) => &wrapper.0,
            None => {
                return Ok(ToolResult::error(
                    "send_file is only available on chat platforms (QQ, Feishu)".to_string(),
                ));
            }
        };

        // Upload file
        let upload_result = match ext.upload_file(file_path_str, &media_type).await {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::error(format!("Failed to upload file: {}", e))),
        };

        // Send media message
        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file");

        match ext
            .send_media_message(&upload_result.url, file_name, &media_type)
            .await
        {
            Ok(_) => Ok(ToolResult::success(format!("File sent: {}", file_name))),
            Err(e) => Ok(ToolResult::error(format!("Failed to send file: {}", e))),
        }
    }
}

/// Classify a file path as "image" or "file" based on its extension.
fn classify_media_type(path: &str) -> &str {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "svg" | "ico" | "tiff" | "tif" => {
            "image"
        }
        _ => "file",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_image() {
        assert_eq!(classify_media_type("/path/to/photo.jpg"), "image");
        assert_eq!(classify_media_type("C:\\img.PNG"), "image");
        assert_eq!(classify_media_type("/tmp/anim.gif"), "image");
    }

    #[test]
    fn test_classify_file() {
        assert_eq!(classify_media_type("/path/to/doc.pdf"), "file");
        assert_eq!(classify_media_type("C:\\data.zip"), "file");
        assert_eq!(classify_media_type("/tmp/notes.txt"), "file");
    }

    #[test]
    fn test_send_file_schema() {
        let tool = SendFileTool;
        assert_eq!(tool.name(), "send_file");
        let schema = tool.parameters_schema();
        assert!(schema["required"][0].as_str() == Some("file_path"));
    }
}
