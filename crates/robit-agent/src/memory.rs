//! File-based memory mechanism.
//!
//! When the SQLite memory tools are disabled (`memory_mode = "file"`), the
//! agent keeps long-term memory in Markdown files under the memory
//! directory: a master `memory.md` plus daily `memory-YYYY-MM-DD.md` files.
//! This module resolves the memory settings and builds the
//! `{memory_section}` injected into the system prompt: the mechanism
//! explanation plus the current master file content (bounded by
//! [`MAX_MEMORY_INJECT_BYTES`], truncated beyond that).

use std::path::Path;

use robit_ai::config::{resolve_memory_mode, MemoryMode, RobitConfig};

use crate::error::Result;
use crate::storage::resolve_memory_dir;

const MEMORY_PROMPT_TEMPLATE: &str = include_str!("../prompts/memory.md");

/// Upper bound for the master memory file content injected into the
/// system prompt. Larger files are truncated with a note pointing at `read`.
pub const MAX_MEMORY_INJECT_BYTES: usize = 16 * 1024;

/// Memory settings for one Agent construction.
#[derive(Debug, Clone)]
pub struct MemorySettings {
    /// The active memory mechanism.
    pub mode: MemoryMode,
    /// Directory holding the memory files (same root as robit.db).
    pub dir: std::path::PathBuf,
}

/// Resolve memory settings from config. Called by frontends when
/// constructing an Agent.
pub fn resolve_memory_settings(
    config: &RobitConfig,
    working_dir: &Path,
) -> Result<MemorySettings> {
    let mode = resolve_memory_mode(config);
    let global_storage = config
        .app
        .as_ref()
        .and_then(|a| a.global_storage)
        .unwrap_or(false);
    let dir = resolve_memory_dir(working_dir, global_storage)?;
    Ok(MemorySettings { mode, dir })
}

/// Build the `{memory_section}` text for the system prompt.
///
/// Returns an empty string for `tools` / `off` modes; for `file` mode
/// returns the mechanism explanation plus the master file content.
/// The `## Memory` heading is generated here (carried by the non-empty
/// text), so no empty heading leaks into the prompt.
pub fn build_memory_section(settings: &MemorySettings, date: &str) -> String {
    match settings.mode {
        MemoryMode::Tools | MemoryMode::Off => String::new(),
        MemoryMode::File => build_file_section(settings, date),
    }
}

fn build_file_section(settings: &MemorySettings, date: &str) -> String {
    let dir_display = settings.dir.display().to_string();
    let explanation = MEMORY_PROMPT_TEMPLATE
        .replace("{memory_dir}", &dir_display)
        .replace("{date}", date);

    let mut section = format!("## Memory\n\n{}", explanation.trim());

    match read_master_file(&settings.dir) {
        Ok(None) => {} // not created yet — the explanation covers first-time creation
        Ok(Some(content)) => {
            section.push_str("\n\n---\n\n");
            section.push_str(&content);
        }
        Err(err) => {
            // Read failure must not block startup: degrade to the
            // explanation plus a failure note.
            tracing::warn!(
                "Failed to read {}: {}",
                settings.dir.join("memory.md").display(),
                err
            );
            section.push_str(&format!(
                "\n\n(Failed to read memory.md: {}. You can check the file later \
                 with the read tool.)",
                err
            ));
        }
    }

    section
}

