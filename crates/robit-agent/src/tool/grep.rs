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
        "Search for a text pattern in files or directories. Supports regex."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Search pattern (regex)"
                },
                "file_path": {
                    "type": "string",
                    "description": "Single file path (use either this or dir_path)"
                },
                "dir_path": {
                    "type": "string",
                    "description": "Directory path (use either this or file_path)"
                },
                "ignore_case": {
                    "type": "boolean",
                    "description": "Case-insensitive search, default false"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Recursively search subdirectories (only effective with dir_path), default false"
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
            Err(e) => return Ok(ToolResult::error(format!("Argument parsing failed: {}", e))),
        };

        // Validate arguments
        if parsed.file_path.is_none() && parsed.dir_path.is_none() {
            return Ok(ToolResult::error("Either file_path or dir_path must be specified".to_string()));
        }
        if parsed.file_path.is_some() && parsed.dir_path.is_some() {
            return Ok(ToolResult::error("Cannot specify both file_path and dir_path simultaneously".to_string()));
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
                return Ok(ToolResult::error(format!("Invalid regex '{}': {}", parsed.pattern, e)));
            }
        };

        // Collect files to search
        let mut files_to_search = Vec::new();

        if let Some(fp) = &parsed.file_path {
            let path = resolve_path(fp, &ctx.working_dir);
            if !path.exists() {
                return Ok(ToolResult::error(format!("File not found: {}", path.display())));
            }
            if path.is_dir() {
                return Ok(ToolResult::error(format!("'{}' is a directory, use dir_path parameter instead", path.display())));
            }
            files_to_search.push(path);
        } else if let Some(dp) = &parsed.dir_path {
            let path = resolve_path(dp, &ctx.working_dir);
            if !path.exists() {
                return Ok(ToolResult::error(format!("Directory not found: {}", path.display())));
            }
            if !path.is_dir() {
                return Ok(ToolResult::error(format!("'{}' is not a directory", path.display())));
            }
            collect_files(&path, parsed.recursive, &mut files_to_search).await;
        }

        // Search each file
        let mut output = String::new();
        output.push_str(&format!("Search: {}\n", parsed.pattern));
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
                        "\n... (Output truncated, byte limit of {} bytes reached)",
                        self.max_output_bytes
                    ));
                    break;
                }
                output.push_str(&file_header);
                byte_count += file_header.len();

                for (line_num, line) in file_matches {
                    if line_count >= self.max_output_lines {
                        output.push_str(&format!(
                            "\n... (Output truncated, line limit of {} lines reached)",
                            self.max_output_lines
                        ));
                        return Ok(ToolResult::success(output));
                    }

                    let line_str = format!("{:>6}: {}\n", line_num, line);
                    if byte_count + line_str.len() > self.max_output_bytes {
                        output.push_str(&format!(
                            "\n... (Output truncated, byte limit of {} bytes reached)",
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
            output.push_str("\n(No matches found)");
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
