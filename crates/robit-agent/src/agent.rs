//! Agent — the event-driven loop that orchestrates LLM calls and tool execution.

use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessage, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessage, ChatCompletionRequestToolMessage,
    ChatCompletionRequestUserMessage, FunctionCall,
};
use futures::StreamExt;
use robit_ai::config::ContextConfig;
use robit_ai::LlmClient;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::context::ContextManager;
use crate::error::{AgentError, Result};
use crate::event::{new_session_id, AgentEvent, FrontendMessage, SessionId};
use crate::frontend::Frontend;
use crate::prompt::PromptBuilder;
use crate::skill::SkillRegistry;
use crate::tool::{ToolCallInfo, ToolContext, ToolRegistry, ToolResult};

// ============================================================================
// AgentSession
// ============================================================================

/// A single conversation session with its own message history.
pub struct AgentSession {
    pub session_id: SessionId,
    pub history: Vec<ChatCompletionRequestMessage>,
    pub working_dir: PathBuf,
}

impl AgentSession {
    fn new(session_id: SessionId, working_dir: PathBuf, system_prompt: String) -> Self {
        let system_msg = ChatCompletionRequestMessage::System(
            ChatCompletionRequestSystemMessage {
                content: system_prompt.into(),
                name: None,
            }
            .into(),
        );

        Self {
            session_id,
            history: vec![system_msg],
            working_dir,
        }
    }
}

// ============================================================================
// Agent
// ============================================================================

/// The Agent orchestrates LLM calls and tool execution.
pub struct Agent {
    llm_client: Arc<LlmClient>,
    tools: Arc<ToolRegistry>,
    skills: Arc<SkillRegistry>,
    sessions: HashMap<SessionId, AgentSession>,
    default_session_id: SessionId,
    context_manager: ContextManager,
    frontend: Arc<dyn Frontend>,
    auto_approve: bool,
}

impl Agent {
    /// Create a new Agent with the given dependencies.
    pub fn new(
        llm_client: Arc<LlmClient>,
        tools: Arc<ToolRegistry>,
        skills: Arc<SkillRegistry>,
        frontend: Arc<dyn Frontend>,
        context_config: Option<&ContextConfig>,
        context_window: Option<u64>,
        working_dir: PathBuf,
        auto_approve: bool,
    ) -> Self {
        let prompt_builder = PromptBuilder::new();
        let context_manager = ContextManager::new(context_window, context_config);

        // Build system prompt with tools AND skills
        let tool_refs: Vec<&dyn crate::tool::Tool> = tools.tools();
        let skill_descs = skills.skill_descriptions();
        let system_prompt = prompt_builder.build_system_prompt(&tool_refs, &skill_descs, &working_dir);

        // Create default session
        let session_id = new_session_id();
        let session = AgentSession::new(session_id.clone(), working_dir, system_prompt);

        let mut sessions = HashMap::new();
        sessions.insert(session_id.clone(), session);

        Self {
            llm_client,
            tools,
            skills,
            sessions,
            default_session_id: session_id,
            context_manager,
            frontend,
            auto_approve,
        }
    }

    /// Run the agent's main event loop. Takes ownership of the message receiver.
    /// Returns when the channel is closed or user types /exit.
    pub async fn run(mut self, mut message_rx: mpsc::Receiver<FrontendMessage>) {
        tracing::info!("Agent started, session: {}", self.default_session_id);

        while let Some(msg) = message_rx.recv().await {
            match msg {
                FrontendMessage::UserInput(input) => {
                    if input == "/exit" || input == "/quit" {
                        break;
                    }
                    if input == "/clear" {
                        self.clear_session();
                        let _ = self
                            .frontend
                            .on_event(AgentEvent::TextDelta(
                                "\n[Conversation history cleared]\n".to_string(),
                            ))
                            .await;
                        let _ = self.frontend.on_event(AgentEvent::TurnComplete).await;
                        continue;
                    }

                    // Check for skill trigger
                    if let Some((skill, args)) = self.skills.match_trigger(&input) {
                        let skill = skill.clone();
                        self.run_skill_turn(&skill, &args).await;
                        continue;
                    }

                    self.run_turn(&input).await;
                }
                FrontendMessage::Cancel => {
                    tracing::info!("Cancel requested (MVP: no-op)");
                }
                FrontendMessage::ConfirmationResponse { .. } => {
                    // Confirmation is handled via frontend.request_tool_confirmation()
                    // within run_one_step. This variant is reserved for future async flow.
                    tracing::warn!("Unexpected ConfirmationResponse outside tool confirmation");
                }
            }
        }

        tracing::info!("Agent stopped");
    }

