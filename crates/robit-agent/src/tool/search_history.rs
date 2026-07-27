//! Search history tool — full-text search of chat messages.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::storage::{search_messages, MessageSearchFilter};
use crate::tool::{Tool, ToolContext, ToolResult};
use crate::error::Result;

#[derive(Debug, Deserialize)]
struct SearchHistoryArgs {
    query: String,
    role: Option<String>,
    since: Option<String>,
    until: Option<String>,
    limit: Option<usize>,
    #[serde(default)]
    all_sessions: bool,
}

pub struct SearchHistoryTool;

impl SearchHistoryTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SearchHistoryTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SearchHistoryTool {
    fn name(&self) -> &str {
        "search_history"
    }

    fn description(&self) -> &str {
        "Search through chat history using full-text search. \
         By default searches only the current session. \
         Useful for finding past messages, references, or context from earlier in the conversation."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query. Supports phrase matching (\"quoted text\"), prefix queries (word*), and boolean operators (AND, OR, NOT)"
                },
                "role": {
                    "type": "string",
                    "description": "Filter by message role: user, assistant, or tool"
                },
                "since": {
                    "type": "string",
                    "description": "Only return messages after this ISO 8601 timestamp"
                },
                "until": {
                    "type": "string",
                    "description": "Only return messages before this ISO 8601 timestamp"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return",
                    "default": 10
                },
                "all_sessions": {
                    "type": "boolean",
                    "description": "If true, search across all sessions. If false, search only the current session",
                    "default": false
                }
            },
            "required": ["query"]
        })
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let parsed: SearchHistoryArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("Argument parsing failed: {}", e))),
        };

        if parsed.query.trim().is_empty() {
            return Ok(ToolResult::error("Search query cannot be empty".to_string()));
        }

        let db_path = match crate::storage::resolve_db_path(&ctx.working_dir, false) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::error(format!("Failed to resolve DB path: {}", e))),
        };

        let conn = match rusqlite::Connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::error(format!("Failed to open DB: {}", e))),
        };

        let session_id = if parsed.all_sessions {
            None
        } else {
            Some(ctx.session_id.as_str())
        };

        // Convert Option<String> to Option<&str> for the filter
        let filter = MessageSearchFilter {
            session_id,
            role: parsed.role.as_deref(),
            since: parsed.since.as_deref(),
            until: parsed.until.as_deref(),
        };

        let limit = parsed.limit.unwrap_or(10).min(50); // Hard cap at 50

        match search_messages(&conn, &parsed.query, &filter, limit) {
            Ok(results) if results.is_empty() => {
                Ok(ToolResult::success(
                    "No messages found matching the search criteria.".to_string()
                ))
            }
            Ok(results) => {
                let mut output = format!(
                    "Found {} message{} matching \"{}\":\n\n",
                    results.len(),
                    if results.len() == 1 { "" } else { "s" },
                    parsed.query
                );

                let current_session = ctx.session_id.as_str();

                for (i, msg) in results.iter().enumerate() {
                    output.push_str(&format!(
                        "{}. [{}] {}",
                        i + 1,
                        msg.role,
                        msg.created_at
                    ));

                    // Show session info when searching across sessions
                    if parsed.all_sessions && msg.session_id != current_session {
                        output.push_str(&format!(
                            " (Session: {})",
                            msg.session_title
                        ));
                    }

                    output.push_str("\n");

                    // Indent snippet for readability
                    for line in msg.content_snippet.lines() {
                        output.push_str(&format!("   {}\n", line));
                    }
                    output.push_str("\n");
                }

                Ok(ToolResult::success(output))
            }
            Err(e) => Ok(ToolResult::error(format!("Search failed: {}", e))),
        }
    }
}
