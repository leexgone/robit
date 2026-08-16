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
use robit_ai::{init_logging, load_config, LlmClient};

use state::AppState;

#[derive(Debug, Parser)]
#[command(name = "robit-gui")]
#[command(about = "AI Automaton Agent with GUI")]
#[command(version)]
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

    // Resolve working directory first
    let working_dir = cli
        .workdir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().expect("Failed to get current directory"));

    // Acquire directory lock. Held in an Arc<Mutex<Option<_>>> so the Tauri
    // exit handler can release it explicitly: Tauri terminates the process
    // without running main()'s destructors, so plain RAII would leave the
    // LOCK file behind on every GUI close.
    let lock = match robit_agent::DirectoryLock::acquire(&working_dir, "robit-gui") {
        Ok(lock) => lock,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };
    let lock = Arc::new(std::sync::Mutex::new(Some(lock)));
    let lock_for_exit = Arc::clone(&lock);

    let config =
        load_config(cli.workdir.as_deref()).expect("Failed to load config.toml configuration");

    // Initialize logging with config log_level
    init_logging(config.app.as_ref(), "robit_gui", &working_dir, &[]);

    let client =
        Arc::new(LlmClient::from_config(&config, None).expect("Failed to initialize LLM client"));

    let app_state = AppState::new(client, config, cli.workdir, cli.global_storage)
        .expect("Failed to initialize app state");

    let app = tauri::Builder::default()
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
            commands::read_image_file,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(move |_app_handle, event| {
        // RunEvent::Exit fires before the process terminates — the only
        // reliable place to run cleanup, since destructors are skipped.
        if let tauri::RunEvent::Exit = event {
            tracing::info!("Application exiting, releasing directory lock");
            if let Ok(mut guard) = lock_for_exit.lock() {
                // Take the lock out and drop it: DirectoryLock's Drop
                // unlocks the file handle and deletes the LOCK file.
                drop(guard.take());
            }
        }
    });
}
