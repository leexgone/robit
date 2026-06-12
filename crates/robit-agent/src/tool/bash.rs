//! `bash` tool — executes shell commands.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use encoding_rs::GBK;

use super::{Tool, ToolContext, ToolResult};
use crate::error::Result;

/// Default timeout in milliseconds (120 seconds).
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

pub struct BashTool {
    /// Max output bytes before truncation.
    max_output_bytes: usize,
}

#[derive(Debug, Deserialize)]
struct BashArgs {
    command: String,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    working_dir: Option<String>,
}

impl BashTool {
    pub fn new(max_output_bytes: usize) -> Self {
        Self { max_output_bytes }
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute shell commands. Uses cmd.exe on Windows, sh on Linux/macOS. Avoid cd, use absolute paths."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds, default 120000"
                },
                "working_dir": {
                    "type": "string",
                    "description": "Working directory (optional, defaults to project root)"
                }
            },
            "required": ["command"]
        })
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let parsed: BashArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("Argument parsing failed: {}", e))),
        };

        let work_dir = parsed
            .working_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.working_dir.clone());

        let timeout_ms = parsed.timeout.unwrap_or(DEFAULT_TIMEOUT_MS);

        // Build command based on platform
        let mut cmd = build_shell_command(&parsed.command);
        cmd.current_dir(&work_dir);

        // Execute with timeout
        let result = timeout(Duration::from_millis(timeout_ms), cmd.output()).await;

        match result {
            Ok(Ok(output)) => {
                // Decode output, using GBK on Windows
                let stdout = decode_output(&output.stdout);
                let stderr = decode_output(&output.stderr);
                let exit_code = output.status.code().unwrap_or(-1);

                let mut content = String::new();

                // Append stdout (truncated)
                if !stdout.is_empty() {
                    content.push_str(&truncate_output(&stdout, self.max_output_bytes));
                }

                // Append stderr
                if !stderr.is_empty() {
                    if !content.is_empty() {
                        content.push_str("\n");
                    }
                    content.push_str("[stderr]\n");
                    content.push_str(&truncate_output(&stderr, self.max_output_bytes));
                }

                // Append exit code if non-zero
                if exit_code != 0 {
                    if !content.is_empty() {
                        content.push_str("\n");
                    }
                    content.push_str(&format!("[exit code: {}]", exit_code));
                }

                if content.is_empty() {
                    content = "(Command executed successfully, no output)".to_string();
                }

                Ok(ToolResult {
                    content,
                    is_error: exit_code != 0,
                })
            }
            Ok(Err(e)) => Ok(ToolResult::error(format!("Command execution failed: {}", e))),
            Err(_) => Ok(ToolResult::error(format!(
                "Command timed out ({}ms limit)",
                timeout_ms
            ))),
        }
    }
}

/// Build a shell command appropriate for the current platform.
fn build_shell_command(command: &str) -> Command {
    if cfg!(target_os = "windows") {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", command]);
        cmd
    } else {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", command]);
        cmd
    }
}

/// Decode command output, using GBK on Windows for compatibility.
fn decode_output(bytes: &[u8]) -> String {
    #[cfg(target_os = "windows")]
    {
        // On Windows, try GBK first, fall back to UTF-8
        let (cow, _, has_error) = GBK.decode(bytes);
        if !has_error {
            return cow.to_string();
        }
    }
    // Default to UTF-8 lossy decode on non-Windows or if GBK fails
    String::from_utf8_lossy(bytes).to_string()
}

/// Truncate output to max_bytes, appending a notice if truncated.
fn truncate_output(output: &str, max_bytes: usize) -> String {
    if output.len() <= max_bytes {
        output.to_string()
    } else {
        let truncated: String = output.chars().take(max_bytes).collect();
        format!(
            "{}\n... (Output truncated, {} bytes total, showing first {} bytes)",
            truncated,
            output.len(),
            max_bytes
        )
    }
}
