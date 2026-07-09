//! Agent — the event-driven loop that orchestrates LLM calls and tool execution.

use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessage, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessage, ChatCompletionRequestToolMessage,
    ChatCompletionRequestUserMessage, ChatCompletionRequestUserMessageContent,
    ChatCompletionRequestUserMessageContentPart,
    ChatCompletionRequestMessageContentPartText,
    ChatCompletionRequestMessageContentPartImage,
    FunctionCall,
};

// Import ImageUrl from wherever it is in async-openai 0.41
use async_openai::types::chat::ImageUrl;
use futures::StreamExt;
use robit_ai::config::ContextConfig;
use robit_ai::LlmClient;
use std::any::Any;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::context::ContextManager;
use crate::error::{AgentError, Result};
use crate::event::{new_session_id, AgentEvent, FrontendMessage, MediaAttachment, SessionId};
use crate::frontend::Frontend;
use crate::media;
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

    /// Create session with pre-loaded history
    pub fn with_history(
        session_id: SessionId,
        working_dir: PathBuf,
        system_prompt: String,
        history: Vec<ChatCompletionRequestMessage>,
    ) -> Self {
        // Create system message (new one with latest config)
        let system_msg = ChatCompletionRequestMessage::System(
            ChatCompletionRequestSystemMessage {
                content: system_prompt.into(),
                name: None,
            }
            .into(),
        );

        // Prepend new system message to history
        let mut full_history = vec![system_msg];
        full_history.extend(history);

        Self {
            session_id,
            history: full_history,
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
    /// Platform-specific extensions passed to ToolContext during tool execution.
    extensions: HashMap<String, Arc<dyn Any + Send + Sync>>,
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
        extensions: HashMap<String, Arc<dyn Any + Send + Sync>>,
    ) -> Self {
        let prompt_builder = PromptBuilder::with_working_dir(Some(&working_dir));
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
            extensions,
        }
    }

    /// Create Agent with pre-loaded history (for resuming sessions)
    pub fn with_history(
        llm_client: Arc<LlmClient>,
        tools: Arc<ToolRegistry>,
        skills: Arc<SkillRegistry>,
        frontend: Arc<dyn Frontend>,
        context_config: Option<&ContextConfig>,
        context_window: Option<u64>,
        working_dir: PathBuf,
        auto_approve: bool,
        extensions: HashMap<String, Arc<dyn Any + Send + Sync>>,
        session_id: SessionId,
        history: Vec<ChatCompletionRequestMessage>,
    ) -> Self {
        tracing::info!(
            "Agent::with_history: session_id={}, received {} history messages",
            session_id,
            history.len()
        );
        for (idx, msg) in history.iter().enumerate() {
            let role = match msg {
                ChatCompletionRequestMessage::System(_) => "system",
                ChatCompletionRequestMessage::User(_) => "user",
                ChatCompletionRequestMessage::Assistant(_) => "assistant",
                ChatCompletionRequestMessage::Tool(_) => "tool",
                ChatCompletionRequestMessage::Developer(_) => "developer",
                ChatCompletionRequestMessage::Function(_) => "function",
            };
            tracing::debug!("  History message {}: role={}", idx, role);
        }

        let prompt_builder = PromptBuilder::with_working_dir(Some(&working_dir));
        let context_manager = ContextManager::new(context_window, context_config);

        // Build system prompt with tools AND skills
        let tool_refs: Vec<&dyn crate::tool::Tool> = tools.tools();
        let skill_descs = skills.skill_descriptions();
        let system_prompt = prompt_builder.build_system_prompt(&tool_refs, &skill_descs, &working_dir);

        // Create session with history
        let mut session = AgentSession::with_history(
            session_id.clone(),
            working_dir,
            system_prompt,
            history,
        );

        tracing::info!(
            "Agent::with_history: after adding system prompt, session history length = {}",
            session.history.len()
        );
        for (idx, msg) in session.history.iter().enumerate() {
            let role = match msg {
                ChatCompletionRequestMessage::System(_) => "system",
                ChatCompletionRequestMessage::User(_) => "user",
                ChatCompletionRequestMessage::Assistant(_) => "assistant",
                ChatCompletionRequestMessage::Tool(_) => "tool",
                ChatCompletionRequestMessage::Developer(_) => "developer",
                ChatCompletionRequestMessage::Function(_) => "function",
            };
            tracing::debug!("  Session history {}: role={}", idx, role);
        }

        // Apply context truncation before starting
        let truncation_result = context_manager.maybe_truncate(&mut session.history);
        if truncation_result.rounds_removed > 0 {
            tracing::info!(
                "Agent::with_history: truncated {} rounds ({} messages), needs_compression={}",
                truncation_result.rounds_removed,
                truncation_result.messages_removed,
                truncation_result.needs_compression
            );
        }
        tracing::debug!(
            "Agent::with_history: after truncation, session history length = {}",
            session.history.len()
        );

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
            extensions,
        }
    }

    /// Run the agent's main event loop. Takes ownership of the message receiver.
    /// Returns when the channel is closed or user types /exit.
    pub async fn run(mut self, mut message_rx: mpsc::Receiver<FrontendMessage>) {
        tracing::info!("Agent started, session: {}", self.default_session_id);

        while let Some(msg) = message_rx.recv().await {
            match msg {
                FrontendMessage::UserInput { text, attachments } => {
                    if text == "/exit" || text == "/quit" {
                        break;
                    }
                    if text == "/clear" {
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
                    if let Some((skill, args)) = self.skills.match_trigger(&text) {
                        let skill = skill.clone();
                        self.run_skill_turn(&skill, &args).await;
                        continue;
                    }

                    self.run_turn(&text, attachments).await;
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
    async fn run_turn(&mut self, user_input: &str, attachments: Vec<MediaAttachment>) {
        let session_id = self.default_session_id.clone();
        let max_tool_calls = self.context_manager.max_tool_calls_per_turn;

        // Build user message first (to avoid borrow conflict)
        let user_message = self.build_user_message(user_input, &attachments).await;

        // Add user message to history
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.history.push(user_message);
        }

        // Run the agentic loop (may iterate if LLM calls tools)
        let max_iterations = 20;
        let mut total_tool_calls = 0usize;
        for iteration in 0..max_iterations {
            match self.run_one_step(&session_id).await {
                Ok(tool_call_count) => {
                    if tool_call_count == 0 {
                        let _ = self.frontend.on_event(AgentEvent::TurnComplete).await;
                        return;
                    }
                    total_tool_calls += tool_call_count;

                    // Check against per-turn tool call limit
                    if total_tool_calls >= max_tool_calls {
                        tracing::warn!(
                            "Tool call limit reached: {} >= {} (max_tool_calls_per_turn), forcing turn completion",
                            total_tool_calls,
                            max_tool_calls
                        );
                        let _ = self
                            .frontend
                            .on_event(AgentEvent::TextDelta(
                                format!(
                                    "\n\n[Tool call limit reached ({} calls). Please summarize progress and continue in the next message.]\n",
                                    total_tool_calls
                                ),
                            ))
                            .await;
                        let _ = self.frontend.on_event(AgentEvent::TurnComplete).await;
                        return;
                    }

                    tracing::debug!(
                        "Iteration {}: {} tool calls executed (total: {}/{}), continuing loop",
                        iteration,
                        tool_call_count,
                        total_tool_calls,
                        max_tool_calls
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
    /// Returns the number of tool calls executed (0 = turn complete, no tools called).
    async fn run_one_step(&mut self, session_id: &SessionId) -> Result<usize> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| AgentError::InternalError("Session not found".to_string()))?;

        // Truncate context if needed
        let truncation_result = self.context_manager.maybe_truncate(&mut session.history);

        // Handle compression: generate actual summary via LLM
        if truncation_result.needs_compression {
            let summary = generate_summary(&self.llm_client, &truncation_result.removed_messages).await;

            // Replace the placeholder notice with the actual summary
            if let Some(msg) = session.history.get_mut(truncation_result.insert_position) {
                let notice = format!("[Earlier conversation summary: {}]", summary);
                *msg = ChatCompletionRequestMessage::User(
                    ChatCompletionRequestUserMessage {
                        content: notice.into(),
                        name: Some("system_notice".to_string()),
                    }
                );
            }

            tracing::info!(
                "Compression completed: removed {} tokens, summary inserted",
                crate::context::estimate_messages_tokens(&truncation_result.removed_messages),
            );
        }

        // Build tool schemas
        let tool_schemas = self.tools.tool_schemas();
        let tools_param = if tool_schemas.is_empty() {
            None
        } else {
            Some(tool_schemas)
        };

        // Log estimated token usage before call
        let estimated_prompt = crate::context::estimate_messages_tokens_with_margin(
            &session.history,
            self.context_manager.token_safety_margin,
        );
        tracing::info!(
            "LLM call: ~{} prompt tokens (with {:.1}x margin), {} messages, threshold={} tokens",
            estimated_prompt,
            self.context_manager.token_safety_margin,
            session.history.len(),
            self.context_manager.truncation_threshold(),
        );

        // Call LLM (streaming)
        let mut stream = self
            .llm_client
            .chat_stream(session.history.clone(), tools_param)
            .await?;

        // Collect streaming response
        let mut full_text = String::new();
        let mut tool_call_chunks: HashMap<usize, ToolCallAccumulator> = HashMap::new();
        let mut api_usage: Option<async_openai::types::chat::CompletionUsage> = None;

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| AgentError::LlmError(e.into()))?;

            // Capture usage info if present in this chunk (some providers include it in final chunk)
            if let Some(ref usage) = chunk.usage {
                api_usage = Some(usage.clone());
            }

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
                        tracing::debug!(
                            "Received tool call chunk: index={}, id={:?}, function={:?}",
                            tc.index,
                            tc.id,
                            tc.function
                        );

                        let acc = tool_call_chunks
                            .entry(tc.index as usize)
                            .or_insert_with(ToolCallAccumulator::new);

                        if let Some(id) = &tc.id {
                            // 只有当id非空时才更新
                            if !id.is_empty() {
                                tracing::debug!("Updating tool id: '{}'", id);
                                acc.id = Some(id.clone());
                            }
                        }
                        if let Some(function) = &tc.function {
                            if let Some(name) = &function.name {
                                // 只有当name非空时才更新
                                if !name.is_empty() {
                                    tracing::debug!("Tool name chunk: '{}'", name);
                                    acc.name = Some(name.clone());
                                }
                            }
                            if let Some(args) = &function.arguments {
                                tracing::debug!("Tool args chunk: '{}'", args);
                                acc.arguments.push_str(args);
                            }
                        }

                        tracing::debug!("Accumulator state after chunk: {:?}", acc);
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

        // Log token usage summary
        let estimated_response = crate::context::estimate_tokens(&full_text);
        if let Some(ref usage) = api_usage {
            tracing::info!(
                "LLM response: API usage = {} prompt + {} completion = {} total tokens. Estimated: ~{} prompt + ~{} response = ~{} total",
                usage.prompt_tokens,
                usage.completion_tokens,
                usage.total_tokens,
                estimated_prompt,
                estimated_response,
                estimated_prompt + estimated_response,
            );
        } else {
            tracing::info!(
                "LLM response: {} chars, ~{} estimated tokens ({} tool calls). API usage not available from streaming.",
                full_text.len(),
                estimated_response,
                assembled_tool_calls.len(),
            );
        }

        tracing::debug!("Assembled {} tool call(s)", assembled_tool_calls.len());
        for (i, tc) in assembled_tool_calls.iter().enumerate() {
            tracing::debug!(
                "Tool call [{}]: id='{}', name='{}', arguments='{}'",
                i,
                tc.id,
                tc.function.name,
                tc.function.arguments
            );
        }

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
            return Ok(0);
        }

        // Execute each tool call
        for tc in &assembled_tool_calls {
            tracing::info!(
                "About to execute tool: id='{}', name='{}'",
                tc.id,
                tc.function.name
            );

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
                    extensions: self.extensions.clone(),
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

        Ok(assembled_tool_calls.len())
    }

    /// Clear the current session's history (keep system prompt).
    fn clear_session(&mut self) {
        if let Some(session) = self.sessions.get_mut(&self.default_session_id) {
            session.history.truncate(1);
        }
    }

    /// Build a user message, potentially with images if model supports them.
    async fn build_user_message(
        &self,
        text: &str,
        attachments: &[MediaAttachment],
    ) -> ChatCompletionRequestMessage {
        // If model supports images and we have image attachments, build multimodal message
        if self.llm_client.supports_images()
            && !attachments.is_empty()
            && attachments.iter().any(|a| a.is_image())
        {
            self.build_multimodal_message(text, attachments)
                .await
        } else {
            // Fallback: add attachment descriptions to text
            let mut full_text = text.to_string();
            for attachment in attachments {
                full_text = format!("{}\n{}", full_text, attachment.describe());
            }
            ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                content: full_text.into(),
                name: None,
            })
        }
    }

    /// Build a multimodal message with text + images.
    async fn build_multimodal_message(
        &self,
        text: &str,
        attachments: &[MediaAttachment],
    ) -> ChatCompletionRequestMessage {
        let mut parts = vec![ChatCompletionRequestUserMessageContentPart::Text(
            ChatCompletionRequestMessageContentPartText {
                text: text.to_string(),
            },
        )];

        // Add images
        for attachment in attachments {
            if attachment.is_image() {
                // Download and encode as base64
                match media::download_and_encode_base64(
                    &attachment.url,
                    &attachment.content_type,
                )
                .await
                {
                    Ok(base64_url) => {
                        parts.push(ChatCompletionRequestUserMessageContentPart::ImageUrl(
                            ChatCompletionRequestMessageContentPartImage {
                                image_url: ImageUrl {
                                    url: base64_url,
                                    detail: None,
                                },
                            },
                        ));
                    }
                    Err(e) => {
                        tracing::warn!("Failed to encode image: {}", e);
                        // Fallback to description
                        let desc = attachment.describe();
                        let current_text = match &mut parts[0] {
                            ChatCompletionRequestUserMessageContentPart::Text(t) => &mut t.text,
                            _ => unreachable!(),
                        };
                        *current_text = format!("{}\n{}", current_text, desc);
                    }
                }
            } else {
                // Non-image: add description
                let desc = attachment.describe();
                let current_text = match &mut parts[0] {
                    ChatCompletionRequestUserMessageContentPart::Text(t) => &mut t.text,
                    _ => unreachable!(),
                };
                *current_text = format!("{}\n{}", current_text, desc);
            }
        }

        ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
            content: ChatCompletionRequestUserMessageContent::Array(parts),
            name: None,
        })
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
                Ok(tool_call_count) => {
                    if tool_call_count == 0 {
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
// Summary generation (free function to avoid borrow conflicts)
// ============================================================================

/// Generate a summary of removed conversation messages using the LLM.
/// Uses a non-streaming call to produce a 1-2 sentence summary.
/// Falls back to a static message on failure.
async fn generate_summary(
    llm_client: &LlmClient,
    removed_messages: &[ChatCompletionRequestMessage],
) -> String {
    let transcript = crate::context::format_removed_messages_as_transcript(removed_messages);

    let system_prompt = "Summarize the following conversation transcript in 1-2 concise sentences. Focus on: what the user asked for, what actions were taken, and the outcomes. Be brief and factual.";

    let messages = vec![
        ChatCompletionRequestMessage::System(
            ChatCompletionRequestSystemMessage {
                content: system_prompt.into(),
                name: None,
            }
        ),
        ChatCompletionRequestMessage::User(
            ChatCompletionRequestUserMessage {
                content: format!("Conversation transcript:\n\n{}", transcript).into(),
                name: None,
            }
        ),
    ];

    match llm_client.chat(messages, None).await {
        Ok(response) => {
            if let Some(choice) = response.choices.first() {
                if let Some(content) = &choice.message.content {
                    let summary = content.trim().to_string();
                    if !summary.is_empty() {
                        tracing::info!("Generated summary: {}", summary);
                        return summary;
                    }
                }
            }
            tracing::warn!("Summary generation returned empty response, using fallback");
            "Conversation history compressed.".to_string()
        }
        Err(e) => {
            tracing::warn!("Summary generation failed: {}, using fallback", e);
            "Conversation history compressed.".to_string()
        }
    }
}

// ============================================================================
// Helper types
// ============================================================================

/// Accumulates streaming tool call chunks.
#[derive(Debug)]
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
        tracing::debug!("Converting accumulator to tool call: {:?}", self);

        let id = self.id?;
        let name = self.name?;

        tracing::debug!("Tool call assembled: id='{}', name='{}', args='{}'", id, name, self.arguments);

        Some(ChatCompletionMessageToolCall {
            id,
            function: FunctionCall {
                name,
                arguments: self.arguments,
            },
        })
    }
}