    /// Execute a single turn: user input -> LLM call(s) -> tool execution(s) -> response.
    async fn run_turn(&mut self, user_input: &str) {
        let session_id = self.default_session_id.clone();

        // Add user message to history
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.history.push(ChatCompletionRequestMessage::User(
                ChatCompletionRequestUserMessage {
                    content: user_input.to_string().into(),
                    name: None,
                }
                .into(),
            ));
        }

        // Run the agentic loop (may iterate if LLM calls tools)
        let max_iterations = 20;
        for iteration in 0..max_iterations {
            match self.run_one_step(&session_id).await {
                Ok(has_tool_calls) => {
                    if !has_tool_calls {
                        let _ = self.frontend.on_event(AgentEvent::TurnComplete).await;
                        return;
                    }
                    tracing::debug!(
                        "Iteration {}: tool calls executed, continuing loop",
                        iteration
                    );
                }
                Err(e) => {
                    let _ = self.frontend.on_event(AgentEvent::Error(e)).await;
                    let _ = self.frontend.on_event(AgentEvent::TurnComplete).await;
                    return;
                }
            }
        }

        // Safety limit
        let _ = self
            .frontend
            .on_event(AgentEvent::Error(AgentError::InternalError(
                format!("Max iterations reached ({})", max_iterations),
            )))
            .await;
        let _ = self.frontend.on_event(AgentEvent::TurnComplete).await;
    }

    /// Run one step: call LLM, process response, execute tools.
    /// Returns Ok(true) if tool calls were made (loop should continue).
    /// Returns Ok(false) if the LLM responded with text only (turn complete).
    async fn run_one_step(&mut self, session_id: &SessionId) -> Result<bool> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| AgentError::InternalError("Session not found".to_string()))?;

        // Truncate context if needed
        let truncation_result = self.context_manager.maybe_truncate(&mut session.history);

        // Handle async compression if needed
        if truncation_result.needs_compression {
            // For now, replace placeholder with a notice
            // In production, spawn async task to call LLM and replace with summary
            if let Some(msg) = session.history.get_mut(truncation_result.insert_position) {
                let notice = format!(
                    "[Omitted {} rounds, {} messages. Context compressed to save space]",
                    truncation_result.rounds_removed,
                    truncation_result.messages_removed
                );
                *msg = ChatCompletionRequestMessage::User(
                    ChatCompletionRequestUserMessage {
                        content: notice.into(),
                        name: Some("system_notice".to_string()),
                    }
                    .into(),
                );
            }

            tracing::info!(
                "Compression triggered: removed {} tokens (threshold: {})",
                crate::context::estimate_messages_tokens(&truncation_result.removed_messages),
                self.context_manager.compression_token_threshold
            );
        }

        // Build tool schemas
        let tool_schemas = self.tools.tool_schemas();
        let tools_param = if tool_schemas.is_empty() {
            None
        } else {
            Some(tool_schemas)
        };

        // Call LLM (streaming)
        let mut stream = self
            .llm_client
            .chat_stream(session.history.clone(), tools_param)
            .await?;

        // Collect streaming response
        let mut full_text = String::new();
        let mut tool_call_chunks: HashMap<usize, ToolCallAccumulator> = HashMap::new();

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| AgentError::LlmError(e.into()))?;

            if let Some(choice) = chunk.choices.first() {
                // Text content
                if let Some(content) = &choice.delta.content {
                    full_text.push_str(content);
                    let _ = self
                        .frontend
                        .on_event(AgentEvent::TextDelta(content.clone()))
                        .await;
                }

                // Tool call deltas
                if let Some(tool_calls) = &choice.delta.tool_calls {
                    for tc in tool_calls {
                        let acc = tool_call_chunks
                            .entry(tc.index as usize)
                            .or_insert_with(ToolCallAccumulator::new);

                        if let Some(id) = &tc.id {
                            acc.id = Some(id.clone());
                        }
                        if let Some(function) = &tc.function {
                            if let Some(name) = &function.name {
                                acc.name = Some(name.clone());
                            }
                            if let Some(args) = &function.arguments {
                                acc.arguments.push_str(args);
                            }
                        }
                    }
                }
            }
        }

        // Assemble complete tool calls from chunks
        let assembled_tool_calls: Vec<ChatCompletionMessageToolCall> = {
            let mut indices: Vec<usize> = tool_call_chunks.keys().cloned().collect();
            indices.sort();
            indices
                .into_iter()
                .filter_map(|idx| tool_call_chunks.remove(&idx)?.into_tool_call())
                .collect()
        };

        // Add assistant message to history
        let assistant_msg = ChatCompletionRequestMessage::Assistant(
            ChatCompletionRequestAssistantMessage {
                content: if full_text.is_empty() {
                    None
                } else {
                    Some(full_text.clone().into())
                },
                name: None,
                tool_calls: if assembled_tool_calls.is_empty() {
                    None
                } else {
                    Some(
                        assembled_tool_calls
                            .clone()
                            .into_iter()
                            .map(ChatCompletionMessageToolCalls::Function)
                            .collect(),
                    )
                },
                refusal: None,
                audio: None,
                #[allow(deprecated)]
                function_call: None,
            }
            .into(),
        );

        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| AgentError::InternalError("Session not found".to_string()))?;
        session.history.push(assistant_msg);
        let working_dir = session.working_dir.clone();

        // If no tool calls, turn is complete
        if assembled_tool_calls.is_empty() {
            return Ok(false);
        }

        // Execute each tool call
        for tc in &assembled_tool_calls {
            let tc_info = ToolCallInfo {
                id: tc.id.clone(),
                name: tc.function.name.clone(),
                arguments: tc.function.arguments.clone(),
            };

            // Notify frontend
            let _ = self
                .frontend
                .on_event(AgentEvent::ToolCallRequested {
                    tool_call_id: tc_info.id.clone(),
                    name: tc_info.name.clone(),
                    arguments: tc_info.arguments.clone(),
                })
                .await;

            // Check confirmation
            let approved = if self.tools.requires_confirmation(&tc.function.name) && !self.auto_approve {
                self.frontend.request_tool_confirmation(&tc_info).await?
            } else {
                true
            };

            // Execute or reject
            let result = if approved {
                let args: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                    .unwrap_or(serde_json::Value::Null);

                let ctx = ToolContext {
                    working_dir: working_dir.clone(),
                    session_id: session_id.clone(),
                    frontend: self.frontend.clone(),
                };

                self.tools.execute(&tc.function.name, args, &ctx).await
            } else {
                ToolResult::error("User rejected this tool call")
            };

            // Truncate output
            let truncated_result = ToolResult {
                content: self.context_manager.truncate_tool_output(&result.content),
                is_error: result.is_error,
            };

            // Notify frontend of result
            let _ = self
                .frontend
                .on_event(AgentEvent::ToolCallResult {
                    tool_call_id: tc.id.clone(),
                    result: truncated_result.clone(),
                })
                .await;

            // Add tool result to history
            let tool_msg = ChatCompletionRequestMessage::Tool(
                ChatCompletionRequestToolMessage {
                    content: truncated_result.content.into(),
                    tool_call_id: tc.id.clone(),
                }
                .into(),
            );

            let session = self
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| AgentError::InternalError("Session not found".to_string()))?;
            session.history.push(tool_msg);
        }

        Ok(true)
    }

    /// Clear the current session's history (keep system prompt).
    fn clear_session(&mut self) {
        if let Some(session) = self.sessions.get_mut(&self.default_session_id) {
            session.history.truncate(1);
        }
    }

    /// Execute a skill-triggered turn: inject skill content, then run the agent loop.
    ///
    /// The skill's full content is injected as a temporary system message and removed
    /// after the turn completes, so it doesn't occupy context in future turns.
    async fn run_skill_turn(&mut self, skill: &crate::skill::Skill, args: &str) {
        // Notify frontend
        let _ = self
            .frontend
            .on_event(AgentEvent::SkillTriggered {
                name: skill.frontmatter.name.clone(),
                description: skill.frontmatter.description.clone(),
            })
            .await;

        let session_id = self.default_session_id.clone();

        // Inject skill content as a system message
        let skill_message = format!(
            "## Skill: {}\n\n{}\n\n{}",
            skill.frontmatter.name,
            skill.frontmatter.description,
            skill.content
        );

        let skill_msg = ChatCompletionRequestMessage::System(
            ChatCompletionRequestSystemMessage {
                content: skill_message.into(),
                name: Some(skill.frontmatter.name.clone()),
            }
            .into(),
        );

        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.history.push(skill_msg);
        }

        // Add user message (args or default)
        let user_content = if args.is_empty() {
            "(User triggered skill, no additional arguments)".to_string()
        } else {
            args.to_string()
        };

        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.history.push(ChatCompletionRequestMessage::User(
                ChatCompletionRequestUserMessage {
                    content: user_content.into(),
                    name: None,
                }
                .into(),
            ));
        }

        // Run the agentic loop
        let max_iterations = 20;
        let mut completed = false;
        for iteration in 0..max_iterations {
            match self.run_one_step(&session_id).await {
                Ok(has_tool_calls) => {
                    if !has_tool_calls {
                        completed = true;
                        break;
                    }
                    tracing::debug!(
                        "Skill iteration {}: tool calls executed",
                        iteration
                    );
                }
                Err(e) => {
                    let _ = self.frontend.on_event(AgentEvent::Error(e)).await;
                    break;
                }
            }
        }

        if !completed {
            let _ = self
                .frontend
                .on_event(AgentEvent::Error(AgentError::InternalError(
                    format!("Max iterations reached ({})", max_iterations),
                )))
                .await;
        }

        let _ = self.frontend.on_event(AgentEvent::TurnComplete).await;

        // Remove the injected skill system message to avoid polluting future turns
        if let Some(session) = self.sessions.get_mut(&session_id) {
            let skill_name = skill.frontmatter.name.clone();
            session.history.retain(|msg| {
                !matches!(
                    msg,
                    ChatCompletionRequestMessage::System(s)
                        if s.name.as_deref() == Some(&skill_name)
                )
            });
        }
    }
}

// ============================================================================
// Helper types
// ============================================================================

/// Accumulates streaming tool call chunks.
struct ToolCallAccumulator {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl ToolCallAccumulator {
    fn new() -> Self {
        Self {
            id: None,
            name: None,
            arguments: String::new(),
        }
    }

    /// Convert accumulated chunks into a complete tool call.
    fn into_tool_call(self) -> Option<ChatCompletionMessageToolCall> {
        let id = self.id?;
        let name = self.name?;
        Some(ChatCompletionMessageToolCall {
            id,
            function: FunctionCall {
                name,
                arguments: self.arguments,
            },
        })
    }
}
