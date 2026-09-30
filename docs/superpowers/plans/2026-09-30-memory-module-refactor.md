# 记忆模块重构实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 引入 `[app] memory_mode` 三态配置（`tools` / `file` / `off`，默认 `file`），记忆工具默认关闭；`file` 模式下启用文件记忆机制（`memory.md` + 每日记忆文件），机制说明与主记忆内容注入系统提示词。

**Architecture:** 配置层在 robit-ai 新增 `MemoryMode` 枚举；robit-agent 新增 `memory.rs` 模块负责设置解析与 memory section 文本生成；`PromptBuilder` 增加 `{memory_section}` 占位符替换（最后替换，保护记忆内容中的字面占位符）；`bootstrap` 按 `memory_mode` 条件注册 4 个 SQLite 记忆工具；`Agent::new` / `Agent::with_history` 增加 `MemorySettings` 参数，4 个前端调用点（TUI/GUI/chatbot/examples）各自解析传入。

**Tech Stack:** Rust workspace（robit-ai、robit-agent、robit-tui、robit-gui、robit-chatbot）、serde/TOML 配置、`include_str!` 提示词模板、tempfile（测试）。

**Spec:** `docs/superpowers/specs/2026-09-30-memory-module-refactor-design.md`

## Global Constraints

- 默认值：`memory_mode` 未配置时为 `file`（记忆工具默认关闭）。
- 记忆文件目录与 `robit.db` 同根：默认 `{cwd}/.robit/memory/`，`global_storage = true` 时 `~/.robit/memory/`；框架**不创建**目录/文件。
- 主记忆文件注入上限 16 KB（常量 `MAX_MEMORY_INJECT_BYTES`），超限截断并标注"可用 read 工具读取完整内容"。
- `{memory_section}` 在 `SYSTEM_PROMPT` 的所有占位符替换中**必须最后替换**。
- 读取 `memory.md` 失败不得阻断 Agent 启动：`tracing::warn` + 降级为仅机制说明。
- `tools` 模式下记忆工具保持现有行为（无条件注册，忽略 `enabled_tools` 列表中的记忆工具名）；`file`/`off` 模式下完全不注册，`enabled_tools` 列出记忆工具名时记 `tracing::warn` 后跳过。
- 记忆相关提示词模板（`prompts/memory.md`）与注入系统提示词的运行时文本（截断标注、读取失败提示）一律用**英文**，与现有 `prompts/system.md` 及代码注释风格一致；代码 doc-comment 用英文（与 storage.rs / bootstrap.rs 一致）。
- 提交信息用中文 conventional commits（如 `feat(agent): ...`），结尾加 `Co-Authored-By: Claude Code <noreply@anthropic.com>`。
- Bot 平台（QQ 多会话）共享单份 `memory.md`，不做隔离（MVP 明确非目标）。
- 各 crate 版本用 `version.workspace = true`，本次不发版、不 bump 版本号。

## Review Focus

规格未逐条覆盖、但最可能咬人的五类输入（每条已把测试加进对应任务）：

1. **`memory.md` 内容含字面 `{os}` / `{memory_dir}` / `{date}` 文本** → 必须原样保留，不被占位符替换破坏。测试：Task 3 `master_content_placeholders_not_replaced` + Task 4 的替换顺序注释。
2. **超大且含多字节字符的 `memory.md`（截断点切在 UTF-8 字符中间）** → 截断不 panic、不出错。测试：Task 3 `file_section_truncates_oversized_master_file`（用 `String::from_utf8_lossy`，禁止对切片直接 `String::from_utf8`）。
3. **记忆目录 / `memory.md` 不存在（首次使用）** → 注入"首次记录时创建"的机制说明，而非报错或空节。测试：Task 3 `file_section_contains_mechanism_explanation`。
4. **`memory.md` 不可读（如被同名目录占用、权限问题）** → Agent 启动不阻断，注入失败说明 + `tracing::warn`。测试：Task 3 `file_section_degrades_when_master_file_unreadable`。
5. **`enabled_tools` 显式列出 `memorize` 等记忆工具但 `memory_mode != "tools"`** → 跳过注册并 warn，而不是静默注册或报错退出。测试：Task 5 `memory_tools_in_enabled_tools_list_ignored_in_file_mode`。

---

### Task 1: robit-ai 配置层 — `MemoryMode` 枚举与解析

**Files:**
- Modify: `crates/robit-ai/src/config.rs`（`AppConfig` 结构体，约 176-194 行；`resolve_image_provider` 之后加 helper；文件末尾 `mod tests` 加测试）

