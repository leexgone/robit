//! `ls` tool — lists directory contents.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use super::{resolve_path, Tool, ToolContext, ToolResult};
use crate::error::Result;

pub struct LsTool;

#[derive(Debug, Deserialize)]
struct LsArgs {
    dir_path: Option<String>,
    #[serde(default)]
    show_hidden: bool,
}

impl LsTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LsTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for LsTool {
    fn name(&self) -> &str {
        "ls"
    }

    fn description(&self) -> &str {
        "列出目录内容。不指定路径则列出当前目录。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "dir_path": {
                    "type": "string",
                    "description": "目录路径（可选，默认为当前目录）"
                },
                "show_hidden": {
                    "type": "boolean",
                    "description": "是否显示隐藏文件（以 . 开头的文件），默认 false"
                }
            },
            "required": []
        })
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let parsed: LsArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("参数解析失败: {}", e))),
        };

        // Resolve directory path
        let dir_path = match parsed.dir_path {
            Some(p) => resolve_path(&p, &ctx.working_dir),
            None => ctx.working_dir.clone(),
        };

        // Check if path exists and is a directory
        if !dir_path.exists() {
            return Ok(ToolResult::error(format!("路径不存在: {}", dir_path.display())));
        }
        if !dir_path.is_dir() {
            return Ok(ToolResult::error(format!("'{}' 不是一个目录", dir_path.display())));
        }

        // Read directory contents
        let mut entries = match tokio::fs::read_dir(&dir_path).await {
            Ok(e) => e,
            Err(e) => {
                return Ok(ToolResult::error(format!("无法读取目录 '{}': {}", dir_path.display(), e)));
            }
        };

        let mut output = String::new();
        let mut dirs = Vec::new();
        let mut files = Vec::new();

        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(e)) => e,
                Ok(None) => break,
                Err(_) => continue,
            };
            let file_name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden files unless show_hidden is true
            if !parsed.show_hidden && file_name.starts_with('.') {
                continue;
            }

            let metadata = match entry.metadata().await {
                Ok(m) => m,
                Err(_) => continue,
            };
            if metadata.is_dir() {
                dirs.push(file_name);
            } else {
                files.push(file_name);
            }
        }

        // Sort alphabetically
        dirs.sort();
        files.sort();

        output.push_str(&format!("目录: {}\n", dir_path.display()));
        output.push_str(&format!("{}", "─".repeat(50)));

        if !dirs.is_empty() {
            output.push_str("\n📁 目录:\n");
            for dir in &dirs {
                output.push_str(&format!("  {}/\n", dir));
            }
        }

        if !files.is_empty() {
            output.push_str("\n📄 文件:\n");
            for file in &files {
                output.push_str(&format!("  {}\n", file));
            }
        }

        if dirs.is_empty() && files.is_empty() {
            output.push_str("\n(目录为空)");
        }

        Ok(ToolResult::success(output))
    }
}
