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
use futures_util::StreamExt;
use robit_ai::config::ContextConfig;
use robit_ai::LlmClient;
use std::any::Any;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::context::{ContextManager, TruncationAction, TruncationResult};
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
    /// Pending truncation result that needs compression (handled at start of run loop).
    pending_truncation: Option<(SessionId, crate::context::TruncationResult)>,
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
            pending_truncation: None,
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

        let pending_truncation = if truncation_result.needs_compression {
            Some((session_id.clone(), truncation_result))
        } else {
            None
        };

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
            pending_truncation,
        }
    }

    /// Run the agent's main event loop. Takes ownership of the message receiver.
    /// Returns when the channel is closed or user types /exit.
    pub async fn run(mut self, mut message_rx: mpsc::Receiver<FrontendMessage>) {
        tracing::info!("Agent started, session: {}", self.default_session_id);

        // Handle pending compression from with_history initialization.
        // May need multiple compression rounds for long histories.
        if self.pending_truncation.is_some() {
            tracing::info!("=== Starting pending compression processing ===");
            let session_id = self.default_session_id.clone();
            let mut iterations = 0;
            const MAX_COMPRESSION_ITERATIONS: usize = 20;

            loop {
                // Take one pending result, if any
                let pending = self.pending_truncation.take();
                let result = match pending {
                    Some((_, r)) => r,
                    None => break,
                };

                iterations += 1;
                if iterations > MAX_COMPRESSION_ITERATIONS {
                    tracing::warn!("Reached max compression iterations ({}), stopping", MAX_COMPRESSION_ITERATIONS);
                    break;
                }

                tracing::info!("Compression iteration {}: action={:?}, removed_rounds={}, removed_msgs={}",
                    iterations, result.action, result.rounds_removed, result.messages_removed);

                // Apply the compression result (generate summary / merge)
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    apply_compression_result(&self.llm_client, &mut session.history, &result).await;
                }

                // Check if more compression is needed
                let needs_more = if let Some(session) = self.sessions.get(&session_id) {
                    let estimated = crate::context::estimate_messages_tokens_with_margin(
                        &session.history,
                        self.context_manager.token_safety_margin,
                    );
                    estimated > self.context_manager.truncation_threshold()
                } else {
                    false
                };

                if !needs_more {
                    tracing::info!("Context below threshold after {} compression iterations", iterations);
                    break;
                }

                // Do another round of truncation
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    let next_result = self.context_manager.maybe_truncate(&mut session.history);
                    if next_result.needs_compression {
                        self.pending_truncation = Some((session_id.clone(), next_result));
                    } else if next_result.messages_removed > 0 {
                        // Truncation happened but no compression needed (e.g. discard)
                        tracing::info!("Truncation without compression: {} messages removed", next_result.messages_removed);
                        // Continue the loop to check if still over threshold
                        self.pending_truncation = Some((session_id.clone(), next_result));
                    } else {
                        break;
                    }
                }
            }

            tracing::info!("=== Compression processing finished ({} iterations) ===", iterations);
        } else {
            tracing::info!("No pending compression needed");
        }

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
        // First get the working_dir before the first mutable borrow
        let working_dir = {
            let session = self
                .sessions
                .get(session_id)
                .ok_or_else(|| AgentError::InternalError("Session not found".to_string()))?;
            session.working_dir.clone()
        };

        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| AgentError::InternalError("Session not found".to_string()))?;

        // Truncate context if needed
        let truncation_result = self.context_manager.maybe_truncate(&mut session.history);

        // Handle compression: generate actual summary / merge via LLM
        if truncation_result.needs_compression {
            apply_compression_result(&self.llm_client, &mut session.history, &truncation_result).await;

            tracing::info!(
                "Compression completed: action={:?}, removed_rounds={}",
                truncation_result.action, truncation_result.rounds_removed
            );
        } else if truncation_result.messages_removed > 0 {
            tracing::info!(
                "Context truncated without compression: {} messages removed",
                truncation_result.messages_removed
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
                        let acc = tool_call_chunks
                            .entry(tc.index as usize)
                            .or_insert_with(ToolCallAccumulator::new);

                        if let Some(id) = &tc.id {
                            // 只有当id非空时才更新
                            if !id.is_empty() {
                                acc.id = Some(id.clone());
                            }
                        }
                        if let Some(function) = &tc.function {
                            if let Some(name) = &function.name {
                                // 只有当name非空时才更新
                                if !name.is_empty() {
                                    acc.name = Some(name.clone());
                                }
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
                "Tool call [{}]: id='{}', name='{}', arguments={}",
                i,
                tc.id,
                tc.function.name,
                truncate_for_log(&tc.function.arguments, 80)
            );
        }

        // Add assistant message to history
        let content = if full_text.is_empty() {
            None
        } else {
            Some(full_text.clone().into())
        };
        let tool_calls = if assembled_tool_calls.is_empty() {
            None
        } else {
            Some(
                assembled_tool_calls
                    .clone()
                    .into_iter()
                    .map(ChatCompletionMessageToolCalls::Function)
                    .collect(),
            )
        };

        // Ensure we don't add an invalid assistant message to history
        if content.is_some() || tool_calls.is_some() {
            let assistant_msg = ChatCompletionRequestMessage::Assistant(
                ChatCompletionRequestAssistantMessage {
                    content,
                    name: None,
                    tool_calls,
                    refusal: None,
                    audio: None,
                    #[allow(deprecated)]
                    function_call: None,
                }
                .into(),
            );

            session.history.push(assistant_msg);
        } else {
            tracing::warn!("Not adding empty assistant message to history (no content and no tool calls)");
        }

        // If no tool calls, turn is complete
        if assembled_tool_calls.is_empty() {
            return Ok(0);
        }

        // Execute each tool call
        tracing::trace!(
            "[tool] beginning execution of {} tool call(s) this step",
            assembled_tool_calls.len()
        );
        for (tc_idx, tc) in assembled_tool_calls.iter().enumerate() {
            tracing::info!(
                "About to execute tool [{}/{}]: id='{}', name='{}'",
                tc_idx + 1,
                assembled_tool_calls.len(),
                tc.id,
                tc.function.name
            );
            tracing::trace!(
                "[tool] tool_call_id='{}', name='{}', arguments={}",
                tc.id,
                tc.function.name,
                truncate_for_log(&tc.function.arguments, 80)
            );

            let tc_info = ToolCallInfo {
                id: tc.id.clone(),
                name: tc.function.name.clone(),
                arguments: tc.function.arguments.clone(),
            };

            // Notify frontend. Capture the result instead of `let _ =` so a
            // failed delivery (closed/full channel, platform send error) is
            // surfaced — a silent failure here is exactly the "no feedback"
            // symptom we want to catch.
            match self
                .frontend
                .on_event(AgentEvent::ToolCallRequested {
                    tool_call_id: tc_info.id.clone(),
                    name: tc_info.name.clone(),
                    arguments: tc_info.arguments.clone(),
                })
                .await
            {
                Ok(()) => tracing::trace!(
                    "[tool] ToolCallRequested delivered to frontend: tool_call_id='{}', name='{}'",
                    tc_info.id,
                    tc_info.name
                ),
                Err(e) => tracing::warn!(
                    "[tool] ToolCallRequested delivery FAILED (user feedback may be lost): tool_call_id='{}', name='{}', error={}",
                    tc_info.id,
                    tc_info.name,
                    e
                ),
            }

            // Check confirmation
            let requires_confirm = self.tools.requires_confirmation(&tc.function.name);
            let approved = if requires_confirm && !self.auto_approve {
                tracing::trace!(
                    "[tool] requesting user confirmation: tool_call_id='{}', name='{}'",
                    tc_info.id,
                    tc_info.name
                );
                match self.frontend.request_tool_confirmation(&tc_info).await {
                    Ok(approved) => {
                        tracing::trace!(
                            "[tool] confirmation response: tool_call_id='{}', name='{}', approved={}",
                            tc_info.id,
                            tc_info.name,
                            approved
                        );
                        approved
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[tool] confirmation request failed: tool_call_id='{}', name='{}', error={}",
                            tc_info.id,
                            tc_info.name,
                            e
                        );
                        return Err(e);
                    }
                }
            } else {
                tracing::trace!(
                    "[tool] skipping confirmation (requires_confirm={}, auto_approve={})",
                    requires_confirm,
                    self.auto_approve
                );
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
                    supports_images: self.llm_client.supports_images(),
                };

                tracing::trace!(
                    "[tool] dispatching to registry: tool_call_id='{}', name='{}'",
                    tc_info.id,
                    tc_info.name
                );
                let result = self.tools.execute(&tc.function.name, args, &ctx).await;
                tracing::trace!(
                    "[tool] execution returned: tool_call_id='{}', name='{}', is_error={}, content_len={}",
                    tc_info.id,
                    tc_info.name,
                    result.is_error,
                    result.content.len()
                );
                result
            } else {
                tracing::trace!(
                    "[tool] tool call rejected by user: tool_call_id='{}', name='{}'",
                    tc_info.id,
                    tc_info.name
                );
                ToolResult::error("User rejected this tool call")
            };

            // Truncate output
            let raw_len = result.content.len();
            let truncated_result = ToolResult {
                content: self.context_manager.truncate_tool_output(&result.content),
                is_error: result.is_error,
                images: result.images.clone(),
            };
            if truncated_result.content.len() != raw_len {
                tracing::trace!(
                    "[tool] output truncated: tool_call_id='{}', name='{}', raw_len={}, truncated_len={}",
                    tc_info.id,
                    tc_info.name,
                    raw_len,
                    truncated_result.content.len()
                );
            }

            // Notify frontend of result. Same rationale as above: capture
            // delivery errors so a lost ToolCallResult is never silent.
            match self
                .frontend
                .on_event(AgentEvent::ToolCallResult {
                    tool_call_id: tc.id.clone(),
                    result: truncated_result.clone(),
                })
                .await
            {
                Ok(()) => tracing::trace!(
                    "[tool] ToolCallResult delivered to frontend: tool_call_id='{}', name='{}'",
                    tc_info.id,
                    tc_info.name
                ),
                Err(e) => tracing::warn!(
                    "[tool] ToolCallResult delivery FAILED (user feedback may be lost): tool_call_id='{}', name='{}', error={}",
                    tc_info.id,
                    tc_info.name,
                    e
                ),
            }

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

            // Inject images from the tool result as a multimodal user message.
            // OpenAI protocol restricts tool message content to text, so images
            // cannot travel in the tool result itself; we inject them in a
            // separate user message right after the tool result.
            if !truncated_result.images.is_empty() && self.llm_client.supports_images() {
                let mut parts = vec![ChatCompletionRequestUserMessageContentPart::Text(
                    ChatCompletionRequestMessageContentPartText {
                        text: format!(
                            "[工具返回的图片] {}",
                            truncated_result
                                .images
                                .iter()
                                .map(|i| i.label.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    },
                )];
                for img in &truncated_result.images {
                    parts.push(ChatCompletionRequestUserMessageContentPart::ImageUrl(
                        ChatCompletionRequestMessageContentPartImage {
                            image_url: ImageUrl {
                                url: img.data_url.clone(),
                                detail: None,
                            },
                        },
                    ));
                }
                let image_msg = ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                    content: ChatCompletionRequestUserMessageContent::Array(parts),
                    name: None,
                });
                session.history.push(image_msg);
            }

            tracing::trace!(
                "[tool] tool result appended to history: tool_call_id='{}', name='{}', history_len={}",
                tc_info.id,
                tc_info.name,
                session.history.len()
            );
        }

        tracing::trace!(
            "[tool] all {} tool call(s) executed this step",
            assembled_tool_calls.len()
        );
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
/// Apply a truncation result to the session history.
/// For NewSegment: generates a summary from removed messages and replaces the placeholder.
/// For MergeSegments: merges existing summary segments and replaces the placeholder.
/// For TruncateOnly: no-op.
async fn apply_compression_result(
    llm_client: &LlmClient,
    history: &mut [ChatCompletionRequestMessage],
    result: &TruncationResult,
) {
    if !result.needs_compression {
        return;
    }

    let pos = result.insert_position;
    if pos >= history.len() {
        tracing::warn!("Insert position {} out of bounds (history len: {})", pos, history.len());
        return;
    }

    let (content, name) = match &result.action {
        TruncationAction::NewSegment => {
            let summary = generate_summary(llm_client, &result.removed_messages).await;
            (
                format!("[Summary: {}]", summary),
                "summary_segment".to_string(),
            )
        }
        TruncationAction::MergeSegments { summaries, .. } => {
            let merged = merge_summaries(llm_client, summaries).await;
            // Determine new merge level from the placeholder's name
            let current_level = crate::context::get_merge_level(&history[pos]);
            let name = if current_level == 0 {
                "summary_segment".to_string()
            } else {
                format!("summary_segment_m{}", current_level)
            };
            (format!("[Summary: {}]", merged), name)
        }
        TruncationAction::TruncateOnly => return,
    };

    tracing::info!("Compression applied at position {}: {}", pos, name);

    history[pos] = ChatCompletionRequestMessage::User(
        ChatCompletionRequestUserMessage {
            content: content.into(),
            name: Some(name),
        }
    );
}

