//! Shared logging initialization for Robit binaries.
//!
//! Provides a unified way to initialize logging with support for:
//! - Config file `app.log_level` setting
//! - Environment variable `RUST_LOG` (takes precedence)
//! - Sensible defaults for third-party crates

use crate::config::AppConfig;
use tracing_subscriber::{filter::Directive, EnvFilter};

/// Initialize logging with optional app config and a target crate name.
///
/// Priority order:
/// 1. `RUST_LOG` environment variable (full control)
/// 2. `app.log_level` from config.toml (sets global level)
/// 3. Defaults to `info` for the target crate and `warn` for third-party crates
///
/// # Arguments
/// - `app_config`: Optional `AppConfig` from config.toml
/// - `target_crate`: Name of the target crate (e.g. "robit_tui", "robit_qq")
/// - `additional_directives`: Optional additional `Directive`s for specific crates
pub fn init_logging(
    app_config: Option<&AppConfig>,
    target_crate: &str,
    additional_directives: &[&str],
) {
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

    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Initialize logging but discard output (for TUI mode).
///
/// Same as `init_logging` but logs go to `/dev/null` instead of stdout.
pub fn init_logging_silent(
    app_config: Option<&AppConfig>,
    target_crate: &str,
    additional_directives: &[&str],
) {
    let mut filter = EnvFilter::from_default_env();

    if std::env::var("RUST_LOG").is_err() {
        let global_level = app_config
            .and_then(|c| c.log_level.as_deref())
            .unwrap_or("info");

        if let Ok(dir) = format!("{}={}", target_crate, global_level).parse() {
            filter = filter.add_directive(dir);
        }

        for robit_crate in &["robit_agent", "robit_chatbot", "robit_ai"] {
            if robit_crate != &target_crate {
                if let Ok(dir) = format!("{}={}", robit_crate, global_level).parse() {
                    filter = filter.add_directive(dir);
                }
            }
        }

        for dir_str in additional_directives {
            if let Ok(dir) = dir_str.parse::<Directive>() {
                filter = filter.add_directive(dir);
            }
        }

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

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::sink)
        .init();
}
