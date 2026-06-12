//! `read` tool — reads file contents with line numbers.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use super::{resolve_path, Tool, ToolContext, ToolResult};
use crate::error::Result;

pub struct ReadTool {
    /// Max output lines before truncation.
    max_output_lines: usize,
    /// Max output bytes before truncation.
    max_output_bytes: usize,
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
    pub fn new(max_output_lines: usize, max_output_bytes: usize) -> Self {
        Self {
            max_output_lines,
            max_output_bytes,
        }
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read file contents. Supports text files. Large files can be read in segments using offset/limit. Output includes line numbers."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "File path (relative or absolute)"
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