/// Generate a short summary from removed full conversation rounds.
async fn generate_summary(
    llm_client: &LlmClient,
    removed_messages: &[ChatCompletionRequestMessage],
) -> String {
    tracing::debug!("Generating summary: removed_messages count = {}", removed_messages.len());
    let transcript = crate::context::format_removed_messages_as_transcript(removed_messages);
    tracing::debug!("Formatted transcript length: {} characters", transcript.len());

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

    tracing::info!("Calling LLM to generate summary...");
    match llm_client.chat(messages, None).await {
        Ok(response) => {
            tracing::info!("LLM responded successfully for summary generation");
            tracing::debug!("Number of choices in response: {}", response.choices.len());
            if let Some(choice) = response.choices.first() {
                tracing::debug!("Choice index: 0, has content: {}", choice.message.content.is_some());
                if let Some(content) = &choice.message.content {
                    let summary = content.trim().to_string();
                    if !summary.is_empty() {
                        tracing::info!("Successfully generated summary (length: {})", summary.len());
                        return summary;
                    }
                }
            }
            tracing::warn!("Summary generation returned empty response, using fallback");
            "Conversation history compressed.".to_string()
        }
        Err(e) => {
            tracing::error!("Summary generation failed with error: {}, using fallback", e);
            "Conversation history compressed.".to_string()
        }
    }
}

