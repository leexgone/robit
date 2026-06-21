//! Tool system: trait, registry, result types, and context.

pub mod bash;
pub mod read;
pub mod write;
pub mod edit;
pub mod load_skill;
pub mod ls;
pub mod find;
pub mod grep;

use async_trait::async_trait;
use robit_ai::ChatCompletionTools;
use serde_json::Value;
use std::collections::HashMap;
use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::Result;
use crate::event::SessionId;
use crate::frontend::Frontend;

// ============================================================================
// Tool trait
// ============================================================================

/// A tool that can be called by the LLM and executed by the Agent.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name — LLM calls the tool by this name.
    fn name(&self) -> &str;

    /// Tool description — injected into system prompt for LLM understanding.
    fn description(&self) -> &str;

    /// JSON Schema for tool parameters — LLM generates arguments based on this.
    fn parameters_schema(&self) -> Value;

    /// Whether this tool requires user confirmation before execution.
    fn requires_confirmation(&self) -> bool;

    /// Execute the tool with parsed arguments. Returns ToolResult for LLM consumption.
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult>;
}

// ============================================================================
// ToolResult
// ============================================================================

/// Result returned to the LLM after tool execution.
#[derive(Debug, Clone)]
pub struct ToolResult {
    /// Text content — LLM will read this.
    pub content: String,
    /// Whether this is an error (LLM can see errors and adjust strategy).
    pub is_error: bool,
}

impl ToolResult {
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

// ============================================================================
// Shared helpers
// ============================================================================

/// Resolve a file path relative to the working directory.
pub fn resolve_path(file_path: &str, working_dir: &Path) -> PathBuf {
    let p = PathBuf::from(file_path);
    if p.is_absolute() {
        p
    } else {
        working_dir.join(p)
    }
}

// ============================================================================
// ToolContext
// ============================================================================

/// Runtime context passed to tools during execution.
pub struct ToolContext {
    /// Current working directory.
    pub working_dir: PathBuf,
    /// Current session ID.
    pub session_id: SessionId,
    /// Frontend for user interaction (e.g., asking for input during tool execution).
    pub frontend: Arc<dyn Frontend>,
    /// Platform-specific extensions, keyed by extension ID.
    /// Chatbot platforms populate this; GUI/TUI leave it empty.
    /// Keys like "chatbot.platform_ext" map to Arc<dyn PlatformExt>.
    pub extensions: HashMap<String, Arc<dyn Any + Send + Sync>>,
}

// ============================================================================
// ToolCallInfo (for confirmation requests)
// ============================================================================

/// Information about a tool call, used for confirmation requests.
#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

// ============================================================================
// ToolRegistry
// ============================================================================

/// Registry that manages all available tools.
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool. Overwrites any existing tool with the same name.
    pub fn register(&mut self, tool: impl Tool + 'static) {
        self.tools.insert(tool.name().to_string(), Box::new(tool));
    }

    /// Get a list of all registered tool names.
    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    /// Check if a tool exists.
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Generate OpenAI function calling schemas for all registered tools.
    pub fn tool_schemas(&self) -> Vec<ChatCompletionTools> {
        self.tools
            .values()
            .map(|tool| {
                let function = serde_json::json!({
                    "name": tool.name(),
                    "description": tool.description(),
                    "parameters": tool.parameters_schema(),
                });

                // Construct ChatCompletionTool via JSON deserialization
                let tool_json = serde_json::json!({
                    "type": "function",
                    "function": function,
                });

                serde_json::from_value(tool_json)
                    .expect("tool schema should be valid ChatCompletionTools")
            })
            .collect()
    }

    /// Execute a tool by name. Returns an error ToolResult if the tool doesn't exist.
    pub async fn execute(
        &self,
        name: &str,
        args: Value,
        ctx: &ToolContext,
    ) -> ToolResult {
        match self.tools.get(name) {
            Some(tool) => match tool.execute(args, ctx).await {
                Ok(result) => result,
                Err(e) => ToolResult::error(format!("Tool execution error: {}", e)),
            },
            None => {
                let available: Vec<&str> = self.tools.keys().map(|s| s.as_str()).collect();
                ToolResult::error(format!(
                    "Tool '{}' not found. Available tools: {:?}",
                    name, available
                ))
            }
        }
    }

    /// Check if a tool requires confirmation.
    pub fn requires_confirmation(&self, name: &str) -> bool {
        self.tools
            .get(name)
            .map(|t| t.requires_confirmation())
            .unwrap_or(false)
    }

    /// Get references to all tools (for prompt building).
    pub fn tools(&self) -> Vec<&dyn Tool> {
        self.tools.values().map(|t| t.as_ref()).collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
