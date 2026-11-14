//! Memory tools for long-term memory.
//!
//! Provides tools for the agent to remember, retrieve, and forget information.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::storage::{
    deactivate_memory, delete_memory_permanently, insert_memory, list_memories, recall_memories,
    update_memory, Memory, MemoryFilter, MemoryType,
};
use crate::tool::{Tool, ToolContext, ToolResult};
use crate::error::Result;

// ============================================================================
// Memorize tool - store a memory
// ============================================================================

#[derive(Debug, Deserialize)]
struct MemorizeArgs {
    title: String,
    content: String,
    #[serde(default = "default_memory_type")]
    memory_type: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    update_if_exists: bool,
}

fn default_memory_type() -> String {
    "note".to_string()
}

pub struct MemorizeTool;

impl MemorizeTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MemorizeTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for MemorizeTool {
    fn name(&self) -> &str {
        "memorize"
    }

    fn description(&self) -> &str {
        "Store important information in long-term memory. Use for user preferences, key facts, project notes, etc."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short, descriptive title for the memory (for easy retrieval)"
                },
                "content": {
                    "type": "string",
                    "description": "Full content of the memory"
                },
                "memory_type": {
                    "type": "string",
                    "description": "Type of memory: fact, preference, note, task, or custom",
                    "default": "note"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Tags for categorization and filtering",
                    "default": []
                },
                "update_if_exists": {
                    "type": "boolean",
                    "description": "If true, update existing memory with the same title",
                    "default": false
                }
            },
            "required": ["title", "content"]
        })
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let parsed: MemorizeArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("Argument parsing failed: {}", e))),
        };

        if parsed.title.trim().is_empty() {
            return Ok(ToolResult::error("Title cannot be empty".to_string()));
        }
        if parsed.content.trim().is_empty() {
            return Ok(ToolResult::error("Content cannot be empty".to_string()));
        }

        // Follow the host's storage scope: `memory_dir` is resolved from
        // `global_storage` at Agent construction (same root as the session
        // DB the manager opens), so memory tools and sessions share one DB.
        let db_path = ctx.memory_dir.join("robit.db");

        let conn = match rusqlite::Connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::error(format!("Failed to open DB: {}", e))),
        };

        let memory_type = MemoryType::from_str(&parsed.memory_type);
        let mut memory = Memory::new(
            parsed.title.clone(),
            parsed.content.clone(),
            memory_type,
            parsed.tags.clone(),
        );

        if ctx.session_id.to_string() != "" {
            memory = memory.with_session_id(ctx.session_id.to_string());
        }

        if parsed.update_if_exists {
            // Look for existing memory with the same title
            let filter = MemoryFilter {
                session_id: Some(ctx.session_id.to_string()),
                only_active: true,
                ..Default::default()
            };
            if let Ok(existing) = list_memories(&conn, &filter, Some(100)) {
                if let Some(mut existing) = existing.into_iter().find(|m| m.title == parsed.title) {
                    existing.content = parsed.content;
                    existing.memory_type = MemoryType::from_str(&parsed.memory_type);
                    existing.tags = parsed.tags;
                    if let Err(e) = update_memory(&conn, &existing) {
                        return Ok(ToolResult::error(format!("Failed to update memory: {}", e)));
                    }
                    return Ok(ToolResult::success(format!(
                        "Updated memory: {} (ID: {})",
                        existing.title, existing.id
                    )));
                }
            }
        }

        // Insert as new memory
        if let Err(e) = insert_memory(&conn, &memory) {
            return Ok(ToolResult::error(format!("Failed to store memory: {}", e)));
        }

        Ok(ToolResult::success(format!(
            "Stored memory: {} (ID: {})",
            memory.title, memory.id
        )))
    }
}

// ============================================================================
// Recall tool - retrieve memories
// ============================================================================

#[derive(Debug, Deserialize)]
struct RecallArgs {
    query: Option<String>,
    memory_type: Option<String>,
    tags: Option<Vec<String>>,
    limit: Option<usize>,
    since: Option<String>,
    session_id: Option<String>,
    chat_id: Option<String>,
}

pub struct RecallTool;

impl RecallTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RecallTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for RecallTool {
    fn name(&self) -> &str {
        "recall"
    }

