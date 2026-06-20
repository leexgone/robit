//! `robit-qq` binary entry point.
//!
//! Loads config, bootstraps the agent (LLM client, tools, skills), connects to
//! the QQ Official Bot WebSocket gateway, and runs the `ChatbotManager` event
//! loop. Each QQ chat (group or private) gets an independent Agent session.

use std::sync::Arc;

use clap::Parser;
use robit_agent::{bootstrap, log_skill_errors};
use robit_ai::{load_config, LlmClient};
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("robit_qq=info".parse().unwrap())
                .add_directive("robit_chatbot=info".parse().unwrap())
                .add_directive("reqwest=warn".parse().unwrap())
                .add_directive("hyper=warn".parse().unwrap())
                .add_directive("hyper_util=warn".parse().unwrap())
                .add_directive("tungstenite=warn".parse().unwrap())
                .add_directive("tokio_tungstenite=warn".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    let working_dir = cli
        .workdir
        .unwrap_or_else(|| std::env::current_dir().expect("Failed to get current directory"));

    // 1. Load configuration.
    let config = load_config(Some(&working_dir)).expect("Failed to load config.toml");

    // 2. Initialize the LLM client.
    let llm_client = Arc::new(
        LlmClient::from_config(&config, None).expect("Failed to initialize LLM client"),
    );

    // 3. Bootstrap tools and skills.
    let base_tool_names = ["read", "bash", "write", "edit"];
    let bootstrap_result = bootstrap(&config, &working_dir, &base_tool_names);
    log_skill_errors(&bootstrap_result.skill_load_errors);

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
        bootstrap_result.tool_registry,
        bootstrap_result.skill_registry,
    )
    .expect("Failed to create ChatbotManager");

    tracing::info!("robit-qq bot is running");

    // 6. Run with graceful shutdown on Ctrl+C.
    tokio::select! {
        result = manager.run() => {
            if let Err(e) = result {
                tracing::error!("ChatbotManager error: {}", e);
            }
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received Ctrl+C, shutting down...");
        }
    }

    tracing::info!("robit-qq bot has stopped");
}
