# 架构设计

## Agent 运行时

Agent 采用**事件驱动的消息循环**：

```text
用户输入
  ↓
Agent Loop
  ├── 组装上下文（历史消息 + 工具定义 + 系统提示词）
  ├── 调用 LLM（流式）
  ├── 解析响应
  │     ├── 纯文本 → 发射 TextDelta 事件，回到等待输入
  │     └── 工具调用 → 发射 ToolCallRequested 事件
  │           ├── 等待前端确认（可通过 auto_approve 配置跳过）
  │           ├── 确认后执行工具
  │           ├── 发射 ToolCallResult 事件
  │           ├── 结果回填到消息历史
  │           └── 回到「调用 LLM」（继续循环）
  └── 上下文管理
        ├── Token 计数（防止溢出）
        └── 历史压缩策略（摘要 / 截断）
```

**关键设计决策：**

- **循环控制权**：LLM 决定下一步（继续调工具还是结束），Agent 只负责执行，不做决策
- **工具执行策略**：默认写操作需用户确认，读操作自动执行，可通过配置覆盖
- **上下文窗口**：三层策略——工具输出截断 → 历史消息轮次截断（提前触发 + 保留最近轮次）→ 摘要压缩（LLM 生成真实摘要）
- **并发**：Agent 循环在 tokio 异步任务中运行，前端通过 channel 订阅事件

## Frontend Trait

`robit-agent` 定义 `Frontend` trait 作为前端抽象接口。Agent 不知道前端是 TUI、飞书还是 QQ，只通过 trait 交互。

```rust
#[async_trait]
pub trait Frontend: Send + Sync {
    /// Agent 推送事件给前端（文本、工具调用、错误等）
    async fn on_event(&self, event: AgentEvent) -> Result<()>;

    /// 请求用户确认工具调用（阻塞等待），返回是否批准
    async fn request_tool_confirmation(&self, info: &ToolCallInfo) -> Result<bool>;
}
```

前端通过独立 channel 向 Agent 推送 `FrontendMessage`（`UserInput` / `Cancel` / `ConfirmationResponse`），由 `Agent::run(message_rx)` 消费。

**各前端实现：**

| 前端 | 输入方式 | 输出方式 | 确认方式 |
| --- | --- | --- | --- |
| `robit` | 键盘直接输入 | 终端实时渲染（流式） | Y/N 键盘快捷键 |
| `robit-gui` | Tauri IPC 命令 | Tauri 事件推送 → React 渲染（流式） | UI 按钮点击 |
| `robit-chatbot` | 平台消息推送 | 平台 API 发送消息（智能分段） | 内联关键字回复 |
| `robit-qq` | QQ WebSocket 推送 | QQ API 发送消息 | 内联关键字回复 |
| `robit-feishu`（计划） | 飞书 WebSocket 推送 | 飞书 API 发送消息 | 消息卡片按钮 |

**TUI 与消息平台的差异处理：**

- 流式输出：TUI/GUI 逐字显示；Bot 平台采用智能分段 + 消息编辑实现近似流式体验
- 会话模型：TUI 单进程单会话；GUI 多会话；Bot 平台天然多用户多会话
- 工具确认：TUI/GUI 同步弹窗；Bot 平台异步内联消息 + 超时机制

**Bot 平台分层架构（`robit-chatbot` + `robit-qq`）：**

```txt
robit-qq (平台实现)
  └── 实现 PlatformAdapter trait
        ├── QQ WebSocket 连接
        ├── QQ 消息协议解析
        └── QQ Token 鉴权

robit-chatbot (共享基座)
  └── ChatbotManager<T: PlatformAdapter>
        ├── 多会话管理（chat_id → Agent 映射）
        ├── 会话持久化（SQLite，复用 robit-agent::storage）
        ├── ChatbotFrontend（实现 Frontend trait）
        │     ├── 流式输出缓冲 + 智能分段
        │     └── 进度提示（自动批准模式）
        ├── Confirmer（工具确认协调器）
        │     ├── 内联确认消息
        │     └── 关键字匹配 + 超时
        └── Markdown 处理（pulldown-cmark 解析）
```

详见 [`docs/superpowers/specs/2026-06-18-robit-chatbot-qq-design.md`](docs/superpowers/specs/2026-06-18-robit-chatbot-qq-design.md)。