    fn description(&self) -> &str {
        "Retrieve relevant memories from long-term memory. Use when you need context or past information."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Keywords to search in title, content, and tags"
                },
                "memory_type": {
                    "type": "string",
                    "description": "Filter by memory type: fact, preference, note, task"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Filter by tags (any match)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of memories to return",
                    "default": 10
                },
                "since": {
                    "type": "string",
                    "description": "Only return memories created after this ISO 8601 timestamp"
                },
                "session_id": {
                    "type": "string",
                    "description": "Filter by session ID"
                },
                "chat_id": {
                    "type": "string",
                    "description": "Filter by chat ID (Bot platforms)"
                }
            }
        })
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let parsed: RecallArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("Argument parsing failed: {}", e))),
        };

        // Follow the host's storage scope: `memory_dir` is resolved from
        // `global_storage` at Agent construction (same root as the session
        // DB the manager opens), so memory tools and sessions share one DB.
        let db_path = ctx.memory_dir.join("robit.db");

        let conn = match rusqlite::Connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::error(format!("Failed to open DB: {}", e))),
        };

        let filter = MemoryFilter {
            memory_type: parsed.memory_type.as_ref().map(|s| MemoryType::from_str(s)),
            tags: parsed.tags,
            session_id: parsed.session_id.or(Some(ctx.session_id.to_string())),
            chat_id: parsed.chat_id,
            since: parsed.since,
            only_active: true,
        };

        let limit = parsed.limit.unwrap_or(10);

        let memories = if let Some(query) = &parsed.query {
            if query.trim().is_empty() {
                list_memories(&conn, &filter, Some(limit))
            } else {
                recall_memories(&conn, query, &filter, limit)
            }
        } else {
            list_memories(&conn, &filter, Some(limit))
        };

        match memories {
            Ok(memories) if memories.is_empty() => Ok(ToolResult::success(
                "No memories found matching the criteria.".to_string(),
            )),
            Ok(memories) => {
                let mut result = format!("Found {} memories:\n\n", memories.len());
                for (i, memory) in memories.iter().enumerate() {
                    result.push_str(&format!(
                        "{}. [{}] {}\n   {}\n   Tags: {}\n   Created: {}\n\n",
                        i + 1,
                        memory.memory_type.as_str(),
                        memory.title,
                        memory.content,
                        if memory.tags.is_empty() {
                            "(none)".to_string()
                        } else {
                            memory.tags.join(", ")
                        },
                        memory.created_at
                    ));
                }
                Ok(ToolResult::success(result))
            }
            Err(e) => Ok(ToolResult::error(format!("Failed to recall memories: {}", e))),
        }
    }
}

// ============================================================================
// Forget tool - remove memories
// ============================================================================

#[derive(Debug, Deserialize)]
struct ForgetArgs {
    memory_id: Option<String>,
    title: Option<String>,
    #[serde(default)]
    permanent: bool,
}

pub struct ForgetTool;

impl ForgetTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ForgetTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ForgetTool {
    fn name(&self) -> &str {
        "forget"
    }

    fn description(&self) -> &str {
        "Remove or deactivate memories you no longer need."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "memory_id": {
                    "type": "string",
                    "description": "Specific memory ID to remove"
                },
                "title": {
                    "type": "string",
                    "description": "Remove memories matching this title"
                },
                "permanent": {
                    "type": "boolean",
                    "description": "If true, permanently delete. If false, just deactivate",
                    "default": false
                }
            }
        })
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let parsed: ForgetArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("Argument parsing failed: {}", e))),
        };

        if parsed.memory_id.is_none() && parsed.title.is_none() {
            return Ok(ToolResult::error(
                "Either memory_id or title must be provided".to_string(),
            ));
        }

        // Follow the host's storage scope: `memory_dir` is resolved from
        // `global_storage` at Agent construction (same root as the session
        // DB the manager opens), so memory tools and sessions share one DB.
        let db_path = ctx.memory_dir.join("robit.db");

        let conn = match rusqlite::Connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::error(format!("Failed to open DB: {}", e))),
        };

        if let Some(memory_id) = parsed.memory_id {
            let result = if parsed.permanent {
                delete_memory_permanently(&conn, &memory_id)
            } else {
                deactivate_memory(&conn, &memory_id)
            };

            match result {
                Ok(_) => Ok(ToolResult::success(format!(
                    "Memory {} has been {}.",
                    memory_id,
                    if parsed.permanent {
                        "permanently deleted"
                    } else {
                        "deactivated"
                    }
                ))),
                Err(e) => Ok(ToolResult::error(format!("Failed to remove memory: {}", e))),
            }
        } else if let Some(title) = parsed.title {
            let filter = MemoryFilter {
                session_id: Some(ctx.session_id.to_string()),
                only_active: true,
                ..Default::default()
            };
            match list_memories(&conn, &filter, Some(100)) {
                Ok(memories) => {
                    let matching: Vec<_> = memories.into_iter().filter(|m| m.title == title).collect();
                    if matching.is_empty() {
                        return Ok(ToolResult::error(format!("No memory found with title: {}", title)));
                    }

                    let mut removed = 0;
                    for memory in &matching {
                        let result = if parsed.permanent {
                            delete_memory_permanently(&conn, &memory.id)
                        } else {
                            deactivate_memory(&conn, &memory.id)
                        };
                        if result.is_ok() {
                            removed += 1;
                        }
                    }

                    Ok(ToolResult::success(format!(
                        "Removed {} memory{} with title: {}",
                        removed,
                        if removed == 1 { "" } else { "s" },
                        title
                    )))
                }
                Err(e) => Ok(ToolResult::error(format!("Failed to find memories: {}", e))),
            }
        } else {
            Ok(ToolResult::error(
                "Internal error: neither memory_id nor title".to_string(),
            ))
        }
    }
}

