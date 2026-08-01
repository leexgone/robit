//! Tool system: trait, registry, result types, and context.

pub mod bash;
pub mod read;
pub mod write;
pub mod edit;
pub mod generate_image;
pub mod load_skill;
pub mod ls;
pub mod find;
pub mod grep;
pub mod memory;
pub mod search_history;
pub mod async_runner;
pub mod task_registry;
pub mod query_task;

use async_trait::async_trait;
use robit_ai::ChatCompletionTools;
use serde_json::Value;
use std::collections::HashMap;
use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::event::SessionId;
use crate::frontend::Frontend;
use async_runner::AsyncTaskRunner;
use task_registry::TaskRegistry;

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

    /// Whether this tool is capable of running asynchronously (returning a
    /// pending placeholder and finishing its work in the background).
    ///
    /// This is **advisory only** - used by frontends/Agent for UI hints (e.g.
    /// showing a "task in progress" affordance). Whether a *given invocation*
    /// actually runs async is decided at runtime inside `execute` (e.g. based
    /// on the provider protocol or input size), by calling
    /// `ctx.async_runner.submit(..)` and returning `ToolResult::pending(..)`.
    /// Tools that never run async should leave the default `false`.
    fn supports_async(&self) -> bool {
        false
    }
}

// ============================================================================
// ToolResult
// ============================================================================

/// A single image attached to a tool result.
///
/// When a tool (e.g. `read` on an image file) produces images and the model
/// supports image inputs, the agent injects them as a multimodal user message
/// after the tool message (OpenAI protocol restricts tool message content to
/// text, so images cannot travel in the tool result itself).
#[derive(Debug, Clone)]
pub struct ToolImage {
    /// Base64 data URL, e.g. "data:image/png;base64,...".
    pub data_url: String,
    /// Human-readable label for log / fallback text.
    pub label: String,
}

/// Result returned to the LLM after tool execution.
#[derive(Debug, Clone)]
pub struct ToolResult {
    /// Text content - LLM will read this.
    pub content: String,
    /// Whether this is an error (LLM can see errors and adjust strategy).
    pub is_error: bool,
    /// Images attached to this result (e.g. from `read` tool reading an image
    /// file). Most tools leave this empty.
    pub images: Vec<ToolImage>,
    /// `true` when this is a *placeholder* for an async task: `content` tells
    /// the LLM the work is in progress, and the real result is reinjected
    /// later (by the Agent) when the background task finishes. The Agent uses
    /// this flag to emit `AsyncToolStarted` instead of treating the call as
    /// finished. The placeholder content is still added to history as the tool
    /// message so the LLM can continue other work while waiting.
    pub is_pending: bool,
    /// Task id of the background task, set iff `is_pending`. Used by the Agent
    /// to track/cancel the task and by the frontend to correlate progress.
    pub pending_task_id: Option<String>,
}

impl ToolResult {
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            images: Vec::new(),
            is_pending: false,
            pending_task_id: None,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            images: Vec::new(),
            is_pending: false,
            pending_task_id: None,
        }
    }

    /// Build a pending placeholder for an async task. `content` should tell the
    /// LLM what is happening and the `task_id` it can reference later.
    pub fn pending(content: impl Into<String>, task_id: String) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            images: Vec::new(),
            is_pending: true,
            pending_task_id: Some(task_id),
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
    /// The tool call id this execution was triggered by. Needed by async tools
    /// to correlate their background task with the originating call.
    pub tool_call_id: String,
    /// Frontend for user interaction (e.g., asking for input during tool execution).
    pub frontend: Arc<dyn Frontend>,
    /// Platform-specific extensions, keyed by extension ID.
    /// Chatbot platforms populate this; GUI/TUI leave it empty.
    /// Keys like "chatbot.platform_ext" map to Arc<dyn PlatformExt>.
    pub extensions: HashMap<String, Arc<dyn Any + Send + Sync>>,
    /// Whether the configured LLM supports image inputs.
    /// Tools (e.g. `read`) use this to decide whether to encode images.
    pub supports_images: bool,
    /// Handle for submitting async background work. A tool that decides (at
    /// runtime) a call should run async calls `async_runner.submit(..)` and
    /// returns `ToolResult::pending(..)`. Cheap to clone.
    pub async_runner: AsyncTaskRunner,
    /// Cancellation token for this tool call. Async tools pass a clone into
    /// `async_runner.submit` so the Agent can cancel the background work.
    /// Sync tools ignore it.
    pub cancel_token: CancellationToken,
    /// Shared registry tracking all async tasks for the current Agent. The
    /// `query_task` tool reads this; async tools register themselves here via
    /// the Agent when they submit. Cheap to clone (shared `Arc`).
    pub task_registry: TaskRegistry,
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
        tracing::info!("ToolRegistry.execute called: name='{}', args={:?}", name, args);
        tracing::debug!("Available tools: {:?}", self.tool_names());

        match self.tools.get(name) {
            Some(tool) => {
                tracing::debug!("Found tool '{}', executing...", name);
                let started = std::time::Instant::now();
                let outcome = tool.execute(args, ctx).await;
                let elapsed = started.elapsed();
                match &outcome {
                    Ok(result) => tracing::trace!(
                        "[tool:{}] execution finished in {:?}: is_error={}, content_len={}",
                        name,
                        elapsed,
                        result.is_error,
                        result.content.len()
                    ),
                    Err(e) => tracing::warn!(
                        "[tool:{}] execution returned error after {:?}: {}",
                        name,
                        elapsed,
                        e
                    ),
                }
                match outcome {
                    Ok(result) => result,
                    Err(e) => ToolResult::error(format!("Tool execution error: {}", e)),
                }
            },
            None => {
                let available: Vec<&str> = self.tools.keys().map(|s| s.as_str()).collect();
                tracing::error!("Tool '{}' not found! Available tools: {:?}", name, available);
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

    /// Names of tools that declare async capability (`supports_async() == true`).
    /// Advisory: frontends use this for UI hints (e.g. a progress affordance).
    /// Whether an invocation actually runs async is still decided at runtime
    /// inside `execute`.
    pub fn async_capable_tools(&self) -> Vec<&str> {
        self.tools
            .values()
            .filter(|t| t.supports_async())
            .map(|t| t.name())
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
