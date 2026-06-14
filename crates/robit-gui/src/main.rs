//! robit-gui — Tauri v2 desktop GUI for the Robit AI automaton agent.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(dead_code)] // Allow unused code for now, will be used in UI

mod commands;
mod config;
mod db;
mod events;
mod frontend;
mod state;

use std::sync::Arc;

use clap::Parser;
use robit_ai::config::load_config;
use robit_ai::LlmClient;

use state::AppState;

#[derive(Debug, Parser)]
#[command(name = "robit-gui")]
#[command(about = "AI Automaton Agent with GUI")]
struct Cli {
    /// Working directory for the agent
    #[arg(long, short = 'w')]
    workdir: Option<std::path::PathBuf>,

    /// Store session database in the global Robit config directory
    #[arg(long)]
    global_storage: bool,
}

fn main() {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("robit_gui=info".parse().unwrap()),
        )
        .init();

    let config =
        load_config(cli.workdir.as_deref()).expect("Failed to load config.toml configuration");
    let client =
        Arc::new(LlmClient::from_config(&config, None).expect("Failed to initialize LLM client"));

    let app_state = AppState::new(client, config, cli.workdir, cli.global_storage)
        .expect("Failed to initialize app state");

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::create_session,
            commands::list_sessions,
            commands::switch_session,
            commands::send_message,
            commands::cancel,
            commands::delete_session,
            commands::rename_session,
            commands::get_messages,
            commands::confirm_tool,
            commands::get_config,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
