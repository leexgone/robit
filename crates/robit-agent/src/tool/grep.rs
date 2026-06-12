//! `grep` tool — searches file contents for a pattern.

use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

use super::{resolve_path, Tool, ToolContext, ToolResult};
use crate::error::Result;

pub struct GrepTool {
    /// Max output lines before truncation.
    max_output_lines: usize,
    /// Max output bytes before truncation.
    max_output_bytes: usize,
}

#[derive(Debug, Deserialize)]
struct GrepArgs {
    pattern: String,
    file_path: Option<String>,
    dir_path: Option<String>,
    #[serde(default)]
    ignore_case: bool,
    #[serde(default)]
    recursive: bool,
}

impl GrepTool {
    pub fn new(max_output_lines: usize, max_output_bytes: usize) -> Self {
        Self {
            max_output_lines,
            max_output_bytes,
        }
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "在文件或目录中搜索文本模式。支持正则表达式。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "搜索模式（正则表达式）"
                },
                "file_path": {
                    "type": "string",
                    "description": "单个文件路径（与 dir_path 二选一）"
                },
                "dir_path": {
                    "type": "string",
                    "description": "目录路径（与 file_path 二选一）"
                },
                "ignore_case": {
                    "type": "boolean",
                    "description": "忽略大小写，默认 false"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "递归搜索子目录（仅在指定 dir_path 时有效），默认 false"
                }
            },
            "required": ["pattern"]
        })
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let parsed: GrepArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("参数解析失败: {}", e))),
        };

        // Validate arguments
        if parsed.file_path.is_none() && parsed.dir_path.is_none() {
            return Ok(ToolResult::error("必须指定 file_path 或 dir_path 之一".to_string()));
        }
        if parsed.file_path.is_some() && parsed.dir_path.is_some() {
            return Ok(ToolResult::error("不能同时指定 file_path 和 dir_path".to_string()));
        }

        // Build regex
        let regex_pattern = if parsed.ignore_case {
            format!("(?i){}", parsed.pattern)
        } else {
            parsed.pattern.clone()
        };

        let regex = match Regex::new(&regex_pattern) {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolResult::error(format!("无效的正则表达式 '{}': {}", parsed.pattern, e)));
            }
        };

        // Collect files to search
        let mut files_to_search = Vec::new();

        if let Some(fp) = &parsed.file_path {
            let path = resolve_path(fp, &ctx.working_dir);
            if !path.exists() {
                return Ok(ToolResult::error(format!("文件不存在: {}", path.display())));
            }
            if path.is_dir() {
                return Ok(ToolResult::error(format!("'{}' 是一个目录，请使用 dir_path 参数", path.display())));
            }
            files_to_search.push(path);
        } else if let Some(dp) = &parsed.dir_path {
            let path = resolve_path(dp, &ctx.working_dir);
            if !path.exists() {
                return Ok(ToolResult::error(format!("目录不存在: {}", path.display())));
            }
            if !path.is_dir() {
                return Ok(ToolResult::error(format!("'{}' 不是一个目录", path.display())));
            }
            collect_files(&path, parsed.recursive, &mut files_to_search).await;
        }

        // Search each file
        let mut output = String::new();
        output.push_str(&format!("搜索: {}\n", parsed.pattern));
        output.push_str(&format!("{}", "─".repeat(50)));

        let mut line_count = 0;
        let mut byte_count = output.len();
        let mut has_matches = false;

        for file_path in &files_to_search {
            // Read file
            let content = match tokio::fs::read_to_string(file_path).await {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Search lines
            let mut file_matches = Vec::new();
            for (line_num, line) in content.lines().enumerate() {
                if regex.is_match(line) {
                    file_matches.push((line_num + 1, line));
                }
            }

            if !file_matches.is_empty() {
                has_matches = true;

                let rel_path = match file_path.strip_prefix(&ctx.working_dir) {
                    Ok(p) => p.to_path_buf(),
                    Err(_) => file_path.clone(),
                };

                let file_header = format!("\n📄 {}\n", rel_path.display());
                if byte_count + file_header.len() > self.max_output_bytes {
                    output.push_str(&format!(
                        "\n... (输出已截断，已达到字节上限 {} bytes)",
                        self.max_output_bytes
                    ));
                    break;
                }
                output.push_str(&file_header);
                byte_count += file_header.len();

                for (line_num, line) in file_matches {
                    if line_count >= self.max_output_lines {
                        output.push_str(&format!(
                            "\n... (输出已截断，已达到行数上限 {} 行)",
                            self.max_output_lines
                        ));
                        return Ok(ToolResult::success(output));
                    }

                    let line_str = format!("{:>6}: {}\n", line_num, line);
                    if byte_count + line_str.len() > self.max_output_bytes {
                        output.push_str(&format!(
                            "\n... (输出已截断，已达到字节上限 {} bytes)",
                            self.max_output_bytes
                        ));
                        break;
                    }
                    output.push_str(&line_str);
                    byte_count += line_str.len();
                    line_count += 1;
                }
            }
        }

        if !has_matches {
            output.push_str("\n(未找到匹配项)");
        }

        Ok(ToolResult::success(output))
    }
}

async fn collect_files(dir: &Path, recursive: bool, files: &mut Vec<std::path::PathBuf>) {
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
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
            if path.is_file() {
                files.push(path);
            } else if path.is_dir() && recursive {
                dirs.push(path);
            }
        }
    }
}
