//! `write` tool — creates or overwrites files.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use super::{resolve_path, Tool, ToolContext, ToolResult};
use crate::error::Result;

#[derive(Debug, Deserialize)]
struct WriteArgs {
    file_path: String,
    content: String,
}

pub struct WriteTool;

impl WriteTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WriteTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Create or overwrite a file. Automatically creates parent directories. Overwrites if the file already exists."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Target file path (relative or absolute)"
                },
                "content": {
                    "type": "string",
                    "description": "File content to write"
                }
            },
            "required": ["file_path", "content"]
        })
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let parsed: WriteArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("Argument parsing failed: {}", e))),
        };

        // Validate file_path
        if parsed.file_path.trim().is_empty() {
            return Ok(ToolResult::error("File path cannot be empty".to_string()));
        }

        let path = resolve_path(&parsed.file_path, &ctx.working_dir);

        // Check if path is an existing directory
        if path.is_dir() {
            return Ok(ToolResult::error(format!(
                "Path is a directory: {}",
                path.display()
            )));
        }

        // Check if file exists (for message differentiation)
        let existed = path.exists();

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    return Ok(ToolResult::error(format!(
                        "Failed to create directory '{}': {}",
                        parent.display(),
                        e
                    )));
                }
            }
        }

        // Write file
        let content_bytes = parsed.content.as_bytes();
        match tokio::fs::write(&path, &parsed.content).await {
            Ok(()) => {
                let msg = if existed {
                    format!(
                        "Overwritten file: {} ({} bytes)",
                        path.display(),
                        content_bytes.len()
                    )
                } else {
                    format!(
                        "Created file: {} ({} bytes)",
                        path.display(),
                        content_bytes.len()
                    )
                };
                Ok(ToolResult::success(msg))
            }
            Err(e) => Ok(ToolResult::error(format!(
                "Failed to write file '{}': {}",
                path.display(),
                e
            ))),
        }
    }
}
