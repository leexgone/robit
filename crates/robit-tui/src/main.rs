//! robit — TUI frontend for the Robit AI programming agent.
//!
//! Usage: cargo run -p robit-tui
//!        or: robit (after install)

mod app;
mod input;
mod markdown;
mod tui_frontend;
mod ui;

use std::io;
use std::sync::Arc;

use clap::Parser;
use anyhow::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use robit_agent::skill::SkillRegistry;
use robit_agent::tool::bash::BashTool;
use robit_agent::tool::edit::EditTool;
use robit_agent::tool::find::FindTool;
use robit_agent::tool::grep::GrepTool;
use robit_agent::tool::load_skill::LoadSkillTool;
use robit_agent::tool::ls::LsTool;
use robit_agent::tool::read::ReadTool;
use robit_agent::tool::write::WriteTool;
use robit_agent::{Agent, AgentEvent, FrontendMessage, ToolRegistry};
use robit_ai::config::load_config;
use robit_ai::LlmClient;
use tokio::sync::mpsc;

use app::{App, InputMode};
use tui_frontend::{ConfirmRequest, TuiFrontend};

#[derive(Debug, Parser)]
#[command(name = "robit")]
#[command(about = "AI Programming Agent with TUI")]
struct Cli {
    /// 自动批准所有工具调用，跳过用户确认
    #[arg(long)]
    auto_approve: bool,

    /// Working directory for the agent
    #[arg(long, short = 'w')]
    workdir: Option<std::path::PathBuf>,
}

fn main() -> Result<()> {
    // Parse CLI args first
    let cli = Cli::parse();

    // Initialize tracing (logs go to file, not terminal)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("robit_tui=info".parse()?),
        )
        .with_writer(|| {
            // Discard log output in TUI mode (use RUST_LOG to enable file logging)
            io::sink()
        })
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
    let model = client.model().to_string();

    let context_config = config.app.as_ref().and_then(|a| a.context.as_ref());
    let context_window = client.resolved().context_window;

    // Load skills first (needed for LoadSkillTool)
    let global_skills_dir = dirs::home_dir().map(|h| h.join(".robit/skills"));
    let project_skills_dir = Some(working_dir.join(".robit/skills"));

    let (skills, skill_errors) = robit_agent::skill::loader::load_skills(
        global_skills_dir,
        project_skills_dir,
    );

    // Log skill load errors as warnings
    for err in &skill_errors {
        tracing::warn!("技能加载错误: {:?}", err);
    }

    // Apply enabled_skills config filter
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
    let base_tool_names = ["read", "bash", "write", "edit"];
    let skill_registry = Arc::new(SkillRegistry::new(filtered_skills, &base_tool_names));

    // Create tools (includes LoadSkillTool which needs skill_registry)
    let tools = Arc::new(create_tools(&config, Arc::clone(&skill_registry)));

    // Create channels
    let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(64);
    let (message_tx, message_rx) = mpsc::channel::<FrontendMessage>(16);
    let (confirm_tx, mut confirm_rx) = mpsc::channel::<ConfirmRequest>(4);

    let frontend = Arc::new(TuiFrontend {
        event_tx,
        confirm_tx,
    });

    let agent = Agent::new(
        client,
        Arc::clone(&tools),
        Arc::clone(&skill_registry),
        frontend,
        context_config,
        context_window,
        working_dir,
        auto_approve,
    );

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    stdout.execute(EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Panic hook: restore terminal on panic
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = io::stdout().execute(LeaveAlternateScreen);
        let _ = io::stdout().execute(DisableMouseCapture);
        original_hook(panic_info);
    }));

    let rt = tokio::runtime::Runtime::new()?;
    let result = rt.block_on(async {
        let agent_handle = tokio::spawn(async move {
            agent.run(message_rx).await;
        });

        let mut app = App::new(model, &tools, Arc::clone(&skill_registry));
        app.status.tools_enabled = tools.tool_names().len();
        app.status.tools_total = tools.tool_names().len();

        run_event_loop(
            &mut terminal,
            &mut app,
            &mut event_rx,
            &mut confirm_rx,
            &message_tx,
            agent_handle,
        )
        .await
    });

    // Restore terminal
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    io::stdout().execute(DisableMouseCapture)?;

    result
}

