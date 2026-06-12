//! `find` tool — searches for files matching a pattern.

use async_trait::async_trait;
use globset::{GlobBuilder, GlobSetBuilder};
use serde::Deserialize;
use serde_json::Value;

use super::{resolve_path, Tool, ToolContext, ToolResult};
use crate::error::Result;

pub struct FindTool {
    /// Max output bytes before truncation.
    max_output_bytes: usize,
}

#[derive(Debug, Deserialize)]
struct FindArgs {
    pattern: String,
    dir_path: Option<String>,
    #[serde(default)]
    file_only: bool,
    #[serde(default)]
    dir_only: bool,
    #[serde(default)]
    ignore_case: bool,
}

impl FindTool {
    pub fn new(max_output_bytes: usize) -> Self {
        Self { max_output_bytes }
    }
}

#[async_trait]
impl Tool for FindTool {
    fn name(&self) -> &str {
        "find"
    }

    fn description(&self) -> &str {
        "搜索匹配模式的文件/目录。支持 glob 模式：* 匹配任意字符，? 匹配单个字符，** 匹配递归目录。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "搜索模式（glob 格式），例如 *.rs、src/**/*.ts、test?.*"
                },
                "dir_path": {
                    "type": "string",
                    "description": "搜索目录（可选，默认为当前目录）"
                },
                "file_only": {
                    "type": "boolean",
                    "description": "仅查找文件，默认 false"
                },
                "dir_only": {
                    "type": "boolean",
                    "description": "仅查找目录，默认 false"
                },
                "ignore_case": {
                    "type": "boolean",
                    "description": "忽略大小写，默认 false"
                }
            },
            "required": ["pattern"]
        })
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let parsed: FindArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("参数解析失败: {}", e))),
        };

        // Resolve directory path
        let search_dir = match parsed.dir_path {
            Some(p) => resolve_path(&p, &ctx.working_dir),
            None => ctx.working_dir.clone(),
        };

        // Check if search directory exists
        if !search_dir.exists() {
            return Ok(ToolResult::error(format!("搜索目录不存在: {}", search_dir.display())));
        }
        if !search_dir.is_dir() {
            return Ok(ToolResult::error(format!("'{}' 不是一个目录", search_dir.display())));
        }

        // Build glob pattern
        let glob = match GlobBuilder::new(&parsed.pattern)
            .case_insensitive(parsed.ignore_case)
            .build() {
            Ok(g) => g,
            Err(e) => {
                return Ok(ToolResult::error(format!("无效的 glob 模式 '{}': {}", parsed.pattern, e)));
            }
        };

        let glob_set = {
            let mut builder = GlobSetBuilder::new();
            builder.add(glob);
            match builder.build() {
                Ok(g) => g,
                Err(e) => {
                    return Ok(ToolResult::error(format!("无效的 glob 模式 '{}': {}", parsed.pattern, e)));
                }
            }
        };

        // Perform walk
        let mut matches = Vec::new();
        let mut walk_dir = vec![search_dir.clone()];

        while let Some(dir) = walk_dir.pop() {
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(e) => e,
                Err(_) => continue,
            };

            loop {
                let entry = match entries.next_entry().await {
                    Ok(Some(e)) => e,
                    Ok(None) => break,
                    Err(_) => continue,
                };
                let path = entry.path();
                let rel_path = match path.strip_prefix(&ctx.working_dir) {
                    Ok(p) => p.to_path_buf(),
                    Err(_) => path.clone(),
                };

                let is_dir = path.is_dir();
                let is_file = path.is_file();

                // Check file/dir filters
                if parsed.file_only && !is_file {
                    continue;
                }
                if parsed.dir_only && !is_dir {
                    continue;
                }

                // Check glob match
                if let Some(file_name) = path.file_name() {
                    if glob_set.is_match(file_name) {
                        matches.push(rel_path.clone());
                    }
                }

                // Continue walking directories recursively
                if is_dir {
                    walk_dir.push(path);
                }
            }
        }

        // Sort matches
        matches.sort();

        // Build output
        let mut output = String::new();
        output.push_str(&format!("搜索: {}\n", parsed.pattern));
        output.push_str(&format!("目录: {}\n", search_dir.display()));
        output.push_str(&format!("{}", "─".repeat(50)));

        if matches.is_empty() {
            output.push_str("\n(未找到匹配项)");
        } else {
            output.push_str(&format!("\n找到 {} 个匹配项:\n", matches.len()));

            let mut byte_count = output.len();
            for (i, m) in matches.iter().enumerate() {
                let entry_str = format!("\n{:>4}. {}", i + 1, m.display());

                if byte_count + entry_str.len() > self.max_output_bytes {
                    output.push_str(&format!(
                        "\n... (输出已截断，共 {} 个匹配项，已达到字节上限 {} bytes)",
                        matches.len(), self.max_output_bytes
                    ));
                    break;
                }

                output.push_str(&entry_str);
                byte_count += entry_str.len();
            }
        }

        Ok(ToolResult::success(output))
    }
}
