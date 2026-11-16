//! `robit-wechat` binary entry point.
//!
//! Two modes:
//! - `robit-wechat login`: interactive QR-code login that persists the
//!   long-lived `bot_token` to `wechat-state.json`.
//! - default run: loads config, bootstraps the agent (LLM client, tools,
//!   skills), starts the WeChat iLink Bot long-poll loop, and runs the
//!   `ChatbotManager` event loop. Each WeChat peer gets an independent Agent
//!   session.

use std::sync::Arc;

use clap::{Parser, Subcommand};
use robit_agent::skill::SkillRegistry;
use robit_agent::{
    create_tools_from_config, filter_skills_by_config, load_all_skills, log_skill_errors,
};
use robit_ai::{init_logging, load_config, LlmClient};
use robit_chatbot::tool::SendFileTool;
use robit_chatbot::ChatbotManager;
use robit_wechat::{login, WechatConfig, WechatPlatformAdapter};

#[derive(Debug, Parser)]
#[command(name = "robit-wechat")]
#[command(about = "Robit AI Agent - WeChat Bot (iLink Bot channel)")]
#[command(version)]
struct Cli {
    /// Working directory for the agent (defaults to the current directory).
    #[arg(long, short = 'w', global = true)]
    workdir: Option<std::path::PathBuf>,

    /// Use global storage (~/.robit/memory/robit.db) for the session database.
    #[arg(long, global = true)]
    global_storage: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Scan a QR code with WeChat to log in and save the bot token.
    Login,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let working_dir = cli
        .workdir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().expect("Failed to get current directory"));

    // Acquire directory lock (shared name across modes: login writes the
    // state file the running bot reads).
    let _lock = match robit_agent::DirectoryLock::acquire(&working_dir, "robit-wechat") {
        Ok(lock) => lock,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    let config = load_config(Some(&working_dir)).expect("Failed to load config.toml");

    // Login subcommand: only needs config + interactive QR flow.
    if let Some(Command::Login) = cli.command {
        if let Err(e) = login::run_login(&working_dir, &config).await {
            eprintln!("登录失败: {}", e);
            std::process::exit(1);
        }
        return;
    }

    // Initialize logging with config
    init_logging(config.app.as_ref(), "robit_wechat", &working_dir, &[]);

    // 2. Initialize the LLM client.
    let llm_client = Arc::new(
        LlmClient::from_config(&config, None).expect("Failed to initialize LLM client"),
    );

    // 3. Bootstrap tools and skills (with custom tool registration).
    let base_tool_names = ["read", "bash", "write", "edit"];
    let (skills, skill_load_errors) = load_all_skills(&working_dir);
    let filtered_skills = filter_skills_by_config(skills, &config);
    let skill_registry = Arc::new(SkillRegistry::new(filtered_skills, &base_tool_names));
    let mut tool_registry = create_tools_from_config(&config, Arc::clone(&skill_registry));

    // Register chatbot-specific tools.
    tool_registry.register(SendFileTool);

    let tool_registry = Arc::new(tool_registry);
    log_skill_errors(&skill_load_errors);

    // 4. Connect to the WeChat platform (long-poll loop starts in the
    //    background; a missing token surfaces here with a login hint).
    let wechat_config = match WechatConfig::from_config(&config, &working_dir) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let platform = WechatPlatformAdapter::connect(wechat_config, shutdown.clone())
        .await
        .expect("Failed to start WeChat poll loop");

    // 5. Create the manager and run.
    let manager = ChatbotManager::new(
        platform,
        config,
        working_dir,
        llm_client,
        tool_registry,
        skill_registry,
    )
    .expect("Failed to create ChatbotManager");

    tracing::info!("robit-wechat bot is running");

    // 6. Run with graceful shutdown on Ctrl+C.
    let shutdown_clone = shutdown.clone();

    tokio::select! {
        result = manager.run(shutdown_clone) => {
            if let Err(e) = result {
                tracing::error!("ChatbotManager error: {}", e);
            }
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received Ctrl+C, shutting down gracefully...");
            shutdown.notify_waiters();
        }
    }

    // Give background tasks a moment to flush (SQLite WAL, state file, etc.)
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    tracing::info!("robit-wechat bot has stopped");
}