## 会话管理

会话由 `SessionId`（UUID v4）唯一标识。不同前端采用不同的会话模型：

- **TUI**：单进程单会话
- **GUI**：多会话，每个会话一个 `Agent` 实例（`Arc` 共享 `LlmClient`/`ToolRegistry`/`SkillRegistry`）
- **Bot 平台**（`robit-chatbot`）：每个聊天（群/私聊）一个 `Agent` 实例，由 `ChatbotManager` 按 `chat_id` 路由

```rust
pub struct AgentSession {
    pub session_id: SessionId,
    pub history: Vec<ChatCompletionRequestMessage>,
    pub working_dir: PathBuf,
}

pub struct Agent {
    llm_client: Arc<LlmClient>,
    tools: Arc<ToolRegistry>,
    skills: Arc<SkillRegistry>,
    sessions: HashMap<SessionId, AgentSession>,
    default_session_id: SessionId,
    // ...
}
```

> 注：`AgentSession` 与 `Agent::sessions` 为 crate 私有，`Agent::run()` 消费 `self` 并运行单一会话循环。
> 因此多会话前端（GUI、Bot）通过为每个会话 spawn 一个独立 `Agent` 实现隔离（见 `robit-gui`、`robit-chatbot`）。

### 会话持久化（SQLite）

所有前端共用 `robit-agent::storage` 的统一 schema（含版本化迁移）。`sessions` 表新增 `chat_id`（平台聊天标识，GUI/TUI 为 NULL）和 `source`（创建前端：`gui`/`tui`/`qq`/`feishu`）两列，详见 QQ 设计规格 §8。Bot 平台通过 `find_session_by_chat_id()` 恢复会话记录。

## 工具系统

### Tool Trait

