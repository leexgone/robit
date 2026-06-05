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

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "创建或覆盖文件。自动创建父目录。如果文件已存在则覆盖。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "目标文件路径（相对或绝对路径）"
                },
                "content": {
                    "type": "string",
                    "description": "写入的文件内容"
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
            Err(e) => return Ok(ToolResult::error(format!("参数解析失败: {}", e))),
        };

        // Validate file_path
        if parsed.file_path.trim().is_empty() {
            return Ok(ToolResult::error("文件路径不能为空".to_string()));
        }

        let path = resolve_path(&parsed.file_path, &ctx.working_dir);

        // Check if path is an existing directory
        if path.is_dir() {
            return Ok(ToolResult::error(format!(
                "路径是目录: {}",
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
                        "无法创建目录 '{}': {}",
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
                        "已覆盖文件: {} ({} bytes)",
                        path.display(),
                        content_bytes.len()
                    )
                } else {
                    format!(
                        "已创建文件: {} ({} bytes)",
                        path.display(),
                        content_bytes.len()
                    )
                };
                Ok(ToolResult::success(msg))
            }
            Err(e) => Ok(ToolResult::error(format!(
                "无法写入文件 '{}': {}",
                path.display(),
                e
            ))),
        }
    }
}