/// Read the master memory file. Returns `Ok(None)` when it does not exist yet.
/// Content larger than [`MAX_MEMORY_INJECT_BYTES`] is truncated with a note.
fn read_master_file(dir: &Path) -> std::io::Result<Option<String>> {
    let path = dir.join("memory.md");
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if bytes.len() > MAX_MEMORY_INJECT_BYTES {
        // The slice may split a multi-byte char at the cut point;
        // from_utf8_lossy substitutes it instead of failing.
        let mut content =
            String::from_utf8_lossy(&bytes[..MAX_MEMORY_INJECT_BYTES]).into_owned();
        content.push_str(&format!(
            "\n\n... (truncated; use the read tool to load the full \
             {}/memory.md)",
            dir.display()
        ));
        Ok(Some(content))
    } else {
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use robit_ai::config::{AppConfig, MemoryMode, RobitConfig};
    use std::collections::HashMap;
    use std::fs;
    use tempfile::TempDir;

    fn config_with(mode: Option<MemoryMode>, global_storage: bool) -> RobitConfig {
        RobitConfig {
            default_model: None,
            providers: HashMap::new(),
            app: Some(AppConfig {
                memory_mode: mode,
                global_storage: Some(global_storage),
                ..Default::default()
            }),
            channels: None,
            default_image_model: None,
            image_providers: HashMap::new(),
        }
    }

    #[test]
    fn resolve_defaults_to_file_mode_local_dir() {
        let tmp = TempDir::new().unwrap();
        let settings = resolve_memory_settings(&config_with(None, false), tmp.path()).unwrap();
        assert_eq!(settings.mode, MemoryMode::File);
        assert_eq!(settings.dir, tmp.path().join(".robit/memory"));
    }

    #[test]
    fn resolve_global_storage_uses_home() {
        let tmp = TempDir::new().unwrap();
        let settings = resolve_memory_settings(&config_with(None, true), tmp.path()).unwrap();
        let home = dirs::home_dir().unwrap();
        assert_eq!(settings.dir, home.join(".robit/memory"));
    }

    #[test]
    fn section_empty_for_tools_and_off() {
        let tmp = TempDir::new().unwrap();
        for mode in [MemoryMode::Tools, MemoryMode::Off] {
            let settings =
                resolve_memory_settings(&config_with(Some(mode), false), tmp.path()).unwrap();
            assert_eq!(build_memory_section(&settings, "2026-09-30"), "");
        }
    }

    #[test]
    fn file_section_contains_mechanism_explanation() {
        // 记忆文件尚不存在：注入机制说明（含目录路径与当日文件名），无内容分隔符
        let tmp = TempDir::new().unwrap();
        let settings = resolve_memory_settings(
            &config_with(Some(MemoryMode::File), false),
            tmp.path(),
        )
        .unwrap();
        let section = build_memory_section(&settings, "2026-09-30");
        assert!(section.starts_with("## Memory\n"));
        assert!(section.contains(
            &tmp
                .path()
                .join(".robit")
                .join("memory")
                .display()
                .to_string()
        ));
        assert!(section.contains("memory-2026-09-30.md"));
        assert!(!section.contains("---"));
    }

    #[test]
    fn file_section_includes_master_content() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".robit/memory");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("memory.md"), "# 记忆\n\n- 用户使用中文交流\n").unwrap();
        let settings = resolve_memory_settings(
            &config_with(Some(MemoryMode::File), false),
            tmp.path(),
        )
        .unwrap();
        let section = build_memory_section(&settings, "2026-09-30");
        assert!(section.contains("---"));
        assert!(section.contains("用户使用中文交流"));
    }

    #[test]
    fn file_section_truncates_oversized_master_file() {
        // 超过 16KB 且截断点落在多字节字符中间 → 截断不 panic
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".robit/memory");
        fs::create_dir_all(&dir).unwrap();
        let big = "记忆内容测试x".repeat(20000); // 每轮 19 字节 → 约 380KB
        fs::write(dir.join("memory.md"), &big).unwrap();
        let settings = resolve_memory_settings(
            &config_with(Some(MemoryMode::File), false),
            tmp.path(),
        )
        .unwrap();
        let section = build_memory_section(&settings, "2026-09-30");
        assert!(section.contains("truncated"));
        assert!(section.len() < MAX_MEMORY_INJECT_BYTES + 4096);
    }

    #[test]
    fn file_section_degrades_when_master_file_unreadable() {
        // memory.md 被同名目录占用 → 读取失败：不 panic，降级为说明 + 失败提示
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".robit/memory");
        fs::create_dir_all(dir.join("memory.md")).unwrap();
        let settings = resolve_memory_settings(
            &config_with(Some(MemoryMode::File), false),
            tmp.path(),
        )
        .unwrap();
        let section = build_memory_section(&settings, "2026-09-30");
        assert!(section.starts_with("## Memory\n"));
        assert!(section.contains("Failed to read memory.md"));
    }

    #[test]
    fn master_content_placeholders_not_replaced() {
        // 记忆内容含字面占位符文本 → 必须原样保留
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".robit/memory");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("memory.md"), "占位符示例 {memory_dir} {date}").unwrap();
        let settings = resolve_memory_settings(
            &config_with(Some(MemoryMode::File), false),
            tmp.path(),
        )
        .unwrap();
        let section = build_memory_section(&settings, "2026-09-30");
        assert!(section.contains("占位符示例 {memory_dir} {date}"));
    }
}
