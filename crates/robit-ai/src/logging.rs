//! Shared logging initialization for Robit binaries.
//!
//! Provides a unified way to initialize logging with support for:
//! - Config file `app.log_level` setting
//! - Config file `app.log_file` setting (log to file, daily rotation)
//! - Environment variable `RUST_LOG` (takes precedence)
//! - Sensible defaults for third-party crates

use crate::config::AppConfig;
use std::fs::OpenOptions;
use std::path::PathBuf;
use time::format_description;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{filter::Directive, EnvFilter};

/// Get the log file path: {working_dir}/.robit/logs/robit-YYYY-MM-DD.log
///
/// Creates the directory if it doesn't exist.
fn get_log_file_path(working_dir: &PathBuf) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let logs_dir = working_dir.join(".robit").join("logs");

    // Create logs directory if it doesn't exist
    std::fs::create_dir_all(&logs_dir)?;

    // Format date as YYYY-MM-DD (UTC)
    let now = time::OffsetDateTime::now_utc();
    let format = format_description::parse_borrowed::<3>("[year]-[month]-[day]").unwrap();
    let date = now.format(&format).unwrap();
    let log_file = logs_dir.join(format!("robit-{}.log", date));

    Ok(log_file)
}

/// Build the EnvFilter from config and defaults.
fn build_filter(
    app_config: Option<&AppConfig>,
    target_crate: &str,
    additional_directives: &[&str],
) -> EnvFilter {
    let mut filter = EnvFilter::from_default_env();

    // If no RUST_LOG is set, build from config and defaults
    if std::env::var("RUST_LOG").is_err() {
        // Use log_level from config if present, otherwise default to info
        let global_level = app_config
            .and_then(|c| c.log_level.as_deref())
            .unwrap_or("info");

        // Add target crate directive
        if let Ok(dir) = format!("{}={}", target_crate, global_level).parse() {
            filter = filter.add_directive(dir);
        }

        // Also set robit crates to the same level
        for robit_crate in &["robit_agent", "robit_chatbot", "robit_ai"] {
            if robit_crate != &target_crate {
                if let Ok(dir) = format!("{}={}", robit_crate, global_level).parse() {
                    filter = filter.add_directive(dir);
                }
            }
        }

        // Add additional directives
        for dir_str in additional_directives {
            if let Ok(dir) = dir_str.parse::<Directive>() {
                filter = filter.add_directive(dir);
            }
        }

        // Default third-party crates to warn
        for dep_crate in &[
            "reqwest",
            "hyper",
            "hyper_util",
            "tungstenite",
            "tokio_tungstenite",
            "tokio",
            "tauri",
        ] {
            if let Ok(dir) = format!("{}=warn", dep_crate).parse() {
                filter = filter.add_directive(dir);
            }
        }
    }

    filter
}

/// Initialize logging with optional app config and a target crate name.
///
/// Priority order:
/// 1. `RUST_LOG` environment variable (full control)
/// 2. `app.log_level` from config.toml (sets global level)
/// 3. Defaults to `info` for the target crate and `warn` for third-party crates
///
/// If `app.log_file = true`, logs are also written to:
///   {working_dir}/.robit/logs/robit-YYYY-MM-DD.log (daily rotation)
///
/// # Arguments
/// - `app_config`: Optional `AppConfig` from config.toml
/// - `target_crate`: Name of the target crate (e.g. "robit_tui", "robit_qq")
/// - `working_dir`: Working directory for the agent (where .robit/logs is created)
/// - `additional_directives`: Optional additional `Directive`s for specific crates
pub fn init_logging(
    app_config: Option<&AppConfig>,
    target_crate: &str,
    working_dir: &PathBuf,
    additional_directives: &[&str],
) {
    let filter = build_filter(app_config, target_crate, additional_directives);

    // Check if file logging is enabled
    let log_file_enabled = app_config.and_then(|c| c.log_file).unwrap_or(false);

    if log_file_enabled {
        // Log to both console and file
        match get_log_file_path(working_dir) {
            Ok(log_path) => {
                match OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log_path)
                {
                    Ok(file) => {
                        let file_writer = tracing_subscriber::fmt::writer::MakeWriterExt::with_max_level(file, tracing::Level::TRACE);

                        // Create layers
                        let console_layer = tracing_subscriber::fmt::layer()
                            .with_writer(std::io::stdout)
                            .with_filter(filter.clone());

                        let file_layer = tracing_subscriber::fmt::layer()
                            .with_writer(file_writer)
                            .with_ansi(false)
                            .with_filter(filter);

                        // Combine layers
                        let registry = tracing_subscriber::registry()
                            .with(console_layer)
                            .with(file_layer);

                        registry.init();

                        tracing::info!("Logging to file: {}", log_path.display());
                    }
                    Err(e) => {
                        // Fallback to console-only logging
                        eprintln!("Failed to open log file: {}. Falling back to console-only logging.", e);
                        tracing_subscriber::fmt().with_env_filter(filter).init();
                    }
                }
            }
            Err(e) => {
                // Fallback to console-only logging
                eprintln!("Failed to prepare log path: {}. Falling back to console-only logging.", e);
                tracing_subscriber::fmt().with_env_filter(filter).init();
            }
        }
    } else {
        // Console-only logging (default)
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
}

/// Initialize logging but discard console output (for TUI mode).
///
/// Same as `init_logging` but logs go to `/dev/null` instead of stdout, so
/// they don't corrupt the terminal UI. When `app.log_file = true`, logs are
/// still written to `{working_dir}/.robit/logs/robit-YYYY-MM-DD.log` - only
/// console output is suppressed.
pub fn init_logging_silent(
    app_config: Option<&AppConfig>,
    target_crate: &str,
    working_dir: &PathBuf,
    additional_directives: &[&str],
) {
    let filter = build_filter(app_config, target_crate, additional_directives);

    let log_file_enabled = app_config.and_then(|c| c.log_file).unwrap_or(false);

    if log_file_enabled {
        // TUI mode: no console output, but write to file if enabled.
        match get_log_file_path(working_dir) {
            Ok(log_path) => match OpenOptions::new().create(true).append(true).open(&log_path) {
                Ok(file) => {
                    let file_writer =
                        tracing_subscriber::fmt::writer::MakeWriterExt::with_max_level(
                            file,
                            tracing::Level::TRACE,
                        );
                    tracing_subscriber::fmt()
                        .with_env_filter(filter)
                        .with_writer(file_writer)
                        .with_ansi(false)
                        .init();
                    tracing::info!("Logging to file: {}", log_path.display());
                }
                Err(e) => {
                    eprintln!(
                        "Failed to open log file: {}. Falling back to silent logging.",
                        e
                    );
                    tracing_subscriber::fmt()
                        .with_env_filter(filter)
                        .with_writer(std::io::sink)
                        .init();
                }
            },
            Err(e) => {
                eprintln!(
                    "Failed to prepare log path: {}. Falling back to silent logging.",
                    e
                );
                tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_writer(std::io::sink)
                    .init();
            }
        }
    } else {
        // No file logging configured: discard all logs (TUI mode).
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::sink)
            .init();
    }
}