// ============================================================================
// ListMemories tool - list all active memories
// ============================================================================

#[derive(Debug, Deserialize)]
struct ListMemoriesArgs {
    limit: Option<usize>,
    memory_type: Option<String>,
}

pub struct ListMemoriesTool;

impl ListMemoriesTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ListMemoriesTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ListMemoriesTool {
    fn name(&self) -> &str {
        "list_memories"
    }

    fn description(&self) -> &str {
        "List all active memories for a quick overview."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of memories to list",
                    "default": 20
                },
                "memory_type": {
                    "type": "string",
                    "description": "Filter by memory type: fact, preference, note, task"
                }
            }
        })
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let parsed: ListMemoriesArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("Argument parsing failed: {}", e))),
        };

        // Follow the host's storage scope: `memory_dir` is resolved from
        // `global_storage` at Agent construction (same root as the session
        // DB the manager opens), so memory tools and sessions share one DB.
        let db_path = ctx.memory_dir.join("robit.db");

        let conn = match rusqlite::Connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::error(format!("Failed to open DB: {}", e))),
        };

        let filter = MemoryFilter {
            memory_type: parsed.memory_type.as_ref().map(|s| MemoryType::from_str(s)),
            session_id: Some(ctx.session_id.to_string()),
            only_active: true,
            ..Default::default()
        };

        match list_memories(&conn, &filter, parsed.limit) {
            Ok(memories) if memories.is_empty() => Ok(ToolResult::success(
                "No active memories yet. Use `memorize` to store something!".to_string(),
            )),
            Ok(memories) => {
                let mut result = format!("Your memories ({} total):\n\n", memories.len());
                for (i, memory) in memories.iter().enumerate() {
                    result.push_str(&format!(
                        "{}. [{}] {}\n   ID: {}\n   Tags: {}\n\n",
                        i + 1,
                        memory.memory_type.as_str(),
                        memory.title,
                        memory.id,
                        if memory.tags.is_empty() {
                            "(none)".to_string()
                        } else {
                            memory.tags.join(", ")
                        }
                    ));
                }
                Ok(ToolResult::success(result))
            }
            Err(e) => Ok(ToolResult::error(format!("Failed to list memories: {}", e))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    struct DummyFrontend;
    #[async_trait]
    impl crate::frontend::Frontend for DummyFrontend {
        async fn on_event(&self, _: crate::event::AgentEvent) -> Result<()> {
            Ok(())
        }
        async fn request_tool_confirmation(&self, _: &crate::tool::ToolCallInfo) -> Result<bool> {
            Ok(true)
        }
    }

    fn ctx_with_memory_dir(working_dir: PathBuf, memory_dir: PathBuf) -> ToolContext {
        let (done_tx, _done_rx) = tokio::sync::mpsc::channel(1);
        ToolContext {
            working_dir,
            memory_dir,
            session_id: "sess".to_string(),
            tool_call_id: "tc".to_string(),
            frontend: Arc::new(DummyFrontend),
            extensions: std::collections::HashMap::new(),
            supports_images: false,
            async_runner: crate::tool::async_runner::AsyncTaskRunner::new(done_tx),
            cancel_token: tokio_util::sync::CancellationToken::new(),
            task_registry: crate::tool::task_registry::TaskRegistry::new(),
        }
    }

    /// H5: memory tools must write to `ctx.memory_dir/robit.db` — the same
    /// root the host opened for sessions when `global_storage = true` — not
    /// to a hardcoded project-local path.
    #[tokio::test]
    async fn memorize_uses_context_memory_dir_not_working_dir() {
        let working = tempfile::tempdir().unwrap();
        let global = tempfile::tempdir().unwrap();
        // The host (manager/frontend) initializes the schema at the global root.
        let db_path = global.path().join("robit.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        crate::storage::init_db(&conn).unwrap();
        // memories.session_id references sessions(id) — create the session row.
        crate::storage::insert_session(&conn, "sess", None, "T", "model", "gui", "").unwrap();
        drop(conn);

        let ctx = ctx_with_memory_dir(
            working.path().to_path_buf(),
            global.path().to_path_buf(),
        );
        let result = MemorizeTool::new()
            .execute(
                serde_json::json!({"title": "t", "content": "c"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!result.is_error, "memorize failed: {}", result.content);

        // Row landed in the global DB…
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        drop(conn);
        // …and no project-local DB was created behind the host's back.
        assert!(!working.path().join(".robit/memory/robit.db").exists());
    }
}
