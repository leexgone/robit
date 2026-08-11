//! Shared logging initialization for Robit binaries.
//!
//! Provides a unified way to initialize logging with support for:
//! - Config file `app.log_level` setting
//! - Config file `app.log_file` setting (log to file, daily rotation, local time)
//! - Local-time timestamps and log file names (falls back to UTC if the system
//!   offset can't be determined)
//! - Config file `app.log_retention_days` setting (delete old `robit-*.log`
//!   files on startup; default 14 days, `0` disables)
//! - Environment variable `RUST_LOG` (takes precedence)
//! - Sensible defaults for third-party crates

use crate::config::AppConfig;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use time::format_description;
use time::UtcOffset;
use tracing_subscriber::fmt::time::OffsetTime;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{filter::Directive, EnvFilter};

/// Get the log file path: {working_dir}/.robit/logs/robit-YYYY-MM-DD.log
///
/// Creates the directory if it doesn't exist. The date uses `offset` (local
/// time by default) so the file rolls at local midnight, not UTC midnight.
fn get_log_file_path(
    working_dir: &PathBuf,
    offset: UtcOffset,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let logs_dir = working_dir.join(".robit").join("logs");

    // Create logs directory if it doesn't exist
    std::fs::create_dir_all(&logs_dir)?;

    // Format date as YYYY-MM-DD (local time)
    let now = time::OffsetDateTime::now_utc().to_offset(offset);
    let format = format_description::parse_borrowed::<3>("[year]-[month]-[day]").unwrap();
    let date = now.format(&format).unwrap();
    let log_file = logs_dir.join(format!("robit-{}.log", date));

    Ok(log_file)
}

/// Determine the system's local UTC offset for log timestamps and file naming.
///
/// Falls back to UTC (with a stderr warning) if the offset can't be determined.
/// In `time` 0.3.37+ this works on any thread: Unix uses the reentrant
/// `localtime_r`, Windows uses `SystemTimeToTzSpecificLocalTime` - both
/// thread-safe - so it's safe to call even after the tokio runtime starts.
fn local_utc_offset() -> UtcOffset {
    match UtcOffset::current_local_offset() {
        Ok(offset) => offset,
        Err(_) => {
            eprintln!(
                "Could not determine local time offset; log timestamps will be UTC."
            );
            UtcOffset::UTC
        }
    }
}

/// Build a tracing timer that stamps events with local time. The format matches
/// the prior default (`YYYY-MM-DDTHH:MM:SS.ffffff`) but carries the local
/// offset (e.g. `+08:00`) instead of `Z`. The format description is parsed
/// once at init from a `&'static str` literal, so the result is `'static` -
/// no leak needed.
fn local_timer(
    offset: UtcOffset,
) -> OffsetTime<format_description::FormatDescriptionV3<'static>> {
    let format = format_description::parse_borrowed::<3>(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6][offset_hour sign:mandatory]:[offset_minute]",
    )
    .expect("hardcoded log timestamp format is valid");
    OffsetTime::new(offset, format)
}

/// Delete `robit-YYYY-MM-DD.log` files whose modification time is older than
/// `retention_days` days. Best-effort: unreadable entries and deletion
/// failures are logged via `tracing::warn!` and skipped. `retention_days == 0`
/// disables cleanup. Only files matching `robit-*.log` are considered, so
/// `err.log` and other files are left untouched. Called once at startup.
fn cleanup_old_logs(logs_dir: &Path, retention_days: u32) {
    if retention_days == 0 {
        return;
    }
    let cutoff = SystemTime::now() - Duration::from_secs(retention_days as u64 * 86_400);
    let entries = match std::fs::read_dir(logs_dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("Failed to scan log dir for cleanup: {}", e);
            return;
        }
    };
    let mut removed = 0u32;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("robit-") && name.ends_with(".log")) {
            continue;
        }
        let modified = match entry.metadata().and_then(|m| m.modified()) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if modified < cutoff {
            if let Err(e) = std::fs::remove_file(entry.path()) {
                tracing::warn!("Failed to delete old log {}: {}", name, e);
            } else {
                removed += 1;
            }
        }
    }
    if removed > 0 {
        tracing::info!(
            "Cleaned up {} old log file(s) (retention {} days).",
            removed,
            retention_days
        );
    }
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

