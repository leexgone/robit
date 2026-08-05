//! robit — TUI frontend for the Robit AI automaton agent.
//!
//! Usage: cargo run -p robit
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
use crossterm::cursor::{EnableBlinking, SetCursorStyle};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use robit_agent::{Agent, AgentEvent, FrontendMessage, bootstrap, log_skill_errors};
use robit_ai::{init_logging_silent, load_config, resolve_image_provider, LlmClient};
use tokio::sync::mpsc;

use app::{App, InputMode};
use tui_frontend::{ConfirmRequest, TuiFrontend};

#[derive(Debug, Parser)]
#[command(name = "robit")]
#[command(about = "AI Automaton Agent with TUI")]
#[command(version)]
struct Cli {
    /// Auto-approve all tool calls, skipping user confirmation
    #[arg(long)]
    auto_approve: bool,

    /// Working directory for the agent
    #[arg(long, short = 'w')]
    workdir: Option<std::path::PathBuf>,
}

fn main() -> Result<()> {
    // Parse CLI args first
    let cli = Cli::parse();

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

    // Acquire directory lock
    let _lock = match robit_agent::DirectoryLock::acquire(&working_dir, "robit-tui") {
        Ok(lock) => lock,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    let config = load_config(cli.workdir.as_deref())?;

    // Initialize tracing (logs go to sink, not terminal) with config log_level
    init_logging_silent(config.app.as_ref(), "robit_tui", &working_dir, &[]);

    // Determine auto_approve: CLI flag takes priority, then config, then default false
    let auto_approve = cli.auto_approve || config.app.as_ref().and_then(|a| a.auto_approve).unwrap_or(false);

    let client = Arc::new(LlmClient::from_config(&config, None)?);
    let model = client.model().to_string();

    // Resolve image generation model (if configured) for status bar display.
    let image_model = resolve_image_provider(&config)
        .ok()
        .map(|p| p.model_id);

    let context_config = config.app.as_ref().and_then(|a| a.context.as_ref());
    let context_window = client.resolved().context_window;

    // Bootstrap skills and tools
    let base_tool_names = ["read", "bash", "write", "edit"];
    let bootstrap_result = bootstrap(&config, &working_dir, &base_tool_names);
    log_skill_errors(&bootstrap_result.skill_load_errors);

    let skill_registry = bootstrap_result.skill_registry;
    let tools = bootstrap_result.tool_registry;

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
        std::collections::HashMap::new(),
    );

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    stdout.execute(EnableMouseCapture)?;
    // Raw mode / alternate screen can leave the cursor steady; explicitly
    // re-enable blinking. Two sequences for coverage:
    //   - DECSET 12 keeps the user's cursor shape on terminals that honor it
    //     (Windows Terminal, xterm, ...);
    //   - DECSCUSR BlinkingBlock is the widely-honored fallback — notably
    //     xterm.js (VSCode's integrated terminal) ignores DECSET 12 but
    //     applies DECSCUSR.
    // Not undone on exit — blinking is the terminal default.
    stdout.execute(EnableBlinking)?;
    stdout.execute(SetCursorStyle::BlinkingBlock)?;
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

        let mut app = App::new(model, image_model, &tools, Arc::clone(&skill_registry));
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

async fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    event_rx: &mut mpsc::Receiver<AgentEvent>,
    confirm_rx: &mut mpsc::Receiver<ConfirmRequest>,
    message_tx: &mpsc::Sender<FrontendMessage>,
    agent_handle: tokio::task::JoinHandle<()>,
) -> Result<()> {
    let mut event_stream = EventStream::new();
    // The tick keeps the loop alive for future animations (e.g. a busy
    // spinner); it deliberately does not trigger a redraw by itself — frames
    // are drawn on demand, when state actually changed.
    let mut tick_interval = tokio::time::interval(std::time::Duration::from_millis(100));
    tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut needs_draw = true;

    loop {
        tokio::select! {
            // Crossterm events (keyboard, mouse, resize)
            maybe_event = event_stream.next() => {
                if let Some(Ok(event)) = maybe_event {
                    handle_crossterm_event(app, event, message_tx).await;
                    needs_draw = true;
                }
            }

            // Agent events
            maybe_event = event_rx.recv() => {
                match maybe_event {
                    Some(event) => {
                        app.handle_agent_event(event);
                        drain_agent_events(app, event_rx);
                        needs_draw = true;
                    }
                    None => {
                        app.should_quit = true;
                    }
                }
            }

            // Confirmation requests from Agent
            maybe_req = confirm_rx.recv() => {
                if let Some(req) = maybe_req {
                    set_tool_status_by_id(
                        app,
                        &req.tool_info.id,
                        crate::app::ToolStatus::AwaitingConfirmation,
                    );
                    app.input_mode = InputMode::Confirmation {
                        tool_call_id: req.tool_info.id,
                        responder: Some(req.responder),
                    };
                    needs_draw = true;
                }
            }

            _ = tick_interval.tick() => {}
        }

        if app.should_quit {
            agent_handle.abort();
            break;
        }

        // Redraw only when something changed (ratatui still diffs the buffer,
        // but this avoids rebuilding/rendering it every tick).
        if needs_draw {
            terminal.draw(|f| ui::draw(f, app))?;
            needs_draw = false;
        }
    }

    Ok(())
}