每个工具需要向 LLM 描述自己的能力，同时能被 Agent 调用执行：

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    /// 工具名称，LLM 通过这个名字调用
    fn name(&self) -> &str;

    /// 工具描述，注入到系统提示词中
    fn description(&self) -> &str;

    /// 工具的参数 JSON Schema，LLM 根据这个生成参数
    fn parameters_schema(&self) -> serde_json::Value;

    /// 是否需要用户确认才执行
    fn requires_confirmation(&self) -> bool;

    /// 执行工具，返回结果给 LLM
    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult>;
}
```

### ToolContext

工具运行时所需的环境：

```rust
pub struct ToolContext {
    pub working_dir: PathBuf,       // 当前工作目录
    pub session_id: SessionId,      // 当前会话
    pub frontend: Arc<dyn Frontend>, // 用于需要和用户交互的工具
}
```

### ToolResult

返回给 LLM 的结果：

```rust
pub struct ToolResult {
    pub content: String,            // 文本结果，LLM 会读取
    pub is_error: bool,             // 是否是错误（LLM 可以看到错误并重试）
    pub metadata: Option<serde_json::Value>, // 附加信息（如图片 base64）
}
```

### ToolRegistry

工具注册表，Agent 通过它查找和管理工具：

```rust
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn register(&mut self, tool: impl Tool + 'static) { ... }

    /// 生成所有工具的 schema，注入到 LLM 请求中
    pub fn tool_schemas(&self) -> Vec<serde_json::Value> { ... }

    /// 根据名称查找并执行工具
    pub async fn execute(
        &self,
        name: &str,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult> { ... }
}
```

### 工具参数定义

#### `read` — 读取文件

```json5
// 参数
{
  "file_path": "src/main.rs",  // 必填，相对或绝对路径
  "offset": 0,                 // 可选，起始行号
  "limit": 200                 // 可选，读取行数上限
}
// 结果：文件内容（带行号），图片返回 base64
```

#### `bash` — 执行命令

```json5
// 参数
{
  "command": "cargo build",    // 必填
  "timeout": 120000,           // 可选，超时（ms），默认 120s
  "working_dir": null          // 可选，覆盖工作目录（默认为项目根目录）
}
// 结果：stdout + stderr，退出码
```

#### `edit` — 精确编辑

```json5
// 参数
{
  "file_path": "src/lib.rs",   // 必填
  "edits": [                   // 必填，支持多处并行编辑
    {
      "old_string": "fn old()",
      "new_string": "fn new()"
    }
  ]
}
// 结果：成功/失败，修改的行数
```

**匹配策略**：`old_string` 必须在文件中**唯一匹配**，否则报错。MVP 阶段不支持 `replace_all`。

#### `write` — 写入文件

```json5
// 参数
{
  "file_path": "src/new.rs",   // 必填
  "content": "..."             // 必填，完整文件内容
}
// 结果：成功/失败，写入字节数
```

#### `grep` / `find` / `ls` — 搜索与浏览

```json5
// grep 参数
{ "pattern": "fn execute", "path": "src/", "output_mode": "content" }

// find 参数
{ "pattern": "*.rs", "path": "src/" }

// ls 参数
{ "path": "src/" }
```

### 安全确认机制

确认机制和 Frontend trait 联动：

```txt
Agent 收到 LLM 的 tool_call
  ↓
检查 tool.requires_confirmation()
  ├── false → 直接执行，返回结果
  └── true  → 检查 auto_approve 配置
              ├── auto_approve = true → 直接执行，跳过确认
              └── auto_approve = false → 调用 frontend.request_tool_confirmation(tool_call)
                              ├── 用户同意 → 执行
                              └── 用户拒绝 → 返回 ToolResult { is_error: true, content: "用户拒绝执行" }
                                              （LLM 可以看到拒绝原因，调整策略）
```

**auto_approve 配置优先级**：

1. 命令行参数 `--auto-approve`（最高优先级）
2. `config.toml` 中的 `[app] auto_approve` 配置
3. 默认 `false`（需要确认）

### 工具启用策略

通过 `config.toml` 中的 `enabled_tools` 属性配置：

```toml
[app]
enabled_tools = ["read", "bash", "edit", "write", "grep", "find", "ls"]
```

**规则：**
- **不配置 `enabled_tools`**：自动启用所有工具
- **配置 `enabled_tools`**：只启用列表中指定的工具
- **`read` 和 `load_skill`**：始终启用（基础功能必需）

## Bootstrap 模块

为避免各前端（`robit-tui`、`robit-gui`、`examples/robit-agent`）重复实现技能和工具加载逻辑，`robit-agent` 提供 `bootstrap` 模块统一处理启动流程。

### 核心功能

`bootstrap` 模块提供以下功能：

| 函数 | 说明 |
| ---- | ---- |
| `bootstrap(config, working_dir, base_tool_names)` | 一站式启动：加载技能 → 过滤启用技能 → 创建 `SkillRegistry` → 创建 `ToolRegistry` → 返回 `BootstrapResult` |
| `load_all_skills(working_dir)` | 从全局 `~/.robit/skills/` 和项目 `{working_dir}/.robit/skills/` 加载技能 |
| `filter_skills_by_config(skills, config)` | 根据 `config.app.enabled_skills` 过滤技能列表 |
| `create_tools_from_config(config, skill_registry)` | 从配置创建完整 `ToolRegistry`，包含所有标准工具 |
| `log_skill_errors(errors)` | 记录技能加载警告（非致命错误） |

### BootstrapResult

`bootstrap()` 返回包含所有必要组件的结构：

```rust
pub struct BootstrapResult {
    pub skill_registry: Arc<SkillRegistry>,
    pub tool_registry: Arc<ToolRegistry>,
    pub total_skills_loaded: usize,
    pub skill_load_errors: Vec<SkillLoadError>,
}
```

### 前端使用示例

各前端不再需要手动加载技能和创建工具：

```rust
// 旧方式（约 40 行重复代码）
let global_skills_dir = dirs::home_dir().map(|h| h.join(".robit/skills"));
let project_skills_dir = Some(working_dir.join(".robit/skills"));
let (skills, errors) = load_skills(global_skills_dir, project_skills_dir);
log_skill_errors(&errors);
let enabled_skills = config.app.as_ref().and_then(|a| a.enabled_skills.as_ref());
let filtered_skills = filter_skills_by_config(skills, config);
let skill_registry = Arc::new(SkillRegistry::new(filtered_skills, &base_tools));
let mut tools = ToolRegistry::new();
tools.register(ReadTool::new(max_lines, max_bytes));
tools.register(BashTool::new(max_bytes));
// ... 更多工具注册

// 新方式（约 5 行）
use robit_agent::{bootstrap, log_skill_errors};
let base_tool_names = ["read", "bash", "write", "edit"];
let bootstrap_result = bootstrap(&config, &working_dir, &base_tool_names);
log_skill_errors(&bootstrap_result.skill_load_errors);
let skill_registry = bootstrap_result.skill_registry;
let tool_registry = bootstrap_result.tool_registry;
```

### 包含的标准工具

`create_tools_from_config()` 自动注册以下工具：

- `read` - 读取文件（带输出截断）
- `bash` - 执行 Shell 命令
- `write` - 创建/覆盖文件
- `edit` - 精确编辑
- `load_skill` - 动态加载技能
- `ls` - 列出目录
- `find` - 查找文件
- `grep` - 搜索内容

所有工具参数从 `config.app.context` 配置读取。

## 技能系统

技能是**预定义的提示词模板**，以目录为单位组织，每个技能目录下包含一个 `SKILL.md` 文件（YAML frontmatter + Markdown body）。系统在启动时加载技能，按需注入到 Agent 的系统提示词中。

### 技能目录结构

每个技能是一个独立目录，内含 `SKILL.md` 作为主定义文件：

```text
~/.robit/skills/
  ├── code-review/
  │   └── SKILL.md
  ├── refactor/
  │   └── SKILL.md
  └── custom-skill/
      ├── SKILL.md
      └── reference.md   (可选，辅助文件)
```

`SKILL.md` 采用 YAML frontmatter + Markdown body 结构：

```markdown
---
name: code-review
description: 审查代码变更，关注正确性、性能和安全性
version: 1.0.0
triggers:
  - /review
  - /代码审查
tools_required:
  - bash
  - read
  - grep
enabled: true
---

# 代码审查技能

请按以下步骤审查代码变更：

## 步骤

1. 运行 `git diff` 查看当前变更
2. 逐文件分析变更内容
3. 对每个文件给出以下维度的评价：
   - **正确性**：逻辑是否正确，边界条件是否处理
   - **性能**：是否存在性能隐患
   - **安全性**：是否引入安全风险
   - **可维护性**：代码是否清晰易懂
4. 给出整体评价和改进建议

## 输出格式

使用结构化输出，每个文件单独评审，最后给出总结。
```

### Frontmatter 字段定义

| 字段 | 类型 | 必填 | 说明 |
| ---- | ------ | ---- | ---- |
| `name` | `string` | 是 | 技能唯一标识符，用于内部引用 |
| `description` | `string` | 是 | 技能描述，展示给用户和 Agent |
| `version` | `string` | 否 | 语义版本号 |
| `triggers` | `string[]` | 否 | 触发命令列表（如 `/review`），为空则仅通过系统提示词注入 |
| `tools_required` | `string[]` | 否 | 该技能依赖的工具列表，用于检查工具可用性 |
| `enabled` | `bool` | 否 | 是否启用，默认 `true` |

### 技能加载优先级

```txt
~/.robit/skills/          ← 全局技能（低优先级）
cwd/.robit/skills/        ← 项目技能（高优先级，同名覆盖全局）
```

每个子目录为一个技能，目录内必须有 `SKILL.md`。项目技能同名覆盖全局技能。

### 技能注入时机

- **Agent 启动时**：将所有 `enabled: true` 的技能描述注入系统提示词，让 LLM 知晓可用技能
- **用户触发时**：当用户输入匹配 `triggers` 中的命令时，将对应技能的完整 Markdown body 作为系统消息注入当前对话

## 提示词系统

### 提示词组成

系统提示词由多个模块动态组装：

```txt
┌─────────────────────────────────────┐
│  1. 身份定义（Agent Identity）       │  ← 固定
├─────────────────────────────────────┤
│  2. 工具使用说明（Tool Instructions）│  ← 根据 enabled_tools 动态生成
├─────────────────────────────────────┤
│  3. 编程规范（Coding Guidelines）    │  ← 固定
├─────────────────────────────────────┤
│  4. 环境信息（Environment Info）     │  ← 运行时注入（OS、cwd、时间等）
├─────────────────────────────────────┤
│  5. 技能注入（Skills）               │  ← 根据启用的技能动态注入
└─────────────────────────────────────┘
```

### 内置默认提示词（精简版）

```txt
你是 Robit，一个 AI 编程代理。

## 工作方式
- 直接执行任务，不要解释计划
- 不确定时先读取代码，再行动
- 修改文件优先用 edit，创建文件用 write
- 遵循项目现有代码风格

## 环境
- 操作系统：{os}
- 工作目录：{cwd}
- 当前日期：{date}
```

**动态替换**：`{os}`、`{cwd}`、`{date}` 在运行时注入实际值。

### 工具 Schema Description

每个工具的 `description` 字段传给 LLM API，保持简短：

| 工具 | Description |
| ---- | ----------- |
| `read` | 读取文件内容。支持文本和图片。大文件可用 offset/limit 分段读取。 |
| `bash` | 执行 Shell 命令。避免 cd，使用绝对路径。 |
| `edit` | 精确修改文件。old_string 必须唯一匹配。支持多处并行修改。 |
| `write` | 创建或完全覆盖文件。修改现有文件优先用 edit。 |
| `grep` | 搜索文件内容。返回匹配行或文件路径。 |
| `find` | 按文件名模式查找文件。 |
| `ls` | 列出目录内容。 |

### 自定义提示词

提示词分为两部分：
1. **Agent 提示词（用户可自定义）**：定义角色、工作方式等
2. **系统提示词（内置固定）**：包含 Tools、Skills、Environment 等，自动追加

用户可通过以下文件自定义 Agent 提示词：

```txt
# 优先级顺序（高→低）：
{cwd}/.robit/prompts/agent.md    ← 项目本地（最高优先级）
~/.robit/prompts/agent.md         ← 全局配置
```

- **存在**：读取用户自定义 agent.md，自动追加系统内置提示词
- **不存在**：使用默认 agent.md + 系统内置提示词

提示词结构：

```txt
[Agent 提示词]   ← 用户可自定义（agent.md）
    ↓
[系统提示词]     ← 内置固定，自动追加（system.md）
  ├── Available Tools
  ├── Available Skills
  └── Environment
```

### 技能注入格式

技能触发时，追加到系统提示词末尾：

```txt
## 技能：{skill.name}

{skill.description}

{skill.content}
```

## TUI 交互设计

### 整体布局

```txt
┌──────────────────────────────────────────────────┐
│  ● robit v0.1.0  │  deepseek/deepseek-chat       │  ← 状态栏（顶部）
├──────────────────────────────────────────────────┤
│                                                    │
│  用户：帮我看看 src/main.rs 有什么问题             │
│                                                    │
│  Robit：                                           │  ← 对话区域
│  让我先看一下这个文件。                             │     （可滚动）
│                                                    │
│  ┌─ 🔧 read ─────────────────────────────────┐    │
│  │ src/main.rs                                │    │  ← 工具调用卡片
│  └────────────────────────────────────────────┘    │
│                                                    │
│  这个文件有几个问题：                               │
│  1. 第 15 行的变量未使用                            │
│  2. 第 23 行缺少错误处理                            │
│                                                    │
│  ┌─ ✏️ edit ─────────────────────────────────┐    │
│  │ src/main.rs                                │    │
│  │ [Y] 允许 / [N] 拒绝                         │    │  ← 确认交互
│  └────────────────────────────────────────────┘    │
│                                                    │
├──────────────────────────────────────────────────┤
│  > 帮我修复这些问题                          [Tab] │  ← 输入区域
└──────────────────────────────────────────────────┘
```

### 状态栏（顶部）

一行，显示关键运行信息：

```txt
● robit v0.1.0  │  deepseek/deepseek-chat  │  工具: 4/7  │  tokens: 2048/65536
```

| 信息 | 说明 |
| ---- | ---- |
| 版本号 | `robit v0.1.0` |
| 当前模型 | `deepseek/deepseek-chat` |
| 工具启用数 | `4/7`（已启用/总数） |
| Token 使用量 | `2048/65536`（已用/上限） |

### 对话区域（主体）

- **流式显示**：LLM 响应逐字输出，光标跟随
- **可滚动**：历史对话可以上下滚动查看
- **Markdown 渲染**：MVP 极简版，仅处理代码块语法高亮和粗体/斜体，其余原样显示
- **工具调用卡片**：内嵌在对话流中，视觉区分

技术实现：`pulldown-cmark`（Markdown 解析）+ `syntect`（代码高亮）。

### 工具调用卡片

工具执行时，在对话流中插入卡片：

```txt
┌─ 🔧 bash ──────────────────────────────────────────┐
│ $ cargo build                                       │
│                                                     │
│ Compiling robit-ai v0.1.0                           │
│ Compiling robit-agent v0.1.0                        │
│ Finished `dev` profile [unoptimized] target(s)      │
│                                                     │
│ ✓ 完成 (3.2s)                                       │
└─────────────────────────────────────────────────────┘
```

卡片状态：

| 状态 | 显示 |
| ---- | ---- |
| 执行中 | `⏳ 执行中...` + 旋转动画 |
| 成功 | `✓ 完成 (耗时)` |
| 失败 | `✗ 失败 (错误信息)` |
| 等待确认 | `[Y] 允许 / [N] 拒绝` |

### 输入区域（底部）

```txt
> 输入内容                                    [Enter 发送 / Tab 多行]
```

| 特性 | 说明 |
| ---- | ---- |
| 单行模式 | 默认，Enter 直接发送 |
| 多行模式 | Tab 切换，支持换行输入 |
| 历史记录 | 上/下箭头浏览历史输入 |
| 取消操作 | `Ctrl+C` 取消当前 Agent 操作，不退出程序 |
| 退出 | `Ctrl+D` 或输入 `/exit` 退出 |

### 交互流程

```txt
用户输入 → Agent 接收
  ↓
Agent 调用 LLM（流式）
  ↓
逐字显示文本到对话区域
  ↓
LLM 返回工具调用
  ↓
显示工具调用卡片
  ├── 不需要确认 → 自动执行，卡片显示执行状态
  └── 需要确认 → 卡片显示 [Y/N]，等待用户按键
       ├── Y → 执行，更新卡片状态
       └── N → 卡片标记"已拒绝"，结果反馈给 LLM
  ↓
工具结果回填，LLM 继续循环
  ↓
LLM 返回最终文本 → 显示到对话区域
  ↓
TurnComplete → 输入区域恢复可用
```

### 斜杠命令

用户在输入区域输入 `/` 开头的命令，由前端直接处理，不经过 LLM：

| 命令 | 说明 |
| ---- | ---- |
| `/exit` | 退出程序 |
| `/clear` | 清空对话历史 |
| `/model <provider/model>` | 切换模型 |
| `/tools` | 显示已启用的工具列表 |
| `/skills` | 显示可用技能列表 |

技能定义的 `triggers` 也以斜杠命令触发，由 Agent 处理（注入技能内容后交给 LLM）。

### 对话历史持久化

TUI 不持久化，每次启动是新对话。GUI 与 Bot 平台通过 SQLite 持久化会话元数据与消息（`robit-agent::storage`，schema 版本化，当前 v2）：

- DB 路径：`cwd/.robit/memory/robit.db`（默认）或 `~/.robit/memory/robit.db`（`global_storage = true`）
- GUI 重开会话时会重新读取 DB 消息用于显示；Bot 平台重启后按 `chat_id` 恢复会话记录
- 注：当前 MVP 不会把 DB 历史注入回 Agent 的内存 `history`（`AgentSession` 私有），重启后 Agent 从空历史开始；完整历史恢复是后续增强

## 上下文管理

当对话历史不断增长，总 Token 接近模型的 `contextWindow` 时，必须采取措施防止溢出。采用三层策略，由简单到复杂逐步实现。

### 第一层：工具输出截断

在工具返回结果时控制大小，防止单次输出过大：

```rust
pub struct ContextManager {
    pub max_output_lines: usize,   // 单次工具输出最大行数，默认 500
    pub max_output_bytes: usize,   // 单次工具输出最大字节数，默认 50KB (51200)
}
```

截断时附加提示，引导 LLM 分段读取：

```txt
... (Output truncated, 1200 lines total, showing first 500. Use offset/limit to read more)
```

可通过 `config.toml` 配置：

```toml
[app.context]
max_output_lines = 500
max_output_bytes = 51200
```

### 第二层：历史消息截断

当总 Token 达到阈值时，从最早的非系统消息开始按**轮次**整体移除（user + assistant + tool_calls + tool_results 一起移除，保持对话完整性）。

**关键参数：**

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `truncation_ratio` | 0.7 | 截断触发阈值（占 max_tokens 的比例），提前触发以留出估算误差余量 |
| `min_keep_rounds` | 3 | 截断后最少保留的最近轮数，防止丢失关键上下文 |
| `token_safety_margin` | 1.3 | 估算安全系数，补偿启发式估算与真实 tokenizer 计数之间的误差 |
| `reserve_ratio` | 0.2 | 为 LLM 响应预留的上下文比例 |

**Token 估算**：按字符类型分类估算，而非简单的 `chars/N`：

| 字符类型 | 估算比率 | 说明 |
|----------|----------|------|
| ASCII 字母/数字 | ~3.5 chars/token | BPE tokenizer 平均 |
| CJK 字符 | ~1.5 chars/token | 大多数字符 1-2 token |
| 标点/符号/代码 | ~1 char/token | 通常每个字符独立成 token |
| 空白 | ~10 chars/token | 通常与相邻 token 合并 |

截断后插入通知，告知 LLM 上下文已被压缩：

```txt
[Context truncated: 5 earlier rounds (23 messages) removed to stay within token limit. 3 most recent rounds preserved.]
```

### 第三层：摘要压缩（已实现）

当被移除的消息 Token 数超过 `compression_token_threshold`（默认 5000）时，调用 LLM 生成 1-2 句摘要替代静态通知：

```txt
[Earlier conversation summary: User asked to refactor the auth module. Agent read auth.rs, identified 3 error handling issues, and applied fixes using edit tool. Tests passed after changes.]
```

摘要生成失败时降级为静态消息 `"Conversation history compressed."`，不阻断 Agent 循环。

可通过 `config.toml` 配置：

```toml
[app.context]
compression_enabled = true              # 是否启用摘要压缩（默认 true）
compression_token_threshold = 5000      # 触发压缩的 token 阈值（默认 5000）
```

### 单轮保护

通过 `max_tool_calls_per_turn`（默认 30）限制单轮内累计工具调用次数，防止单轮爆炸式增长。超出限制时强制终止当前轮次并提示 LLM 在下一消息中继续。

### 完整配置

```toml
[app.context]
max_output_lines = 500                  # 单次工具输出最大行数
max_output_bytes = 51200                # 单次工具输出最大字节数 (50KB)
reserve_ratio = 0.2                     # 为 LLM 响应预留的上下文比例 (20%)
truncation_ratio = 0.7                  # 截断触发阈值 (70% of max_tokens)
min_keep_rounds = 3                     # 截断后最少保留轮数
token_safety_margin = 1.3               # 估算安全系数
compression_enabled = true              # 是否启用摘要压缩
compression_token_threshold = 5000      # 触发压缩的 token 阈值
max_tool_calls_per_turn = 30            # 单轮最大工具调用数
```

### Token 使用量日志

每次 LLM 调用前后记录 `info` 级别日志，包括估计 prompt tokens、response tokens 和 API 返回的实际 usage（如果提供方在流式响应末尾包含 usage 信息）。

## 错误处理策略

错误分为四类：网络与 API 错误、工具执行错误、Agent 逻辑错误、配置错误。核心原则是**区分工具错误和系统错误**：工具错误返回给 LLM 让其自行调整，系统错误显示给用户需要人工介入。

### 错误分类

```txt
┌─────────────────────────────────────────────────┐
│  1. 网络与 API 错误（robit-ai 层）               │
│     - 连接超时 / 网络断开                        │
│     - 认证失败（API Key 无效/过期）              │
│     - 速率限制（429）                            │
│     - 模型不存在 / 参数错误（4xx）               │
│     - 服务端错误（5xx）                          │
├─────────────────────────────────────────────────┤
│  2. 工具执行错误（robit-agent 层）               │
│     - 文件不存在 / 权限不足                      │
│     - edit 匹配失败（old_string 未找到/不唯一）  │
│     - bash 命令执行失败（非零退出码）            │
│     - 命令超时                                   │
├─────────────────────────────────────────────────┤
│  3. Agent 逻辑错误（robit-agent 层）             │
│     - LLM 返回格式错误（JSON 解析失败）          │
│     - LLM 调用不存在的工具                       │
│     - 上下文溢出                                 │
├─────────────────────────────────────────────────┤
│  4. 配置错误（启动时）                           │
│     - llms.toml 格式错误 / 文件缺失              │
│     - API Key 未配置                             │
│     - 模型引用不存在（provider/model）            │
└─────────────────────────────────────────────────┘
```

### 网络与 API 错误

```rust
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("网络连接失败: {0}")]
    ConnectionError(String),

    #[error("认证失败，请检查 API Key 配置")]
    AuthenticationError,

    #[error("请求速率受限，请稍后重试")]
    RateLimitError { retry_after: Option<u64> },

    #[error("模型不可用: {model}")]
    ModelNotFound { model: String },

    #[error("服务端错误 ({status}): {message}")]
    ServerError { status: u16, message: String },

    #[error("响应格式错误: {0}")]
    ParseError(String),
}
```

**重试策略**：

| 错误类型 | 是否自动重试 | 说明 |
| -------- | ------------ | ---- |
| 连接超时 | 是（最多 3 次，指数退避） | 网络波动常见 |
| 认证失败 | 否 | 需要用户修复配置 |
| 速率限制 (429) | 是（尊重 `Retry-After`） | 等待后重试 |
| 4xx 客户端错误 | 否 | 请求本身有问题 |
| 5xx 服务端错误 | 是（最多 2 次） | 服务端临时故障 |
| 流式中断 | 是（1 次，从头重试） | 网络断开导致流中断 |

重试策略可通过 `settings.toml` 配置：

```toml
[retry]
max_retries = 3             # 最大重试次数，默认 3
initial_backoff_ms = 1000   # 初始退避时间，默认 1000ms
max_backoff_ms = 30000      # 最大退避时间，默认 30000ms
```

### 工具执行错误

工具错误**不抛异常**，而是包装成 `ToolResult { is_error: true }`，让 LLM 看到并调整策略：

```txt
// LLM 调用 edit，但 old_string 不匹配
→ ToolResult { content: "错误：old_string 在文件中未找到匹配", is_error: true }
→ LLM 看到错误，可能重新 read 文件后再次尝试 edit

