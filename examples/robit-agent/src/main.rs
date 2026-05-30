//! robit-agent-cli: stdin/stdout validation frontend for Phase 2.
//!
//! Usage: cargo run -p robit-agent-cli
//!
//! Architecture: ALL stdout/stdin I/O happens in the main task.
//! The Agent's Frontend pushes events through a channel — never prints directly.
//! This avoids concurrent stdout writes between the agent task and the main task.

use async_trait::async_trait;
use robit_agent::tool::bash::BashTool;
use robit_agent::tool::read::ReadTool;
use robit_agent::{Agent, AgentEvent, Frontend, FrontendMessage, ToolCallInfo, ToolRegistry};
use robit_ai::config::{load_env, load_llm_config, load_settings};
use robit_ai::LlmClient;
use std::io::Write;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("robit_agent=info".parse()?),
        )
        .init();

    load_env();
    let llm_config = load_llm_config()?;
    let settings = load_settings()?;

    let client = Arc::new(LlmClient::from_config(&llm_config, &settings)?);
    println!(
        "Robit Agent | provider: {} | model: {}",
        client.provider(),
        client.model()
    );
    println!("输入消息开始对话，输入 exit 或 /exit 退出\n");

    let context_config = settings.context.as_ref();
    let context_window = {
        let resolved = robit_ai::config::resolve_model(&llm_config, &settings)?;
        llm_config
            .providers
            .get(&resolved.provider_key)
            .and_then(|p| p.models.iter().find(|m| m.id == resolved.model_id))
            .and_then(|m| m.context_window)
    };

    let working_dir = std::env::current_dir()?;

    let mut tools = ToolRegistry::new();
    let max_lines = context_config.and_then(|c| c.max_output_lines).unwrap_or(500);
    let max_bytes = context_config
        .and_then(|c| c.max_output_bytes)
        .unwrap_or(51200);
    tools.register(ReadTool::new(max_lines, max_bytes));
    tools.register(BashTool::new(max_bytes));
    let tools = Arc::new(tools);

    // Channels:
    //   event channel:   Agent → main task (render events to stdout)
    //   message channel: main task → Agent (user input)
    //   confirm channel: Agent → main task (confirmation prompts)
    let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(64);
    let (message_tx, message_rx) = mpsc::channel::<FrontendMessage>(16);
    let (confirm_tx, mut confirm_rx) =
        mpsc::channel::<(String, tokio::sync::oneshot::Sender<bool>)>(4);

    let frontend = Arc::new(CliFrontend {
        event_tx,
        confirm_tx,
    });

    let agent = Agent::new(
        client,
        tools,
        frontend,
        context_config,
        context_window,
        working_dir,
    );

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let agent_handle = tokio::spawn(async move {
            agent.run(message_rx).await;
        });

        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin).lines();

        // Pending tool confirmation
        let mut pending_confirm: Option<tokio::sync::oneshot::Sender<bool>> = None;

        // Show initial prompt
        print!("> ");
        let _ = std::io::stdout().flush();

        loop {
            tokio::select! {
                // Events from Agent — main task is the sole stdout writer
                maybe_event = event_rx.recv() => {
                    if let Some(event) = maybe_event {
                        let done = matches!(event, AgentEvent::TurnComplete);
                        render_event(&event);
                        if done {
                            print!("> ");
                            let _ = std::io::stdout().flush();
                        }
                    } else {
                        break; // event_tx dropped (agent gone)
                    }
                }

                // Confirmation prompts from Agent
                maybe_req = confirm_rx.recv(), if pending_confirm.is_none() => {
                    if let Some((prompt, tx)) = maybe_req {
                        print!("{}", prompt);
                        let _ = std::io::stdout().flush();
                        pending_confirm = Some(tx);
                    }
                }

                // Stdin input
                result = reader.next_line() => {
                    match result {
                        Ok(Some(line)) => {
                            let trimmed = line.trim().to_string();

                            // Route to confirmation if pending
                            if let Some(tx) = pending_confirm.take() {
                                let answer = trimmed.to_lowercase();
                                let approved = answer.is_empty()
                                    || answer == "y"
                                    || answer == "yes";
                                let _ = tx.send(approved);
                                continue;
                            }

                            if trimmed.is_empty() {
                                continue;
                            }

                            // Exit commands — handle locally for clean shutdown
                            if matches!(trimmed.as_str(), "exit" | "/exit" | "quit" | "/quit") {
                                break;
                            }

                            if trimmed == "/cancel" {
                                let _ = message_tx.send(FrontendMessage::Cancel).await;
                                continue;
                            }

                            if message_tx.send(FrontendMessage::UserInput(trimmed)).await.is_err() {
                                break;
                            }
                            // Agent will now emit events via event_rx
                        }
                        Ok(None) | Err(_) => break,
                    }
                }
            }
        }

        agent_handle.abort();
        println!("\n再见！");
    });

    Ok(())
}

/// Render an agent event to stdout. Called only from the main task.
fn render_event(event: &AgentEvent) {
    match event {
        AgentEvent::TextDelta(text) => {
            print!("{}", text);
            let _ = std::io::stdout().flush();
        }
        AgentEvent::ToolCallRequested {
            name, arguments, ..
        } => {
            println!("\n┌─ 🔧 {} ─────────────────────────", name);
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(arguments) {
                if let Some(obj) = parsed.as_object() {
                    for (k, v) in obj {
                        let display_val = match v {
                            serde_json::Value::String(s) => {
                                if s.len() > 200 {
                                    format!("{}...", &s[..200])
                                } else {
                                    s.clone()
                                }
                            }
                            other => other.to_string(),
                        };
                        println!("│ {}: {}", k, display_val);
                    }
                }
            } else {
                println!("│ {}", arguments);
            }
            println!("└────────────────────────────────────");
        }
        AgentEvent::ToolCallResult { result, .. } => {
            if result.is_error {
                println!("✗ 失败:\n{}", result.content);
            } else {
                let display = if result.content.len() > 1000 {
                    format!(
                        "{}\n... (输出已截断，共 {} bytes)",
                        &result.content[..1000],
                        result.content.len()
                    )
                } else {
                    result.content.clone()
                };
                println!("✓ 结果:\n{}", display);
            }
        }
        AgentEvent::TurnComplete => {
            println!();
            let _ = std::io::stdout().flush();
        }
        AgentEvent::Error(e) => {
            eprintln!("\n[错误] {}", e);
        }
    }
}

// ============================================================================
// CLI Frontend — forwards everything through channels, never writes stdout
// ============================================================================

struct CliFrontend {
    event_tx: mpsc::Sender<AgentEvent>,
    confirm_tx: mpsc::Sender<(String, tokio::sync::oneshot::Sender<bool>)>,
}

#[async_trait]
impl Frontend for CliFrontend {
    async fn on_event(&self, event: AgentEvent) -> robit_agent::error::Result<()> {
        // Forward to main task — it's the sole stdout writer
        let _ = self.event_tx.send(event).await;
        Ok(())
    }

    async fn request_tool_confirmation(
        &self,
        info: &ToolCallInfo,
    ) -> robit_agent::error::Result<bool> {
        let prompt = format!("\n⚠️  工具 '{}' 需要确认执行 [Y/n]: ", info.name);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.confirm_tx.send((prompt, tx)).await;
        match rx.await {
            Ok(approved) => Ok(approved),
            Err(_) => Ok(false),
        }
    }
}