**Interfaces:**
- Consumes: 无（首个任务）
- Produces: `robit_ai::config::MemoryMode`（`Tools` / `File` / `Off`，`#[serde(rename_all = "lowercase")]`，derive `Debug, Clone, Copy, PartialEq, Eq, Deserialize`）；`AppConfig.memory_mode: Option<MemoryMode>`；`pub fn resolve_memory_mode(config: &RobitConfig) -> MemoryMode`（未配置返回 `MemoryMode::File`）

- [ ] **Step 1: 写失败测试**

在 `crates/robit-ai/src/config.rs` 末尾的 `mod tests` 中追加（该文件测试已有 `toml::from_str` 解析 TOML 的先例，见 `test_parse_robit_config`）：

```rust
    #[test]
    fn test_memory_mode_all_values() {
        for (mode_str, expected) in [
            ("tools", MemoryMode::Tools),
            ("file", MemoryMode::File),
            ("off", MemoryMode::Off),
        ] {
            let toml_str = format!(
                "[providers.test]\nbase_url = \"https://example.com\"\napi_key = \"k\"\n\n\
                 [[providers.test.models]]\nid = \"m\"\n\n\
                 [app]\nmemory_mode = \"{}\"",
                mode_str
            );
            let config: RobitConfig = toml::from_str(&toml_str).unwrap();
            assert_eq!(resolve_memory_mode(&config), expected);
        }
    }

    #[test]
    fn test_memory_mode_defaults_to_file() {
        // 老用户配置：有 [app] 但没有 memory_mode → 默认 File（记忆工具关闭）
        let toml_str = r#"
            [providers.test]
            base_url = "https://example.com"
            api_key = "k"

            [[providers.test.models]]
            id = "m"

            [app]
            max_steps = 5
        "#;
        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(resolve_memory_mode(&config), MemoryMode::File);
    }

    #[test]
    fn test_memory_mode_invalid_rejected() {
        let toml_str = r#"
            [providers.test]
            base_url = "https://example.com"
            api_key = "k"

            [[providers.test.models]]
            id = "m"

            [app]
            memory_mode = "bogus"
        "#;
        assert!(toml::from_str::<RobitConfig>(toml_str).is_err());
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p robit-ai --lib memory`
Expected: 编译失败，`MemoryMode` / `resolve_memory_mode` 未定义。

- [ ] **Step 3: 写最小实现**

在 `config.rs` 的 `AppConfig` 结构体定义之后（约 194 行 `}` 后）追加：

```rust
/// `[app] memory_mode` 长期记忆机制选择器。
///
/// - `tools`：注册 SQLite 记忆工具（memorize/recall/forget/list_memories）。
/// - `file`：文件记忆机制——记忆目录下的 `memory.md` 与每日文件
///   `memory-YYYY-MM-DD.md`，主记忆内容注入系统提示词。
/// - `off`：不启用任何记忆机制。
///
/// 默认 `file`（记忆工具默认关闭）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryMode {
    Tools,
    File,
    Off,
}
```

在 `AppConfig` 结构体内（`pub bot: Option<BotConfig>,` 之前）加字段：

```rust
    /// 长期记忆机制（默认 file）。
    pub memory_mode: Option<MemoryMode>,
```

在 `resolve_image_provider` 函数之后追加：

```rust
/// 解析生效的记忆模式。未配置时返回 `File`
/// （记忆工具默认关闭，文件记忆机制默认启用）。
pub fn resolve_memory_mode(config: &RobitConfig) -> MemoryMode {
    config
        .app
        .as_ref()
        .and_then(|a| a.memory_mode)
        .unwrap_or(MemoryMode::File)
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p robit-ai --lib`
Expected: 全部 PASS（含既有测试，确认 `AppConfig` 新字段不破坏旧解析）。

- [ ] **Step 5: 提交**

