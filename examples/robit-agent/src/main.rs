//! robit-agent-cli: stdin/stdout validation frontend for Phase 2.
//!
//! Usage: cargo run -p robit-agent-cli
//!
//! Architecture: ALL stdout/stdin I/O happens in the main task.
//! The Agent's Frontend pushes events through a channel — never prints directly.
//! This avoids concurrent stdout writes between the agent task and the main task.

use async_trait::async_trait;
use clap::Parser;
use robit_agent::skill::SkillRegistry;
use robit_agent::tool::bash::BashTool;
use robit_agent::tool::load_skill::LoadSkillTool;
use robit_agent::tool::read::ReadTool;
use robit_agent::{Agent, AgentEvent, Frontend, FrontendMessage, ToolCallInfo, ToolRegistry};
use robit_ai::config::load_config;
use robit_ai::LlmClient;
use std::io::Write;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

#[derive(Debug, Parser)]
#[command(name = "robit-agent-cli")]
#[command(about = "AI Programming Agent with stdin/stdout frontend")]
struct Cli {
    /// 自动批准所有工具调用，跳过用户确认
    #[arg(long)]
    auto_approve: bool,

    /// Working directory for the agent
    #[arg(long, short = 'w')]
    workdir: Option<std::path::PathBuf>,
}

fn main() -> anyhow::Result<()> {
    // Parse CLI args first
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("robit_agent=info".parse()?),
        )
        .init();

    // Resolve working directory
    let working_dir = if let Some(ref workdir) = cli.workdir {
        if !workdir.exists() {
            anyhow::bail!("Working directory does not exist: {}", workdir.display());
        }
        if !workdir.is_dir() {
            anyhow::bail!("Path is not a directory: {}", workdir.display());
        }
        std::fs::canonicalize(workdir)?
    } else {
        std::env::current_dir()?
    };

    let config = load_config(cli.workdir.as_deref())?;

    // Determine auto_approve: CLI flag takes priority, then config, then default false
    let auto_approve = cli.auto_approve || config.app.as_ref().and_then(|a| a.auto_approve).unwrap_or(false);

    let client = Arc::new(LlmClient::from_config(&config, None)?);
    println!(
        "Robit Agent | profile: {} | model: {}",
        client.profile(),
        client.model()
    );
    println!("输入消息开始对话，输入 exit 或 /exit 退出\n");

    let context_config = config.app.as_ref().and_then(|a| a.context.as_ref());
    let context_window = client.resolved().context_window;

    // Load skills first (needed for LoadSkillTool)
    let global_skills_dir = dirs::home_dir().map(|h| h.join(".robit/skills"));
    let project_skills_dir = Some(working_dir.join(".robit/skills"));

    let (skills, skill_errors) = robit_agent::skill::loader::load_skills(
        global_skills_dir,
        project_skills_dir,
    );
    for err in &skill_errors {
        tracing::warn!("技能加载错误: {:?}", err);
    }

    let enabled_skills = config
        .app
        .as_ref()
        .and_then(|a| a.enabled_skills.as_ref());
    let filtered_skills: Vec<_> = match enabled_skills {
        Some(list) => skills
            .into_iter()
            .filter(|s| list.contains(&s.frontmatter.name))
            .collect(),
        None => skills,
    };

    // Create skill registry first (LoadSkillTool needs it)
    let base_tool_names = ["read", "bash"];
    let skill_registry = Arc::new(SkillRegistry::new(filtered_skills, &base_tool_names));

    // Create tools
    let mut tools = ToolRegistry::new();
    let max_lines = context_config.and_then(|c| c.max_output_lines).unwrap_or(500);
    let max_bytes = context_config
        .and_then(|c| c.max_output_bytes)
        .unwrap_or(51200);
    tools.register(ReadTool::new(max_lines, max_bytes));
    tools.register(BashTool::new(max_bytes));
    tools.register(LoadSkillTool::new(Arc::clone(&skill_registry)));
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
        skill_registry,
        frontend,
        context_config,
        context_window,
        working_dir,
        auto_approve,
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
        AgentEvent::SkillTriggered { name, description } => {
            println!("\n[技能] {} — {}", name, description);
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