/// Batch-process all queued agent events so we redraw once per burst rather
/// than once per event (streaming sends one `TextDelta` per token).
fn drain_agent_events(app: &mut App, event_rx: &mut mpsc::Receiver<AgentEvent>) {
    loop {
        match event_rx.try_recv() {
            Ok(event) => app.handle_agent_event(event),
            Err(mpsc::error::TryRecvError::Empty) => break,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                app.should_quit = true;
                break;
            }
        }
    }
}

/// Change a tool card's status by id (also invalidates its cached rendering).
fn set_tool_status_by_id(app: &mut App, tool_call_id: &str, new_status: crate::app::ToolStatus) {
    let idx = app.conversation.iter().position(|entry| match entry {
        crate::app::ConversationEntry::ToolCard { tool_call_id: id, .. } => id == tool_call_id,
        _ => false,
    });
    if let Some(idx) = idx {
        if let crate::app::ConversationEntry::ToolCard { status, .. } =
            &mut app.conversation[idx]
        {
            *status = new_status;
        }
        app.render_cache.invalidate(idx);
    }
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
            if matches!(app.input_mode, InputMode::Confirmation { .. }) {
                let mut decision: Option<(bool, String)> = None;
                if let InputMode::Confirmation {
                    tool_call_id,
                    responder,
                } = &mut app.input_mode
                {
                    match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            if let Some(tx) = responder.take() {
                                let _ = tx.send(true);
                            }
                            decision = Some((true, tool_call_id.clone()));
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') => {
                            if let Some(tx) = responder.take() {
                                let _ = tx.send(false);
                            }
                            decision = Some((false, tool_call_id.clone()));
                        }
                        _ => {}
                    }
                }
                if let Some((approved, id)) = decision {
                    if approved {
                        // Immediate feedback: show the card as running. The
                        // final Success/Failed status arrives via ToolCallResult.
                        set_tool_status_by_id(app, &id, crate::app::ToolStatus::Running);
                    } else {
                        // Remember the rejection so the (error) result that the
                        // agent sends back is displayed as "Rejected", not "Failed".
                        app.rejected_tool_ids.insert(id);
                    }
                    app.input_mode = InputMode::Normal;
                }
                return;
            }

            // Normal input mode
            match (key.code, key.modifiers) {
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                    // Ctrl+C = quit the application
                    app.should_quit = true;
                }
                (KeyCode::Char('d'), KeyModifiers::CONTROL)
                    if app.is_agent_busy =>
                {
                    // Ctrl+D = cancel the in-flight operation
                    let _ = message_tx.send(FrontendMessage::Cancel).await;
                }
                (KeyCode::Enter, _) => {
                    if app.input.multi_line {
                        app.input.insert_newline();
                    } else if let Some(text) = app.input.take() {
                        app.handle_user_input(text, message_tx).await;
                    }
                }
                (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
                    // Ctrl+J = send in multi-line mode
                    if let Some(text) = app.input.take() {
                        app.handle_user_input(text, message_tx).await;
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
                        // Move toward the latest content (offset-from-bottom shrinks).
                        app.scroll_offset = app.scroll_offset.saturating_sub(1);
                        if app.scroll_offset == 0 {
                            app.auto_scroll = true;
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
                    // Move toward the latest content.
                    app.scroll_offset = app.scroll_offset.saturating_sub(10);
                    if app.scroll_offset == 0 {
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
            // scroll_offset is the distance from the bottom: wheel up moves
            // away from the latest content (into history), wheel down moves
            // back toward it.
            match me.kind {
                MouseEventKind::ScrollUp => {
                    app.auto_scroll = false;
                    app.scroll_offset = app.scroll_offset.saturating_add(3);
                }
                MouseEventKind::ScrollDown => {
                    app.scroll_offset = app.scroll_offset.saturating_sub(3);
                    if app.scroll_offset == 0 {
                        app.auto_scroll = true;
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }

}