```bash
git add crates/robit-ai/src/config.rs
git commit -m "feat(ai): 新增 memory_mode 三态配置（tools/file/off，默认 file）

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 2: robit-agent — `resolve_memory_dir` 记忆目录解析

**Files:**
- Modify: `crates/robit-agent/src/storage.rs`（`resolve_db_path`，21-30 行，重构为共用推导；文件末尾 `mod tests` 加测试）

**Interfaces:**
- Consumes: 无
- Produces: `crate::storage::resolve_memory_dir(working_dir: &Path, global_storage: bool) -> Result<PathBuf>`（`Result` 即 `crate::error::Result`），返回 `{cwd}/.robit/memory/` 或 `~/.robit/memory/`，**不创建目录**

- [ ] **Step 1: 写失败测试**

在 `storage.rs` 末尾 `mod tests` 中追加（紧邻既有的 `resolves_local_db_path` 测试，风格一致）：

```rust
    #[test]
    fn resolves_memory_dir_local() {
        let working_dir = PathBuf::from("/tmp/project");
        let dir = resolve_memory_dir(&working_dir, false).unwrap();
        assert_eq!(dir, PathBuf::from("/tmp/project/.robit/memory"));
    }

    #[test]
    fn resolves_memory_dir_is_db_path_parent() {
        // 记忆目录必须与 robit.db 同根（db 就在记忆目录下）
        let working_dir = PathBuf::from("/tmp/project");
        let db_path = resolve_db_path(&working_dir, false).unwrap();
        let dir = resolve_memory_dir(&working_dir, false).unwrap();
        assert_eq!(db_path.parent().unwrap(), dir.as_path());
    }

    #[test]
    fn resolves_memory_dir_global() {
        let working_dir = PathBuf::from("/tmp/project");
        let dir = resolve_memory_dir(&working_dir, true).unwrap();
        let home = dirs::home_dir().unwrap();
        assert_eq!(dir, home.join(".robit/memory"));
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p robit-agent --lib resolves_memory_dir`
Expected: 编译失败，`resolve_memory_dir` 未定义。

- [ ] **Step 3: 写最小实现**

将 `storage.rs` 21-30 行的 `resolve_db_path` 替换为：

```rust
/// Resolve the session database path for a working directory and storage scope.
pub fn resolve_db_path(working_dir: &Path, global_storage: bool) -> Result<PathBuf> {
    Ok(resolve_memory_dir(working_dir, global_storage)?.join(DB_FILE))
}

/// Resolve the memory directory for a working directory and storage scope.
///
/// Same root derivation as [`resolve_db_path`]: `{working_dir}/.robit/memory/`
/// by default, `~/.robit/memory/` when `global_storage` is enabled. Does NOT
/// create the directory — memory files (`memory.md` and daily
/// `memory-YYYY-MM-DD.md`) are created by the agent itself via the `write`
/// tool on first use.
pub fn resolve_memory_dir(working_dir: &Path, global_storage: bool) -> Result<PathBuf> {
    if global_storage {
        let home = dirs::home_dir().ok_or_else(|| {
            crate::error::AgentError::InternalError("Cannot determine home directory".to_string())
        })?;
        Ok(home.join(ROBIT_DIR).join(MEMORY_DIR))
    } else {
        Ok(working_dir.join(ROBIT_DIR).join(MEMORY_DIR))
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p robit-agent --lib storage`
Expected: 全部 PASS（含既有 `resolves_local_db_path`，确认重构未破坏 db 路径解析）。

- [ ] **Step 5: 提交**

```bash
git add crates/robit-agent/src/storage.rs
git commit -m "refactor(agent): 提取 resolve_memory_dir 记忆目录解析

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 3: robit-agent — `memory.rs` 模块与机制说明模板

**Files:**
- Create: `crates/robit-agent/src/memory.rs`
- Create: `crates/robit-agent/prompts/memory.md`
- Modify: `crates/robit-agent/src/lib.rs`（模块声明与 re-export）

**Interfaces:**
- Consumes: `robit_ai::config::{resolve_memory_mode, MemoryMode, RobitConfig}`（Task 1）；`crate::storage::resolve_memory_dir`（Task 2）
- Produces:
  - `pub struct MemorySettings { pub mode: MemoryMode, pub dir: std::path::PathBuf }`
  - `pub fn resolve_memory_settings(config: &RobitConfig, working_dir: &Path) -> crate::error::Result<MemorySettings>`
  - `pub fn build_memory_section(settings: &MemorySettings, date: &str) -> String`（`tools`/`off` 返回空串；`file` 返回自带 `## Memory` 标题的节文本）
  - `pub const MAX_MEMORY_INJECT_BYTES: usize`（16 * 1024）
  - 模板 `prompts/memory.md`，占位符 `{memory_dir}`、`{date}`

- [ ] **Step 1: 创建提示词模板 `crates/robit-agent/prompts/memory.md`**

```markdown
You have a file-based memory mechanism. Memories are stored as Markdown files in `{memory_dir}/`:

- **Master memory file** `{memory_dir}/memory.md`: information worth keeping long-term (user preferences, key facts, core project knowledge). Its content is automatically injected into your context at the start of every session — keep it concise, deduplicated, and maintainable.
- **Daily memory file** `{memory_dir}/memory-YYYY-MM-DD.md`: the current day's work process and temporary context. Today's file is `{memory_dir}/memory-{date}.md`. To review a past day, use the `read` tool to open the file for that date.

Usage rules:

1. Create memory files with the `write` tool when they do not exist; update existing files with the `edit` tool instead of rewriting them whole.
2. When the user asks you to "remember" something, or you judge that information will matter in future sessions, write it to the appropriate memory file; proactively clean up outdated or redundant entries.
3. Information with long-term value goes into `memory.md`; same-day process notes go into the daily file.
```

- [ ] **Step 2: 写失败测试**

创建 `crates/robit-agent/src/memory.rs`，先只写测试骨架（实现部分留待 Step 4，此时文件顶部暂不 `include_str!` 会导致编译错，属预期）：

```rust
//! File-based memory mechanism.
//!
//! When the SQLite memory tools are disabled (`memory_mode = "file"`), the
//! agent keeps long-term memory in Markdown files under the memory
//! directory: a master `memory.md` plus daily `memory-YYYY-MM-DD.md` files.
//! This module resolves the memory settings and builds the
//! `{memory_section}` injected into the system prompt: the mechanism
//! explanation plus the current master file content (bounded by
//! [`MAX_MEMORY_INJECT_BYTES`], truncated beyond that).

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
                .join(".robit/memory")
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
```

同时在 `crates/robit-agent/src/lib.rs` 模块声明中（`pub mod media;` 与 `pub mod prompt;` 之间）加：

```rust
pub mod memory;
```

- [ ] **Step 3: 运行测试确认失败**

Run: `cargo test -p robit-agent --lib memory`
Expected: 编译失败，`resolve_memory_settings` / `build_memory_section` / `MAX_MEMORY_INJECT_BYTES` 未定义。

- [ ] **Step 4: 写最小实现**

在 `memory.rs` 顶部（`#[cfg(test)]` 之前）补上实现：

```rust
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
```

在 `lib.rs` 的 re-export 区（`pub use frontend::...` 一带，按现有分组风格）追加：

```rust
pub use memory::{build_memory_section, resolve_memory_settings, MemorySettings, MAX_MEMORY_INJECT_BYTES};
```

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test -p robit-agent --lib`
Expected: 全部 PASS（含既有 storage/skill/tool 测试）。

- [ ] **Step 6: 提交**

```bash
git add crates/robit-agent/src/memory.rs crates/robit-agent/prompts/memory.md crates/robit-agent/src/lib.rs
git commit -m "feat(agent): 文件记忆模块 memory.rs 与机制说明模板

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 4: 接线全链路 — 提示词占位符、Agent 签名与 4 个前端调用点

本任务一次完成接口贯通（`build_system_prompt` 签名变更 + `Agent` 构造函数新参数 + 全部调用点），否则中途工作区无法编译。每步仍是小改动。

**Files:**
- Modify: `crates/robit-agent/prompts/system.md`
- Modify: `crates/robit-agent/src/prompt.rs`（`build_system_prompt`，68-90 行；文件末尾新增 `mod tests`）
- Modify: `crates/robit-agent/src/agent.rs`（`Agent::new` 146-192 行、`Agent::with_history` 195 起，两处 `build_system_prompt` 调用）
- Modify: `crates/robit-tui/src/main.rs`（110-120 行）
- Modify: `crates/robit-gui/src/state.rs`（`spawn_agent`，261-273 行）
- Modify: `crates/robit-chatbot/src/manager.rs`（`spawn_session_agent`，644-664 行）
- Modify: `examples/robit-agent/src/main.rs`（89-99 行）

**Interfaces:**
- Consumes: Task 3 的 `MemorySettings` / `resolve_memory_settings` / `build_memory_section`
- Produces: `PromptBuilder::build_system_prompt(&self, skills: &[(&str, &str)], working_dir: &Path, memory_settings: &MemorySettings) -> String`；`Agent::new(..., extensions: HashMap<...>, memory: MemorySettings) -> Self` 与 `Agent::with_history(..., history: Vec<ChatCompletionRequestMessage>, memory: MemorySettings) -> Self`（`memory` 均为最后一个参数）

- [ ] **Step 1: 写失败测试**

`prompt.rs` 末尾新增测试模块（该文件此前无测试）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::resolve_memory_settings;
    use robit_ai::config::{AppConfig, MemoryMode, RobitConfig};
    use std::collections::HashMap;

    fn config_with_memory_mode(mode: MemoryMode) -> RobitConfig {
        RobitConfig {
            default_model: None,
            providers: HashMap::new(),
            app: Some(AppConfig {
                memory_mode: Some(mode),
                ..Default::default()
            }),
            channels: None,
            default_image_model: None,
            image_providers: HashMap::new(),
        }
    }

    #[test]
    fn system_prompt_contains_memory_section_in_file_mode() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join(".robit/memory");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("memory.md"), "- 用户偏好：深色主题\n").unwrap();
        let settings =
            resolve_memory_settings(&config_with_memory_mode(MemoryMode::File), tmp.path())
                .unwrap();
        let prompt = PromptBuilder::new().build_system_prompt(&[], tmp.path(), &settings);
        assert!(prompt.contains("## Memory"));
        assert!(prompt.contains("用户偏好：深色主题"));
        assert!(prompt.contains("## Environment"));
    }

    #[test]
    fn system_prompt_has_no_memory_section_in_tools_mode() {
        let tmp = tempfile::TempDir::new().unwrap();
        let settings =
            resolve_memory_settings(&config_with_memory_mode(MemoryMode::Tools), tmp.path())
                .unwrap();
        let prompt = PromptBuilder::new().build_system_prompt(&[], tmp.path(), &settings);
        assert!(!prompt.contains("## Memory"));
        assert!(prompt.contains("## Environment"));
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p robit-agent --lib prompt`
Expected: 编译失败——`build_system_prompt` 只有 2 个参数，测试传了 3 个。

- [ ] **Step 3: 修改 `prompts/system.md`**

将文件整体替换为（原内容保持不变，只在末尾追加 `{memory_section}`）：

```markdown
## Available Skills

{skills_section}

> - Skill-related environment variable configuration can be done in the `.robit/.env` file in the working directory, using the `KEY=VALUE` format.

## Environment

- Operating System: {os}
- Working Directory: {cwd}
- Current Date: {date}

{memory_section}
```

- [ ] **Step 4: 修改 `prompt.rs` 的 `build_system_prompt`**

函数签名与函数体替换为：

```rust
    pub fn build_system_prompt(
        &self,
        skills: &[(&str, &str)],
        working_dir: &std::path::Path,
        memory_settings: &crate::memory::MemorySettings,
    ) -> String {
        let skills_section = Self::build_skills_section(skills);
        let os = std::env::consts::OS;
        let cwd = working_dir.display().to_string();
        let date = current_date();

        // Select base prompt: custom agent prompt or default agent prompt
        let agent_prompt = self.custom_prompt.as_deref().unwrap_or(DEFAULT_AGENT_PROMPT);

        let memory_section = crate::memory::build_memory_section(memory_settings, &date);

        // Replace variables in the system prompt (Skills, Environment, Memory).
        // NOTE: `{memory_section}` must be replaced LAST — the injected memory
        // content may legitimately contain literal `{os}` / `{memory_dir}` /
        // ... text that must survive verbatim.
        let system_part = SYSTEM_PROMPT
            .replace("{os}", os)
            .replace("{cwd}", &cwd)
            .replace("{date}", &date)
            .replace("{skills_section}", &skills_section)
            .replace("{memory_section}", &memory_section)
            .trim_end()
            .to_string();

        // Combine: agent prompt + system prompt
        format!("{}\n\n{}", agent_prompt.trim(), system_part)
    }
```

其余内容（`PromptBuilder` 结构、`with_working_dir`、`build_skills_section`、常量定义与文档注释）不变；顺带把文件头部的函数级文档注释中"The prompt is composed of"清单补上 Memory 一项。

- [ ] **Step 5: 修改 `agent.rs` 两个构造函数**

在 `agent.rs` 顶部导入区追加：

```rust
use crate::memory::MemorySettings;
```

`Agent::new` 与 `Agent::with_history` 的参数列表**末尾**各加一个参数（放在 `extensions` / `history` 之后）：

```rust
        memory: MemorySettings,
```

两个构造函数中的系统提示词调用（约 163、219 行）均改为：

```rust
        let system_prompt =
            prompt_builder.build_system_prompt(&skill_descs, &working_dir, &memory);
```

构造函数 doc-comment 中"Build system prompt with skills"一行补充记忆说明：`// Build system prompt with skills + memory section.`

- [ ] **Step 6: 修改 4 个前端调用点**

**TUI `crates/robit-tui/src/main.rs`**（在 `let bootstrap_result = ...` 与 `log_skill_errors(...)` 之后、`Agent::new` 之前插入解析，并把 `memory` 作为最后一个参数传入）：

```rust
    let memory = robit_agent::memory::resolve_memory_settings(&config, &working_dir)?;
```

```rust
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
        memory,
    );
```

（若 `?` 因 main 的错误类型不接受 `AgentError` 而编译失败，参照同文件 81 行 `LlmClient::from_config(&config, None)?` 的错误处理方式适配——两者都实现了 `std::error::Error`，通常无需改动。）

**GUI `crates/robit-gui/src/state.rs`**（`spawn_agent` 内、`Agent::with_history` 调用之前插入）：

```rust
        let memory = robit_agent::memory::resolve_memory_settings(&self.config, &self.working_dir)
            .map_err(|e| format!("Failed to resolve memory settings: {}", e))?;
```

`Agent::with_history(...)` 参数末尾追加 `memory,`（`history_messages` 之后）。

**chatbot `crates/robit-chatbot/src/manager.rs`**（`spawn_session_agent` 内、`Agent::with_history` 调用之前插入；函数返回 `Result<_, AgentError>`，`resolve_memory_settings` 的错误即 `AgentError`，直接 `?`）：

```rust
        let memory = robit_agent::memory::resolve_memory_settings(&self.config, &self.working_dir)?;
```

`Agent::with_history(...)` 参数末尾追加 `memory,`（`history_messages` 之后）。

**examples `examples/robit-agent/src/main.rs`**（`Agent::new` 之前插入）：

```rust
    let memory = robit_agent::memory::resolve_memory_settings(&config, &working_dir)?;
```

`Agent::new(...)` 参数末尾追加 `memory,`（`std::collections::HashMap::new()` 之后）。

- [ ] **Step 7: 编译与测试验证**

Run: `cargo test -p robit-agent --lib`
Expected: 全部 PASS（含 Step 1 的两个新测试）。

Run: `cargo check --workspace --all-targets`
Expected: 全 workspace 编译通过（TUI、GUI、chatbot、examples 四个调用点全部接上新参数）。

- [ ] **Step 8: 提交**

```bash
git add crates/robit-agent/prompts/system.md crates/robit-agent/src/prompt.rs crates/robit-agent/src/agent.rs crates/robit-tui/src/main.rs crates/robit-gui/src/state.rs crates/robit-chatbot/src/manager.rs examples/robit-agent/src/main.rs
git commit -m "feat(agent): 系统提示词接入 memory section，Agent 构造传入记忆设置

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 5: bootstrap — 记忆工具按 `memory_mode` 条件注册

**Files:**
- Modify: `crates/robit-agent/src/bootstrap.rs`（`create_tools_from_config`，106-198 行；文件末尾新增 `mod tests`）

**Interfaces:**
- Consumes: `robit_ai::config::{resolve_memory_mode, MemoryMode}`（Task 1）
- Produces: 无新接口；行为变更——`tools` 模式注册 4 个记忆工具，`file`/`off` 模式不注册

- [ ] **Step 1: 写失败测试**

`bootstrap.rs` 末尾新增（该文件此前无测试）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use robit_ai::config::{AppConfig, MemoryMode, RobitConfig};
    use std::collections::HashMap;

    const MEMORY_TOOL_NAMES: [&str; 4] = ["memorize", "recall", "forget", "list_memories"];

    fn config_with_memory_mode(mode: Option<MemoryMode>) -> RobitConfig {
        RobitConfig {
            default_model: None,
            providers: HashMap::new(),
            app: Some(AppConfig {
                memory_mode: mode,
                ..Default::default()
            }),
            channels: None,
            default_image_model: None,
            image_providers: HashMap::new(),
        }
    }

    fn registry_for(config: &RobitConfig) -> ToolRegistry {
        let skills = Arc::new(SkillRegistry::new(Vec::new(), &[]));
        create_tools_from_config(config, skills)
    }

    #[test]
    fn memory_tools_registered_in_tools_mode() {
        let registry = registry_for(&config_with_memory_mode(Some(MemoryMode::Tools)));
        let names = registry.tool_names();
        for tool in MEMORY_TOOL_NAMES {
            assert!(names.contains(&tool), "missing {}", tool);
        }
    }

    #[test]
    fn memory_tools_not_registered_in_file_mode() {
        let registry = registry_for(&config_with_memory_mode(Some(MemoryMode::File)));
        let names = registry.tool_names();
        for tool in MEMORY_TOOL_NAMES {
            assert!(!names.contains(&tool), "{} should not be registered", tool);
        }
        assert!(names.contains(&"read"));
    }

    #[test]
    fn memory_tools_not_registered_by_default() {
        // 未配置 memory_mode（老用户升级路径）→ 默认 file，不注册记忆工具
        let registry = registry_for(&config_with_memory_mode(None));
        let names = registry.tool_names();
        for tool in MEMORY_TOOL_NAMES {
            assert!(!names.contains(&tool));
        }
    }

    #[test]
    fn memory_tools_not_registered_in_off_mode() {
        let registry = registry_for(&config_with_memory_mode(Some(MemoryMode::Off)));
        for tool in MEMORY_TOOL_NAMES {
            assert!(!registry.tool_names().contains(&tool));
        }
    }

    #[test]
    fn memory_tools_in_enabled_tools_list_ignored_in_file_mode() {
        // enabled_tools 显式列出记忆工具但模式非 tools → 跳过注册（warn），不报错
        let mut config = config_with_memory_mode(Some(MemoryMode::File));
        config.app.as_mut().unwrap().enabled_tools =
            Some(vec!["read".into(), "memorize".into(), "recall".into()]);
        let registry = registry_for(&config);
        let names = registry.tool_names();
        assert!(names.contains(&"read"));
        for tool in MEMORY_TOOL_NAMES {
            assert!(!names.contains(&tool));
        }
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p robit-agent --lib bootstrap`
Expected: `memory_tools_not_registered_in_file_mode` / `..._by_default` / `..._in_off_mode` / `..._ignored_in_file_mode` FAIL（当前记忆工具无条件注册），`memory_tools_registered_in_tools_mode` PASS。

- [ ] **Step 3: 写最小实现**

`bootstrap.rs` 顶部导入改为：

```rust
use robit_ai::config::{
    resolve_image_provider, resolve_memory_mode, resolve_profile, MemoryMode, RobitConfig,
};
```

`create_tools_from_config` 的 doc-comment 中这行：

```rust
/// - `read`, `load_skill`, and memory tools are always registered (required for basic functionality)
```

改为：

```rust
/// - `read` and `load_skill` are always registered (required for basic functionality)
/// - Memory tools (`memorize`/`recall`/`forget`/`list_memories`) are registered
///   only when `memory_mode = "tools"`; `file`/`off` modes skip them entirely.
```

在函数体内（`let supports_images = ...` 之前）加：

```rust
    // Memory tools are only registered in "tools" mode. The default is
    // "file" (file-based memory), where they are entirely absent from the
    // ToolRegistry and thus invisible to the LLM.
    let memory_tools_enabled = resolve_memory_mode(config) == MemoryMode::Tools;
```

无条件注册区（132-144 行）改为：

```rust
    // Always register read, load_skill, history, and query_task tools
    // (required for basic functionality / async task visibility)
    tools.register(ReadTool::new(
        max_lines,
        max_bytes,
        supports_images,
        max_image_dimension,
    ));
    tools.register(LoadSkillTool::new(skill_registry));
    if memory_tools_enabled {
        tools.register(MemorizeTool::new());
        tools.register(RecallTool::new());
        tools.register(ForgetTool::new());
        tools.register(ListMemoriesTool::new());
    }
    tools.register(SearchHistoryTool::new());
    tools.register(QueryTaskTool::new());
```

`enabled_tools` Some(list) 分支中，把 4 个记忆工具的 4 个 arm：

```rust
                    "memorize" => {} // already registered
                    "recall" => {} // already registered
                    "forget" => {} // already registered
                    "list_memories" => {} // already registered
```

合并替换为：

```rust
                    "memorize" | "recall" | "forget" | "list_memories" => {
                        // Only effective in "tools" mode; skipped otherwise.
                        if !memory_tools_enabled {
                            tracing::warn!(
                                "memory tool {} listed in enabled_tools but memory_mode \
                                 != \"tools\", skipping",
                                tool_name
                            );
                        }
                    }
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p robit-agent --lib`
Expected: 全部 PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/robit-agent/src/bootstrap.rs
git commit -m "feat(agent): 记忆工具按 memory_mode 条件注册（默认不注册）

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 6: 文档 — CLAUDE.md、architecture.md、CHANGELOG.md

**Files:**
- Modify: `CLAUDE.md`
- Modify: `docs/architecture.md`
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: Task 1-5 的最终行为
- Produces: 无代码接口；用户可见文档

- [ ] **Step 1: 更新 `CLAUDE.md`**

三处编辑：

（a）工具系统表中，`ls` 行之后追加一行：

```markdown
| `memorize` / `recall` / `forget` / `list_memories` | SQLite 长期记忆工具，仅在 `memory_mode = "tools"` 时注册 | 否（默认关闭） | 仅 `forget` 需确认 |
```

（b）config.toml 结构示例的 `[app]` 段中，`global_storage = false` 行之后追加：

```toml
memory_mode = "file"                   # 可选，记忆机制：tools（SQLite 记忆工具）/ file（文件记忆：memory.md + 每日文件，主记忆注入系统提示词）/ off，默认 file
```

（c）"配置目录结构"代码块中，两处 `memory/robit.db` 注释补充记忆文件说明：

```text
    |--项目本地：.robit/memory/robit.db # 默认 GUI 会话数据库；memory_mode = "file" 时同目录存放记忆文件（memory.md、memory-YYYY-MM-DD.md，由 Agent 维护）
```

与全局段：

```text
    |   |-- memory/robit.db           # 启用 global_storage 后的 GUI 会话数据库；同上，memory_mode = "file" 时存放记忆文件
```

- [ ] **Step 2: 更新 `docs/architecture.md`**

在 `enabled_tools` 小节（约 308-317 行，"**配置 `enabled_tools`**：只启用列表中指定的工具"之后）追加：

```markdown
### 记忆机制（memory_mode）

`[app] memory_mode` 控制长期记忆机制，默认 `file`：

- `tools`：注册 SQLite 记忆工具（`memorize` / `recall` / `forget` / `list_memories`），数据存于 `robit.db` 的 `memories` 表。
- `file`（默认）：文件记忆机制。记忆文件存于记忆目录（`.robit/memory/`，`global_storage` 时 `~/.robit/memory/`）：主记忆文件 `memory.md` 与每日记忆文件 `memory-YYYY-MM-DD.md` 均由 Agent 通过 `write` / `edit` 工具按系统提示词指引维护；系统提示词中注入机制说明，`memory.md` 内容（上限 16KB，超限截断）自动拼入系统提示词。会话中途修改 `memory.md` 不热刷新，重启或新会话生效。
- `off`：不启用任何记忆机制。

Bot 平台（QQ 多会话）下所有聊天共享同一份 `memory.md`，不做按用户隔离。
```

另在 `architecture.md` 中 DB 路径一节（约 704 行 `DB 路径：cwd/.robit/memory/robit.db ...`）末尾追加一句：

```markdown
- 同目录（`.robit/memory/`）在 `memory_mode = "file"` 时存放记忆文件：`memory.md`（主记忆，注入系统提示词）与 `memory-YYYY-MM-DD.md`（每日记忆）
```

- [ ] **Step 3: 更新 `CHANGELOG.md`**

`## [Unreleased]` 标题下追加：

```markdown
### Changed

- **robit-agent / robit-ai**：记忆机制重构。新增 `[app] memory_mode` 配置（`tools` / `file` / `off`），**默认 `file`**——未配置的用户升级后 SQLite 记忆工具（`memorize` / `recall` / `forget` / `list_memories`）不再注册，自动切换为文件记忆机制：记忆目录（`.robit/memory/`，启用 `global_storage` 时为 `~/.robit/memory/`）下由 Agent 通过 `write` / `edit` 工具维护主记忆文件 `memory.md` 与每日记忆文件 `memory-YYYY-MM-DD.md`，机制说明与主记忆内容（上限 16KB，超限截断）随系统提示词注入上下文；`memory.md` 内容中的字面占位符文本不受模板替换影响。需要原 SQLite 记忆工具的用户请显式配置 `memory_mode = "tools"`；已有 `memories` 表数据不迁移、不删除。Bot 平台各聊天共享同一份 `memory.md`。
```

- [ ] **Step 4: 全量回归验证**

Run: `cargo check --workspace --all-targets`
Expected: 编译通过。

Run: `cargo test --workspace --lib`
Expected: 全部 PASS。

- [ ] **Step 5: 提交**

```bash
git add CLAUDE.md docs/architecture.md CHANGELOG.md
git commit -m "docs: 记忆模块重构文档（CLAUDE.md/architecture.md/CHANGELOG）

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```