// LLM 调用 bash 执行测试，测试失败
→ ToolResult { content: "退出码 1\n\ntest result: FAILED. 2 passed; 1 failed", is_error: true }
→ LLM 看到测试失败信息，分析原因并修复代码
```

**bash 非零退出码不算系统错误**——测试失败、编译错误这些都是正常的编程反馈，LLM 需要看到这些信息。

**超时策略**：仅 `bash` 工具有超时（默认 120s），其他工具为本地文件操作，不会长时间阻塞。

### Agent 逻辑错误

| 错误 | 处理方式 |
| ---- | -------- |
| LLM 返回格式错误 | 记录日志，通知前端，终止当前轮次 |
| LLM 调用不存在的工具 | 返回错误给 LLM：`"工具 xxx 不存在，可用工具: [...]"` |
| 上下文溢出 | 触发第二层截断策略后重试 |

### 配置错误

启动时检查，**快速失败**：

```txt
robit 启动
  ↓
检查 ~/.robit/llms.toml
  ├── 不存在 → 提示用户创建，给出示例
  ├── 格式错误 → 显示具体解析错误和行号
  └── API Key 未配置 → 提示设置环境变量
检查 settings.toml 中的 model 引用
  ├── provider 不存在 → 提示可用 provider 列表
  └── model 不存在 → 提示该 provider 下的可用模型
```

### 统一错误类型

在 `robit-agent` 中定义统一的错误类型：

```rust
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(transparent)]
    LlmError(#[from] LlmError),

    #[error("配置错误: {0}")]
    ConfigError(String),

    #[error("上下文溢出: 当前 {current} tokens，上限 {max} tokens")]
    ContextOverflow { current: usize, max: usize },

    #[error("Agent 内部错误: {0}")]
    InternalError(String),
}
```

### 错误展示

| 错误来源 | 展示位置 | 展示方式 |
| -------- | -------- | -------- |
| 工具执行失败 | 对话区域工具卡片 | `✗ 失败` + 红色错误信息 |
| LLM API 错误 | 对话区域 | 系统消息，红色显示 |
| 配置错误 | 启动日志 | 终端输出 + 修复建议 |
| 重试中 | 状态栏 | `⟳ 重试中 (2/3)...` |