/// Merge multiple existing summary segments into one coherent summary.
async fn merge_summaries(
    llm_client: &LlmClient,
    summaries: &[String],
) -> String {
    tracing::info!("Merging {} summary segments...", summaries.len());

    let numbered: Vec<String> = summaries
        .iter()
        .enumerate()
        .map(|(i, s)| format!("[{}] {}", i + 1, s))
        .collect();
    let joined = numbered.join("\n\n");

    let system_prompt = "You are given multiple conversation summaries from different time periods, ordered from oldest to newest. Merge them into a single concise summary (2-3 sentences) that preserves all key information.

Key points to preserve:
- User goals and requests
- Important decisions made
- Technical context (file paths, APIs, architectures)
- Major outcomes and conclusions

Do not simply concatenate — synthesize into a coherent narrative.";

    let messages = vec![
        ChatCompletionRequestMessage::System(
            ChatCompletionRequestSystemMessage {
                content: system_prompt.into(),
                name: None,
            }
        ),
        ChatCompletionRequestMessage::User(
            ChatCompletionRequestUserMessage {
                content: format!("Summaries to merge:\n\n{}", joined).into(),
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
                        tracing::info!("Successfully merged {} summaries (length: {})", summaries.len(), summary.len());
                        return summary;
                    }
                }
            }
            tracing::warn!("Summary merge returned empty response, using fallback");
            "Multiple earlier conversation segments merged.".to_string()
        }
        Err(e) => {
            tracing::error!("Summary merge failed with error: {}, using fallback", e);
            "Multiple earlier conversation segments merged.".to_string()
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
        let id = self.id?;
        let name = self.name?;

        tracing::debug!(
            "Tool call assembled: id='{}', name='{}', args={}",
            id,
            name,
            truncate_for_log(&self.arguments, 80)
        );

        Some(ChatCompletionMessageToolCall {
            id,
            function: FunctionCall {
                name,
                arguments: self.arguments,
            },
        })
    }
}

/// Truncate a string to at most `max_chars` characters for log output,
/// appending a length note when truncated. Counts by `char` to avoid
/// splitting multi-byte UTF-8 sequences (safe for CJK text).
fn truncate_for_log(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else {
        let preview: String = s.chars().take(max_chars).collect();
        format!("{}... ({} chars total)", preview, char_count)
    }
}