/// Install a panic hook that records panics through the tracing subscriber
/// (so they land in the log file) before chaining to the previous hook (which
/// prints to stderr).
///
/// Without this, a panic in a spawned task goes to stderr only. A binary run
/// detached (e.g. the robit-qq server under nohup/systemd with stderr in a
/// separate file) then leaves no trace in the tracing log, and the failure
/// looks like an unexplained silent stall. This is exactly what masked the QQ
/// gateway supervisor panic ("JoinHandle polled after completion"): it
/// appeared in err.log (stderr) but never in the tracing log, so the server
/// silently lost its QQ connection after every ~30-min gateway rotation.
///
/// Must be called AFTER the tracing subscriber is initialized, otherwise the
/// `tracing::error!` is dropped (no subscriber). The hook chains to whatever
/// hook was installed before it, so an existing hook still runs - e.g. the TUI
/// installs its terminal-restore hook after `init_logging_silent`, so its
/// `take_hook` captures this one and the chain becomes: TUI restore -> tracing
/// log -> default stderr.
fn install_panic_hook() {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Best-effort message extraction. `panic!("literal")` yields `&'static str`,
        // `panic!("{}", x)` yields `String`; anything else falls back to a placeholder
        // so the log line is still useful.
        let payload_msg: String = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown location>".to_string());
        let thread_name = std::thread::current().name().unwrap_or("<unnamed>").to_string();

        // The default target is the module path (`robit_ai::logging`), which
        // the `robit_ai` EnvFilter directive (always added by `build_filter`)
        // matches, so this passes the filter in every config. `error!` is used
        // because a panic is always severe, and error >= any configured level
        // except `off`.
        tracing::error!(
            "thread '{}' panicked at {}: {}",
            thread_name,
            location,
            payload_msg
        );

        // Chain to the previous hook (default: stderr) so existing behavior is
        // preserved.
        previous_hook(info);
    }));
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

    // Local time offset, used for both the timestamp format and the daily log
    // file name. Falls back to UTC if undeterminable.
    let offset = local_utc_offset();
    let timer = local_timer(offset);

    // Check if file logging is enabled
    let log_file_enabled = app_config.and_then(|c| c.log_file).unwrap_or(false);

    if log_file_enabled {
        // Log to both console and file
        match get_log_file_path(working_dir, offset) {
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
                            .with_timer(timer.clone())
                            .with_filter(filter.clone());

                        let file_layer = tracing_subscriber::fmt::layer()
                            .with_writer(file_writer)
                            .with_ansi(false)
                            .with_timer(timer.clone())
                            .with_filter(filter);

                        // Combine layers
                        let registry = tracing_subscriber::registry()
                            .with(console_layer)
                            .with(file_layer);

                        registry.init();

                        tracing::info!("Logging to file: {}", log_path.display());

                        // Best-effort cleanup of old daily log files.
                        let retention = app_config
                            .and_then(|c| c.log_retention_days)
                            .unwrap_or(14);
                        if let Some(dir) = log_path.parent() {
                            cleanup_old_logs(dir, retention);
                        }
                    }
                    Err(e) => {
                        // Fallback to console-only logging
                        eprintln!("Failed to open log file: {}. Falling back to console-only logging.", e);
                        tracing_subscriber::fmt()
                            .with_env_filter(filter)
                            .with_timer(timer.clone())
                            .init();
                    }
                }
            }
            Err(e) => {
                // Fallback to console-only logging
                eprintln!("Failed to prepare log path: {}. Falling back to console-only logging.", e);
                tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_timer(timer.clone())
                    .init();
            }
        }
    } else {
        // Console-only logging (default)
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_timer(timer.clone())
            .init();
    }

    // Install after the subscriber is set up so panics are captured in the log.
    install_panic_hook();
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

    let offset = local_utc_offset();
    let timer = local_timer(offset);

    let log_file_enabled = app_config.and_then(|c| c.log_file).unwrap_or(false);

    if log_file_enabled {
        // TUI mode: no console output, but write to file if enabled.
        match get_log_file_path(working_dir, offset) {
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
                        .with_timer(timer.clone())
                        .init();
                    tracing::info!("Logging to file: {}", log_path.display());

                    // Best-effort cleanup of old daily log files.
                    let retention = app_config
                        .and_then(|c| c.log_retention_days)
                        .unwrap_or(14);
                    if let Some(dir) = log_path.parent() {
                        cleanup_old_logs(dir, retention);
                    }
                }
                Err(e) => {
                    eprintln!(
                        "Failed to open log file: {}. Falling back to silent logging.",
                        e
                    );
                    tracing_subscriber::fmt()
                        .with_env_filter(filter)
                        .with_writer(std::io::sink)
                        .with_timer(timer.clone())
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
                    .with_timer(timer.clone())
                    .init();
            }
        }
    } else {
        // No file logging configured: discard all logs (TUI mode).
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::sink)
            .with_timer(timer.clone())
            .init();
    }

    // Install after the subscriber is set up so panics are captured in the log.
    install_panic_hook();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cleanup_old_logs` must delete only old `robit-*.log` files: keep recent
    /// robit logs, and leave non-matching files (e.g. `err.log`) untouched
    /// regardless of age. `retention_days == 0` disables cleanup entirely.
    #[test]
    fn cleanup_old_logs_deletes_old_keeps_recent_and_ignores_others() {
        let dir = std::env::temp_dir().join(format!("robit-log-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // mtime far in the past => older than any reasonable retention window.
        let ancient = SystemTime::now() - Duration::from_secs(60 * 86_400); // 60 days ago

        let write_with_mtime = |name: &str, mtime: SystemTime| {
            let path = dir.join(name);
            let f = std::fs::File::create(&path).unwrap();
            f.set_modified(mtime).unwrap();
            path
        };

        let old = write_with_mtime("robit-2000-01-01.log", ancient); // should be deleted
        let recent = write_with_mtime("robit-2099-01-01.log", SystemTime::now()); // keep
        let other = write_with_mtime("err.log", ancient); // untouched (non-robit)

        cleanup_old_logs(&dir, 14);

        assert!(!old.exists(), "old robit-*.log should be deleted");
        assert!(recent.exists(), "recent robit-*.log should be kept");
        assert!(other.exists(), "non-robit file should be untouched");

        // retention_days == 0 disables cleanup: even ancient files survive.
        let old2 = write_with_mtime("robit-2001-01-01.log", ancient);
        cleanup_old_logs(&dir, 0);
        assert!(old2.exists(), "retention_days=0 should disable cleanup");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
