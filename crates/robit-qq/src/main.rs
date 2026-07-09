//! `robit-qq` binary entry point.
//!
//! Loads config, bootstraps the agent (LLM client, tools, skills), connects to
//! the QQ Official Bot WebSocket gateway, and runs the `ChatbotManager` event
//! loop. Each QQ chat (group or private) gets an independent Agent session.

use std::sync::Arc;

use clap::Parser;
use robit_agent::{create_tools_from_config, filter_skills_by_config, load_all_skills, log_skill_errors};
use robit_agent::skill::SkillRegistry;
use robit_ai::{init_logging, load_config, LlmClient};
use robit_chatbot::tool::SendFileTool;
use robit_chatbot::ChatbotManager;
use robit_qq::{QqConfig, QqPlatformAdapter};

#[derive(Debug, Parser)]
#[command(name = "robit-qq")]
#[command(about = "Robit AI Agent - QQ Bot")]
#[command(version)]
struct Cli {
    /// Working directory for the agent (defaults to the current directory).
    #[arg(long, short = 'w')]
    workdir: Option<std::path::PathBuf>,

    /// Use global storage (~/.robit/memory/robit.db) for the session database.
    #[arg(long)]
    global_storage: bool,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Load config first so we can use log_level from config
    let working_dir = cli
        .workdir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().expect("Failed to get current directory"));
    let config = load_config(Some(&working_dir)).expect("Failed to load config.toml");

    // Initialize logging with config
    init_logging(config.app.as_ref(), "robit_qq", &working_dir, &[]);

    // 2. Initialize the LLM client.
    let llm_client = Arc::new(
        LlmClient::from_config(&config, None).expect("Failed to initialize LLM client"),
    );

    // 3. Bootstrap tools and skills (with custom tool registration).
    let base_tool_names = ["read", "bash", "write", "edit"];
    let (skills, skill_load_errors) = load_all_skills(&working_dir);
    // let total_skills_loaded = skills.len();
    let filtered_skills = filter_skills_by_config(skills, &config);
    let skill_registry = Arc::new(SkillRegistry::new(filtered_skills, &base_tool_names));
    let mut tool_registry = create_tools_from_config(&config, Arc::clone(&skill_registry));

    // Register chatbot-specific tools.
    tool_registry.register(SendFileTool);

    let tool_registry = Arc::new(tool_registry);
    log_skill_errors(&skill_load_errors);

    // 4. Connect to the QQ platform.
    let qq_config = QqConfig::from_config(&config).expect("QQ Bot config not found");
    let platform = QqPlatformAdapter::connect(qq_config)
        .await
        .expect("Failed to connect to QQ gateway");

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

    tracing::info!("robit-qq bot is running");

    // 6. Run with graceful shutdown on Ctrl+C.
    // Note: spawned WebSocket tasks (heartbeat, dispatch) hold Arc references
    // and block the tokio runtime from shutting down when main() returns.
    // std::process::exit() ensures immediate exit regardless of leftover tasks.
    tokio::select! {
        result = manager.run() => {
            if let Err(e) = result {
                tracing::error!("ChatbotManager error: {}", e);
            }
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received Ctrl+C, shutting down...");
            std::process::exit(0);
        }
    }

    tracing::info!("robit-qq bot has stopped");
}