fn create_tools(config: &robit_ai::config::RobitConfig, skills: Arc<SkillRegistry>) -> ToolRegistry {
    let mut tools = ToolRegistry::new();
    let context_config = config.app.as_ref().and_then(|a| a.context.as_ref());
    let max_lines = context_config.and_then(|c| c.max_output_lines).unwrap_or(500);
    let max_bytes = context_config
        .and_then(|c| c.max_output_bytes)
        .unwrap_or(51200);
    tools.register(ReadTool::new(max_lines, max_bytes));
    tools.register(BashTool::new(max_bytes));
    tools.register(WriteTool::new());
    tools.register(EditTool::new());
    tools.register(LoadSkillTool::new(skills));
    tools.register(LsTool::new());
    tools.register(FindTool::new(max_bytes));
    tools.register(GrepTool::new(max_lines, max_bytes));
    tools
}

async fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    event_rx: &mut mpsc::Receiver<AgentEvent>,
    confirm_rx: &mut mpsc::Receiver<ConfirmRequest>,
    message_tx: &mpsc::Sender<FrontendMessage>,
    agent_handle: tokio::task::JoinHandle<()>,
) -> Result<()> {
    let mut event_stream = EventStream::new();
    let tick_rate = std::time::Duration::from_millis(100);
    let mut tick_interval = tokio::time::interval(tick_rate);

    terminal.draw(|f| ui::draw(f, app))?;

    loop {
        tokio::select! {
            // Crossterm events (keyboard, resize)
            maybe_event = event_stream.next() => {
                if let Some(Ok(event)) = maybe_event {
                    handle_crossterm_event(app, event, message_tx).await;
                }
            }

            // Agent events
            maybe_event = event_rx.recv() => {
                match maybe_event {
                    Some(event) => {
                        app.handle_agent_event(event);
                    }
                    None => {
                        app.should_quit = true;
                    }
                }
            }

            // Confirmation requests from Agent
            maybe_req = confirm_rx.recv() => {
                if let Some(req) = maybe_req {
                    set_tool_awaiting(app, &req.tool_info.id);
                    app.input_mode = InputMode::Confirmation {
                        _tool_call_id: req.tool_info.id,
                        responder: Some(req.responder),
                    };
                }
            }

            // Tick for redraw
            _ = tick_interval.tick() => {}
        }

        // Redraw
        terminal.draw(|f| ui::draw(f, app))?;

        if app.should_quit {
            agent_handle.abort();
            break;
        }
    }

    Ok(())
}

