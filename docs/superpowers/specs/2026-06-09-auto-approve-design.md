# Auto-Approve 工具自动批准功能设计

**日期**: 2026-06-09  
**状态**: 设计中

## 概述

添加一个全局配置选项，允许用户配置是否自动批准所有工具调用，跳过用户确认步骤。同时支持命令行参数 `--auto-approve` 来覆盖配置文件。

## 背景与动机

当前 robit 中，某些工具（如 bash、write、edit）需要用户手动确认才能执行。在某些场景下（如自动化脚本、可信环境），用户希望跳过确认步骤，直接执行所有工具调用。

## 需求

1. **全局开关**：在配置文件中提供 `auto_approve` 布尔选项
2. **命令行参数**：支持 `--auto-approve` 标志来覆盖配置
3. **默认关闭**：保持当前行为，默认需要确认
4. **配置优先级**：命令行参数 > 配置文件 > 默认值

## 设计方案

### 1. 配置文件变更

在 `robit.toml` 的 `[app]` 节中添加：

```toml
[app]
# ... 现有配置 ...
auto_approve = false   # 是否自动批准所有工具调用（默认 false）
```

### 2. 配置结构变更

**文件**: `crates/robit-ai/src/config.rs`

在 `AppConfig` 结构体中添加字段：

```rust
#[derive(Debug, Deserialize, Default)]
pub struct AppConfig {
    pub log_level: Option<String>,
    pub max_steps: Option<usize>,
    pub enabled_tools: Option<Vec<String>>,
    pub enabled_skills: Option<Vec<String>>,
    pub context: Option<ContextConfig>,
    pub retry: Option<RetryConfig>,
    pub auto_approve: Option<bool>,  // 新增
}
```

### 3. Agent 结构体变更

**文件**: `crates/robit-agent/src/agent.rs`

在 `Agent` 结构体中添加配置字段：

```rust
pub struct Agent {
    llm_client: Arc<LlmClient>,
    tools: Arc<ToolRegistry>,
    skills: Arc<SkillRegistry>,
    sessions: HashMap<SessionId, AgentSession>,
    default_session_id: SessionId,
    context_manager: ContextManager,
    frontend: Arc<dyn Frontend>,
    auto_approve: bool,  // 新增
}
```

修改 `Agent::new()` 签名，接收此配置：

```rust
pub fn new(
    llm_client: Arc<LlmClient>,
    tools: Arc<ToolRegistry>,
    skills: Arc<SkillRegistry>,
    frontend: Arc<dyn Frontend>,
    context_config: Option<&ContextConfig>,
    context_window: Option<u64>,
    working_dir: PathBuf,
    auto_approve: bool,  // 新增
) -> Self {
    // ... 现有代码 ...

    Self {
        llm_client,
        tools,
        skills,
        sessions,
        default_session_id: session_id,
        context_manager,
        frontend,
        auto_approve,  // 新增
    }
}
```

### 4. 确认逻辑变更

**文件**: `crates/robit-agent/src/agent.rs`

修改 `run_one_step()` 中的确认检查（约第 360 行）：

```rust
// Check confirmation
let approved = if self.tools.requires_confirmation(&tc.function.name) && !self.auto_approve {
    self.frontend.request_tool_confirmation(&tc_info).await?
} else {
    true
};
```

逻辑说明：
- 如果工具需要确认 **且** `auto_approve` 未启用 → 请求用户确认
- 否则 → 直接批准

### 5. 命令行参数支持

需要修改两个二进制入口文件，并添加 `clap` 依赖。

#### 5.1 依赖添加

首先在 workspace 的 `Cargo.toml` 的 `[workspace.dependencies]` 中添加：

```toml
clap = { version = "4.5", features = ["derive"] }
```

然后在以下文件引用 workspace 版本：
- `crates/robit-tui/Cargo.toml`
- `examples/robit-agent/Cargo.toml`

```toml
clap.workspace = true
```

#### 5.2 TUI 入口

**文件**: `crates/robit-tui/src/main.rs`

添加 `--auto-approve` 参数：

```rust
#[derive(Debug, clap::Parser)]
struct Cli {
    /// 自动批准所有工具调用，跳过用户确认
    #[arg(long)]
    auto_approve: bool,
}
```

配置解析逻辑：
1. 解析命令行参数
2. 如果命令行提供 `--auto-approve` → 使用 `true`
3. 否则，使用配置文件中的 `app.auto_approve`
4. 如果配置文件未设置 → 使用默认值 `false`

#### 5.3 示例 Agent 入口

**文件**: `examples/robit-agent/src/main.rs`

同样添加 `--auto-approve` 参数，保持与 TUI 一致的行为。

### 6. 配置优先级

最终是否启用 `auto_approve` 按以下优先级决定（从高到低）：

1. **命令行参数 `--auto-approve`**
2. **配置文件中的 `app.auto_approve`**
3. **默认值 `false`**

## 修改文件清单

| 文件 | 修改内容 |
|------|----------|
| `crates/robit-ai/src/config.rs` | 在 `AppConfig` 添加 `auto_approve` 字段 |
| `crates/robit-agent/src/agent.rs` | 在 `Agent` 添加字段，修改 `new()` 和确认逻辑 |
| `crates/robit-tui/src/main.rs` | 添加 `--auto-approve` 参数，解析配置 |
| `examples/robit-agent/src/main.rs` | 添加 `--auto-approve` 参数，解析配置 |

## 非目标

- 不支持按工具白名单配置（后续可扩展）
- 不支持反向的 `--no-auto-approve` 参数（当前不需要）
- 不修改现有工具的 `requires_confirmation()` 默认值

## 测试计划

1. 配置文件测试：验证 `auto_approve = true` 能被正确解析
2. 命令行参数测试：验证 `--auto-approve` 能正确覆盖配置
3. 集成测试：验证启用 `auto_approve` 后工具执行不再请求确认
