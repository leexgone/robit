# 记忆模块重构设计规格

**版本**: 1.0
**日期**: 2026-09-30
**状态**: 设计中
**前置文档**: [2026-07-18-memory-tools-design.md](2026-07-18-memory-tools-design.md)（SQLite 记忆工具系统）

## 1. 背景与动机

当前记忆能力由 4 个 SQLite 工具（`memorize` / `recall` / `forget` / `list_memories`）提供，在 `bootstrap.rs` 中**无条件注册**，不受 `enabled_tools` 配置控制，数据存于 `robit.db` 的 `memories` 表。

存在的问题：

- 记忆工具无法关闭，即使用户不需要结构化记忆能力，也会占用 LLM 的 `tools` 参数空间并干扰工具选择。
- 没有轻量、透明、可直接编辑的记忆机制——SQLite 中的记忆对用户不可见、不可手工维护。

本次重构引入**文件记忆机制**作为替代：记忆以 Markdown 文件形式存放于记忆目录，主记忆文件内容直接拼装进系统提示词，用户可以直接查看和编辑。

## 2. 需求

1. 为记忆工具增加是否启用的配置选项。
2. 记忆工具未启用时，智能体改用文件记忆机制：
   - 在记忆目录下使用主记忆文件 `memory.md` 和每日记忆文件 `memory-YYYY-MM-DD.md`；
   - 系统提示词中拼接记忆机制说明内容；
   - 主记忆文件内容拼装到智能体上下文中。
3. 记忆工具默认配置为关闭。

## 3. 已确认的设计决策

| 决策点 | 结论 |
|--------|------|
| 记忆文件目录 | 跟随 `robit.db` 位置：默认 `.robit/memory/`，启用 `global_storage` 时为 `~/.robit/memory/` |
| 配置形态 | 三态模式 `[app] memory_mode = "tools" \| "file" \| "off"`，默认 `file` |
| 文件创建 | 全部由 Agent 创建——框架只注入说明和内容，不主动建文件 |
| Bot 多会话隔离 | 共享单份 `memory.md`，不按用户/会话隔离（MVP） |
| 注入机制 | 系统提示词注入（Agent 构建时读取拼入），而非独立系统消息或工具化读取 |

## 4. 配置层（robit-ai/src/config.rs）

### 4.1 新增枚举与字段

```rust
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryMode {
    Tools, // 注册 SQLite 记忆工具
    File,  // 文件记忆机制（默认）
    Off,   // 完全关闭记忆
}

pub struct AppConfig {
    // ...现有字段...
    pub memory_mode: Option<MemoryMode>,
}
```

### 4.2 配置示例

```toml
[app]
memory_mode = "file"   # tools | file | off，默认 file（记忆工具默认关闭）
```

### 4.3 解析 helper

```rust
/// 返回生效的记忆模式，未配置时默认 File。
pub fn resolve_memory_mode(config: &RobitConfig) -> MemoryMode
```

### 4.4 模式语义

| 模式 | SQLite 记忆工具 | 文件记忆机制 |
|------|-----------------|--------------|
| `tools` | 注册（现有 `enabled_tools` 交互行为不变） | 关闭 |
| `file`（默认） | 不注册 | 启用 |
| `off` | 不注册 | 关闭 |

## 5. 记忆目录与设置解析（robit-agent）

### 5.1 记忆目录解析

`storage.rs` 新增：

```rust
/// 返回记忆目录（与 resolve_db_path 同源逻辑）。
/// 默认 {cwd}/.robit/memory/，global_storage 时 ~/.robit/memory/。
/// 不创建目录——记忆文件由 Agent 按提示词指引用 write 工具创建。
pub fn resolve_memory_dir(working_dir: &Path, global_storage: bool) -> Result<PathBuf>
```

与 `resolve_db_path` 保持同一套根目录推导，避免两处逻辑漂移（实现时可提取共享的根目录推导函数）。

### 5.2 新模块 `src/memory.rs`

```rust
pub struct MemorySettings {
    pub mode: MemoryMode,
    pub dir: PathBuf,
}

/// 从配置解析记忆设置。前端（TUI/GUI/chatbot/examples）构造 Agent 时调用。
pub fn resolve_memory_settings(
    config: &RobitConfig,
    working_dir: &Path,
) -> Result<MemorySettings>
```

`memory.rs` 同时负责：

- 读取 `{dir}/memory.md`（带大小上限，见 §8）；
- 生成拼入系统提示词的 memory section 文本（见 §7）。

## 6. 工具注册（robit-agent/src/bootstrap.rs）

现状（`create_tools_from_config`）：4 个记忆工具与 `read` / `load_skill` / `search_history` / `query_task` 同属"无条件注册"集合。

变更后：

- **`Tools` 模式**：4 个记忆工具保持现有行为（无条件注册；`enabled_tools` 列表中的 `memorize` 等条目被忽略）。
- **`File` / `Off` 模式**：4 个记忆工具完全不注册——不进 `ToolRegistry`，LLM 的 `tools` 请求参数中不可见；`enabled_tools` 中若出现记忆工具名，记录 `tracing::warn` 后忽略。

### 6.1 指令与工具展示入口（无需改动，仅结论）

经排查，当前各前端**不存在**记忆专属聊天指令，禁用记忆工具后无需屏蔽任何指令响应：