async fn handle_crossterm_event(
    app: &mut App,
    event: Event,
    message_tx: &mpsc::Sender<FrontendMessage>,
) {
    match event {
        Event::Key(key) => {
            // Only handle key press events — Windows sends Press + Release,
            // which causes duplicate characters (especially with IME input).
            if key.kind != KeyEventKind::Press {
                return;
            }

            // Check for pending confirmation
            if let InputMode::Confirmation { responder, .. } = &mut app.input_mode {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        if let Some(tx) = responder.take() {
                            let _ = tx.send(true);
                        }
                        // Update tool card status
                        update_last_awaiting_tool(app, true);
                        app.input_mode = InputMode::Normal;
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') => {
                        if let Some(tx) = responder.take() {
                            let _ = tx.send(false);
                        }
                        update_last_awaiting_tool(app, false);
                        app.input_mode = InputMode::Normal;
                    }
                    _ => {}
                }
                return;
            }

            // Normal input mode
            match (key.code, key.modifiers) {
                (KeyCode::Char('c'), KeyModifiers::CONTROL)
                    if app.is_agent_busy =>
                {
                    let _ = message_tx.send(FrontendMessage::Cancel).await;
                }
                (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                    app.should_quit = true;
                }
                (KeyCode::Enter, _) => {
                    if app.input.multi_line {
                        app.input.insert_newline();
                    } else if let Some(text) = app.input.take() {
                        app.handle_user_input(text, message_tx);
                    }
                }
                (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
                    // Ctrl+J = send in multi-line mode
                    if let Some(text) = app.input.take() {
                        app.handle_user_input(text, message_tx);
                    }
                }
                (KeyCode::Tab, _) => {
                    app.input.multi_line = !app.input.multi_line;
                }
                (KeyCode::Backspace, _) => app.input.backspace(),
                (KeyCode::Delete, _) => app.input.delete(),
                (KeyCode::Left, _) => app.input.move_left(),
                (KeyCode::Right, _) => app.input.move_right(),
                (KeyCode::Up, _) => {
                    if app.scroll_mode || app.input.multi_line {
                        // Scroll conversation up
                        app.auto_scroll = false;
                        app.scroll_offset = app.scroll_offset.saturating_add(1);
                    } else {
                        app.input.history_prev();
                    }
                }
                (KeyCode::Down, _) => {
                    if app.scroll_mode || app.input.multi_line {
                        // Scroll conversation down
                        if app.scroll_offset > 0 {
                            app.scroll_offset -= 1;
                            if app.scroll_offset == 0 {
                                app.auto_scroll = true;
                            }
                        }
                    } else {
                        app.input.history_next();
                    }
                }
                (KeyCode::F(8), _) => {
                    app.toggle_scroll_mode();
                }
                (KeyCode::Home, _) => app.input.move_home(),
                (KeyCode::End, _) => app.input.move_end(),
                (KeyCode::PageUp, _) => {
                    app.auto_scroll = false;
                    app.scroll_offset = app.scroll_offset.saturating_add(10);
                }
                (KeyCode::PageDown, _) => {
                    if app.scroll_offset > 10 {
                        app.scroll_offset -= 10;
                    } else {
                        app.scroll_offset = 0;
                        app.auto_scroll = true;
                    }
                }
                (KeyCode::Char(c), _) => app.input.insert_char(c),
                _ => {}
            }
        }
        Event::Resize(_, _) => {
            // Terminal resize — ratatui handles this on next draw
        }
        Event::Mouse(me) => {
            // Always scroll the conversation pane — independent of scroll_mode.
            // Direction swapped for Windows: wheel up = show content below (down),
            // wheel down = show content above (up).
            match me.kind {
                MouseEventKind::ScrollDown => {
                    app.auto_scroll = false;
                    app.scroll_offset = app.scroll_offset.saturating_add(3);
                }
                MouseEventKind::ScrollUp => {
                    if app.scroll_offset >= 3 {
                        app.scroll_offset -= 3;
                        if app.scroll_offset == 0 {
                            app.auto_scroll = true;
                        }
                    } else if app.scroll_offset > 0 {
                        app.scroll_offset = 0;
                        app.auto_scroll = true;
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }

}

/// Update the last AwaitingConfirmation tool card based on user response.
fn update_last_awaiting_tool(_app: &App, _approved: bool) {
    // The Agent will send ToolCallResult after executing (or being rejected),
    // which will update the status. We don't need to do anything here.
}

/// Set a tool card's status to AwaitingConfirmation.
fn set_tool_awaiting(app: &mut App, tool_call_id: &str) {
    for entry in app.conversation.iter_mut().rev() {
        if let crate::app::ConversationEntry::ToolCard {
            tool_call_id: id,
            status,
            ..
        } = entry
        {
            if id == tool_call_id {
                *status = crate::app::ToolStatus::AwaitingConfirmation;
                return;
            }
        }
    }
}
