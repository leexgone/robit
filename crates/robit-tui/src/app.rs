//! App state — conversation model and event handling.

use tokio::sync::mpsc;

use robit_agent::{AgentEvent, FrontendMessage, ToolRegistry};

use crate::input::InputEditor;

// ============================================================================
// Conversation model
// ============================================================================

/// A single entry in the conversation display.
#[derive(Debug)]
pub enum ConversationEntry {
    UserMessage(String),
    AssistantText(String),
    ToolCard {
        tool_call_id: String,
        name: String,
        arguments: String,
        status: ToolStatus,
    },
    Error(String),
    SystemNotice(String),
}

/// Tool call display status.
#[derive(Debug)]
#[allow(dead_code)]
pub enum ToolStatus {
    Pending,
    Running,
    Success(String),
    Failed(String),
    Rejected,
    AwaitingConfirmation,
}

// ============================================================================
// Input mode
// ============================================================================

/// The current input mode of the TUI.
pub enum InputMode {
    /// Normal text input.
    Normal,
    /// Waiting for Y/N confirmation.
    Confirmation {
        _tool_call_id: String,
        responder: Option<tokio::sync::oneshot::Sender<bool>>,
    },
}

// ============================================================================
// Status info
// ============================================================================

/// Information displayed in the status bar.
pub struct StatusInfo {
    pub model: String,
    pub tools_enabled: usize,
    pub tools_total: usize,
}

// ============================================================================
// App
// ============================================================================

/// Main application state.
pub struct App {
    pub conversation: Vec<ConversationEntry>,
    pub current_assistant_text: String,
    pub input: InputEditor,
    pub input_mode: InputMode,
    pub scroll_offset: usize,
    pub auto_scroll: bool,
    pub status: StatusInfo,
    pub is_agent_busy: bool,
    pub should_quit: bool,
}

impl App {
    pub fn new(model: String, tools: &ToolRegistry) -> Self {
        let tool_names = tools.tool_names();
        Self {
            conversation: Vec::new(),
            current_assistant_text: String::new(),
            input: InputEditor::new(),
            input_mode: InputMode::Normal,
            scroll_offset: 0,
            auto_scroll: true,
            status: StatusInfo {
                model,
                tools_enabled: tool_names.len(),
                tools_total: tool_names.len(),
            },
            is_agent_busy: false,
            should_quit: false,
        }
    }

    /// Flush the streaming assistant text into a conversation entry.
    pub fn commit_assistant_text(&mut self) {
        if !self.current_assistant_text.is_empty() {
            let text = std::mem::take(&mut self.current_assistant_text);
            self.conversation.push(ConversationEntry::AssistantText(text));
        }
    }

    /// Handle an incoming agent event, updating the UI state.
    pub fn handle_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::TextDelta(text) => {
                self.is_agent_busy = true;
                self.current_assistant_text.push_str(&text);
                self.auto_scroll = true;
            }
            AgentEvent::ToolCallRequested {
                tool_call_id,
                name,
                arguments,
            } => {
                self.is_agent_busy = true;
                self.commit_assistant_text();
                self.conversation.push(ConversationEntry::ToolCard {
                    tool_call_id,
                    name,
                    arguments,
                    status: ToolStatus::Pending,
                });
                self.auto_scroll = true;
            }
            AgentEvent::ToolCallResult {
                tool_call_id,
                result,
            } => {
                self.update_tool_status(&tool_call_id, result);
                self.auto_scroll = true;
            }
            AgentEvent::TurnComplete => {
                self.commit_assistant_text();
                self.is_agent_busy = false;
            }
            AgentEvent::Error(e) => {
                self.commit_assistant_text();
                self.conversation
                    .push(ConversationEntry::Error(format!("{}", e)));
                self.is_agent_busy = false;
                self.auto_scroll = true;
            }
        }
    }

    fn update_tool_status(&mut self, tool_call_id: &str, result: robit_agent::ToolResult) {
        for entry in self.conversation.iter_mut().rev() {
            if let ConversationEntry::ToolCard {
                tool_call_id: id,
                status,
                ..
            } = entry
            {
                if id == tool_call_id {
                    *status = if result.is_error {
                        ToolStatus::Failed(result.content)
                    } else {
                        ToolStatus::Success(result.content)
                    };
                    return;
                }
            }
        }
    }

    /// Process user input text (slash commands or send to agent).
    pub fn handle_user_input(
        &mut self,
        text: String,
        message_tx: &mpsc::Sender<FrontendMessage>,
    ) {
        if text.starts_with('/') {
            self.handle_slash_command(&text, message_tx);
        } else {
            self.conversation
                .push(ConversationEntry::UserMessage(text.clone()));
            let tx = message_tx.clone();
            tokio::spawn(async move {
                let _ = tx.send(FrontendMessage::UserInput(text)).await;
            });
            self.auto_scroll = true;
        }
    }

    fn handle_slash_command(
        &mut self,
        cmd: &str,
        message_tx: &mpsc::Sender<FrontendMessage>,
    ) {
        let parts: Vec<&str> = cmd.splitn(2, ' ').collect();
        match parts[0] {
            "/exit" | "/quit" => {
                self.should_quit = true;
            }
            "/clear" => {
                self.conversation.clear();
                self.current_assistant_text.clear();
                let tx = message_tx.clone();
                tokio::spawn(async move {
                    let _ = tx.send(FrontendMessage::UserInput("/clear".to_string())).await;
                });
                self.conversation
                    .push(ConversationEntry::SystemNotice("对话历史已清空".to_string()));
            }
            "/model" => {
                let msg = format!("当前模型: {}", self.status.model);
                self.conversation
                    .push(ConversationEntry::SystemNotice(msg));
            }
            "/tools" => {
                let msg = format!(
                    "已启用工具: {} 个",
                    self.status.tools_enabled
                );
                self.conversation
                    .push(ConversationEntry::SystemNotice(msg));
            }
            _ => {
                self.conversation.push(ConversationEntry::Error(format!(
                    "未知命令: {}",
                    parts[0]
                )));
            }
        }
    }
}