- TUI 指令集（`/exit`、`/clear`、`/model`、`/tools`、`/scroll`、`/skills`）与 chatbot 指令集（`/clear`、`/stop`、`/cancel`、`/new`、`/list`、`/switch`、`/help`）均无记忆条目；GUI 无斜杠指令。
- TUI `/tools` 打印的是 `ToolRegistry` 实时工具列表，记忆工具不注册后自动不再显示，无需额外处理。
- chatbot `/help` 为静态文本，不含记忆条目，无需修改。

## 7. 提示词拼装

### 7.1 prompts/system.md

在 Environment 之后新增单一占位符 `{memory_section}`。**节标题由 memory section 文本自带**（非空时以 `## Memory` 开头），避免 `tools` / `off` 模式下残留空节标题：

```markdown
{memory_section}
```

### 7.2 新模板 `prompts/memory.md`（include_str!）

文件记忆机制说明，含变量 `{memory_dir}`、`{date}`。**模板与注入系统提示词的运行时文本（截断标注、读取失败提示）一律用英文**，与现有 `prompts/system.md`、`prompts/default.md` 的提示词语言保持一致。要点：

1. **目录与文件**：记忆目录为 `{memory_dir}`；主记忆文件 `memory.md`（持久事实、用户偏好、项目关键知识）；每日记忆文件 `memory-YYYY-MM-DD.md`（当日工作过程、临时上下文，按 `{date}` 推导当日文件名）。
2. **创建与写入指引**：文件不存在时用 `write` 工具创建；更新用 `edit` 工具。
3. **分工指引**：值得长期保留的信息进 `memory.md`（精炼、去重、可维护）；当天会话的过程记录进每日文件；`memory.md` 内容会在每次会话开始时自动注入上下文，应保持精简。
4. **每日文件回顾**：需要历史过程时可用 `read` 工具读取指定日期的每日文件。

### 7.3 memory section 的三种形态

| 模式 | `{memory_section}` 内容 |
|------|--------------------------|
| `tools` | 空（记忆工具已通过 function-calling `tools` 参数暴露，与现状一致） |
| `file` | `prompts/memory.md` 渲染后的机制说明 + `\n\n---\n\n` + `memory.md` 实际内容（文件存在时） |
| `file`（文件不存在） | 仅机制说明，含"首次记录时创建 `memory.md`"指引 |
| `off` | 空（占位符替换为空串，无 Memory 节） |

### 7.4 PromptBuilder 与 Agent 接线

- `PromptBuilder::build_system_prompt` 增加 memory 相关参数（`MemorySettings` 或等价信息）。
- `Agent::new` / `Agent::with_history` 增加 `memory: MemorySettings` 参数，构造系统提示词时传入。
- 系统提示词在 Agent 构造时构建一次；会话中途更新 `memory.md` 不会热刷新，重启或新会话生效（与 `{cwd}`、`{date}` 等变量同一生命周期，可接受）。
- 前端调用点（均已持有完整 `RobitConfig`）各自调用 `resolve_memory_settings` 后传入：
  - `crates/robit-tui/src/main.rs`
  - `crates/robit-gui/src/state.rs`
  - `crates/robit-chatbot/src/manager.rs`
  - `examples/robit-agent/src/main.rs`
- Bot 平台（QQ 多会话）共享单份 `memory.md`，不做隔离。

## 8. 大小上限与错误处理

- **注入上限**：`memory.md` 内容注入上限 **16 KB**（MVP 固定常量 `MAX_MEMORY_INJECT_BYTES`）。超限时截断并在末尾标注：

  ```text
  ... (truncated; use the read tool to load the full {memory_dir}/memory.md)
  ```

- **读取失败**（权限/IO 错误）：`tracing::warn` 记录，注入机制说明及失败提示（`Failed to read memory.md: ...`），**不阻断 Agent 启动**。
- **目录解析失败**：同上，降级为不含 `{memory_dir}` 具体路径的说明文本。

## 9. 测试计划

| 层 | 测试 |
|----|------|
| config（robit-ai） | `memory_mode` 三态解析正确；未配置时 `resolve_memory_mode` 返回 `File`；非法值报错 |
| storage | `resolve_memory_dir` 本地/全局两分支；与 `resolve_db_path` 根目录一致 |
| memory.rs | 生成三种模式的 section 文本；文件缺失时含指引；超限截断带标注；读取失败降级 |
| bootstrap | `tools` 模式注册 4 个记忆工具；`file`/`off` 模式不注册；`file` 模式下 `enabled_tools` 列出记忆工具名时仅 warn |
| prompt | `file` 模式系统提示词含机制说明 + 文件内容；`tools`/`off` 模式无 Memory 节 |

## 10. 文档与兼容性

- 更新 `CLAUDE.md`：工具表（记忆工具"默认启用"改为"取决于 `memory_mode`"）、`config.toml` 示例加 `memory_mode`。
- 更新 `docs/architecture.md`（如涉及工具注册/提示词章节）。
- **行为变化（需在 CHANGELOG 显著说明）**：未配置 `memory_mode` 的老用户升级后，SQLite 记忆工具默认消失，自动切换为文件记忆机制。需要原工具行为的用户应显式配置 `memory_mode = "tools"`。已存的 `memories` 表数据不迁移、不删除。

## 11. 非目标（YAGNI）

- 不做文件记忆的按用户/会话隔离（Bot 平台共享单份）。
- 不做 memory.md 注入上限的配置化（固定 16 KB）。
- 不做每日记忆文件的自动轮转/清理/归档。
- 不迁移 SQLite 记忆数据到文件。
- 不做 memory.md 会话中途热刷新。
