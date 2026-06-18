# CLAUDE.md

此文档为编程工具在此代码仓库中工作使用。

## 项目概述

**robit** 是一个基于 Rust 的单体仓库，构建 AI 编程代理框架。它提供可扩展的自动化代理，包含终端 UI (TUI)、桌面 GUI (React + Tauri)、统一多提供商 LLM API 和代理运行时，支持 Windows、Linux 和 macOS 系统，兼容常用终端。

## 仓库结构

### 核心包（`crates/`）

| 包 | 说明 |
| --- | --- |
| `crates/robit-ai` | 多提供商 LLM API，支持 OpenAI 协议，适配 DeepSeek、QWen 等模型 |
| `crates/robit-agent` | 代理运行时（Agent 循环、工具执行、会话管理）。定义 `Frontend` trait 供前端实现。依赖 `robit-ai` |
| `crates/robit-tui` | 终端前端（`ratatui` + `crossterm`），crate 和二进制名均为 `robit`。实现 `Frontend` trait，依赖 `robit-agent` |
| `crates/robit-gui` | 桌面 GUI 前端（React + Tauri v2），实现 `Frontend` trait。SQLite 持久化会话。依赖 `robit-agent` |
| `crates/robit-chatbot` | 多会话 Bot 基础设施，提供 `PlatformAdapter` trait 和 `ChatbotManager`。QQ 和飞书等平台接入的共享基座。依赖 `robit-agent`、`robit-ai` |
| `crates/robit-feishu` | _（计划）_ 飞书前端，实现 `PlatformAdapter` trait |
| `crates/robit-qq` | QQ 前端，实现 `PlatformAdapter` trait（QQ 官方 Bot WebSocket 网关 + HTTP 发消息），二进制名 `robit-qq`。依赖 `robit-chatbot` |

### 验证工程（`examples/`）

| 工程 | 说明 |
| --- | --- |
| `examples/robit-chat` | REPL 交互式对话，用于阶段 1 验证。依赖 `robit-ai` |
| `examples/robit-agent` | 命令行 Agent 对话（stdin/stdout 前端），用于阶段 2 验证。依赖 `robit-agent` |

## 技术选型

| 领域 | 选择 | 说明 |
| ------ | ------ | ------ |
| 异步运行时 | `tokio` | 生态成熟，流式 HTTP 支持好 |
| HTTP 客户端 | `reqwest` | 支持 SSE 流式响应 |
| 序列化 | `serde` + `serde_json` | JSON 处理 |
| 配置解析 | `toml` | TOML 配置文件解析 |
| YAML 解析 | `serde_yaml` | 技能文件 frontmatter 解析 |
| Markdown 解析 | `pulldown-cmark` | TUI Markdown 渲染（MVP 极简版） |
| TUI 框架 | `ratatui` + `crossterm` | 跨平台，社区活跃 |
| GUI 框架 | Tauri v2 + React | 跨平台桌面应用，轻量级 |
| GUI UI 组件 | shadcn/ui + Tailwind CSS | 现代化、可定制 |
| GUI 状态管理 | Zustand | React 状态管理 |
| GUI Markdown | react-markdown | 前端 Markdown 渲染 |
| GUI 代码高亮 | react-syntax-highlighter | 代码块高亮 |
| 数据库 | SQLite + rusqlite | robit-gui 会话持久化 |
| 错误处理 | `thiserror`（库）+ `anyhow`（应用） | — |
| 日志 | `tracing` + `tracing-subscriber` | 结构化日志，异步友好 |
| CLI 参数 | `clap` (derive) | — |
| 异步 trait | `async-trait` | Tool trait 和 Frontend trait |
| 环境变量 | `dotenvy` | 加载 `~/.robit/.env` |
| 主目录 | `dirs` | 跨平台获取 `~` 路径 |
| 正则搜索 | `regex` | `grep` 工具实现 |
| 文件查找 | `globset` | `find` 工具实现 |
| 字符编码 | `encoding_rs` | 处理非 UTF-8 文件 |
| WebSocket | `tokio-tungstenite` | QQ Bot 网关连接（robit-qq） |
| HTTP 客户端（Bot） | `reqwest` | QQ Bot HTTP 发消息 + access token 获取 |

**后续版本待引入**：

- `syntect` — 代码高亮（MVP 先不做）
- `tiktoken-rs` — Token 精确计数（MVP 用字符估算 + API 返回的 `usage.total_tokens`）

## 工具系统

| 工具 | 说明 | 默认启用 | 需用户确认 |
| ------ | ------ | ---------- | ------------ |
| `read` | 读取文件内容，支持图片 | 是 | 否 |
| `bash` | 执行 Shell 命令，流式输出 | 是 | 是 |
| `edit` | 精确查找替换，支持多处并行编辑 | 是 | 是 |
| `write` | 创建/覆盖文件，自动创建父目录 | 是 | 是 |
| `grep` | 搜索文件内容 | 否 | 否 |
| `find` | 按模式查找文件 | 否 | 否 |
| `ls` | 列出目录内容 | 否 | 否 |

## 技能系统

技能是**预定义的提示词模板**，以 Markdown/YAML 文件形式存储，注入到系统提示词中指导 Agent 行为。技能与工具的区别：

| 维度 | 工具（Tool） | 技能（Skill） |
| ------ | ------------- | -------------- |
| 本质 | 代码能力 | 行为模板 |
| 实现 | Rust 代码 | Markdown/YAML 文件 |
| 触发 | LLM 主动调用 | 用户命令 / 系统提示词注入 |

技能文件格式及注册机制详见 `docs/architecture.md`。

## 配置

### 配置文件

统一配置文件 `config.toml`，加载顺序：`cwd/.robit/config.toml`（项目本地） → `~/.robit/config.toml`（全局）。

`api_key` 支持 `${ENV_VAR}` 环境变量替换，需配合 `.env` 文件或系统环境变量使用。

```txt
配置目录结构
    |--项目本地：.robit/config.toml   # 项目配置（最高优先级）
    |--项目本地：.robit/memory/robit.db # 默认 GUI 会话数据库
    |--全局：~/.robit/
    |   |-- .env                      # 环境变量（API keys 等）
    |   |-- config.toml               # 全局配置（fallback）
    |   |-- memory/robit.db           # 启用 global_storage 后的 GUI 会话数据库
    |   |-- skills/                   # 全局技能目录
    |   |-- prompts/                  # 自定义提示词目录
    |       |-- system.txt            # 自定义系统提示词（可选）
```

### config.toml 结构

```toml
# 默认模型（provider/model 格式）
default_model = "deepseek/deepseek-chat"

# 提供商定义
[providers.deepseek]
name = "DeepSeek"
base_url = "https://api.deepseek.com/v1"
api_key = "${DEEPSEEK_API_KEY}"        # 支持 ${ENV_VAR} 替换

[[providers.deepseek.models]]          # 每个提供商可有多个模型
id = "deepseek-chat"
name = "DeepSeek Chat"
context_window = 65536
max_output_tokens = 8192
temperature = 0.0
max_tokens = 4096
supports_images = false
supports_tools = true

[[providers.deepseek.models]]
id = "deepseek-reasoner"
name = "DeepSeek Reasoner"
context_window = 65536
temperature = 0.6
max_tokens = 8192

[providers.qwen]                       # 另一个提供商
name = "通义千问"
base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
api_key = "${DASHSCOPE_API_KEY}"

[[providers.qwen.models]]
id = "qwen-max"
name = "Qwen Max"
context_window = 32768
temperature = 0.7
supports_images = true
supports_tools = true

# 应用配置
[app]
log_level = "DEBUG"
max_steps = 10
enabled_tools = ["read", "bash", "edit", "write", "grep", "find", "ls"]  # 可选，启用的工具列表。不配置时启用所有工具；read 和 load_skill 始终启用
auto_approve = false                   # 可选，是否自动批准所有工具调用（默认 false）
global_storage = false                 # 可选，是否使用全局会话存储 ~/.robit/memory/robit.db（默认 false，使用 cwd/.robit/memory/robit.db）

[app.context]                          # 上下文管理配置（可选，以下为默认值）
max_output_lines = 500                 # 单次工具输出最大行数
max_output_bytes = 51200              # 单次工具输出最大字节数 (50KB)
reserve_ratio = 0.2                    # 为 LLM 响应预留的上下文比例 (20%)

[app.retry]                            # 重试策略配置（可选，以下为默认值）
max_retries = 3
initial_backoff_ms = 1000
max_backoff_ms = 30000

# 通信渠道配置（可选，QQ/飞书等 Bot 平台接入）
[channels.qq_bot]                      # QQ 官方 Bot 渠道
app_id = "123456789"
app_secret = "${QQ_BOT_SECRET}"        # 支持 ${ENV_VAR} 替换
bot_token = "${QQ_BOT_TOKEN}"

[app.bot]                              # Bot 平台共享设置（可选，以下为默认值）
auto_approve = false                   # 自动批准工具调用（覆盖 app.auto_approve）
confirm_timeout_secs = 60              # 工具确认超时（秒）
session_timeout_minutes = 30           # 空闲会话过期（分钟）

[app.bot.confirm_keywords]             # 确认/取消关键词（可选）
approve = ["确认", "同意", "yes", "y", "approve", "ok", "允许"]
reject = ["取消", "拒绝", "no", "n", "reject", "cancel", "deny"]
```

## 文档索引

| 文档 | 内容 |
| ------ | ------ |
| [`docs/architecture.md`](docs/architecture.md) | Agent 运行时、Frontend trait、会话管理、工具系统、技能系统、提示词系统、TUI 交互设计、上下文管理、错误处理策略 |
| [`docs/protocol.md`](docs/protocol.md) | 消息数据结构、Agent 事件定义 |
| [`docs/llm-config.md`](docs/llm-config.md) | ~~LLM 提供商配置结构~~（已过时，已统一为 `robit.toml`） |
| [`docs/roadmap.md`](docs/roadmap.md) | 构建路线图（4 个阶段） |
| [`docs/specs/2026-05-28-robit-ai-design.md`](docs/specs/2026-05-28-robit-ai-design.md) | 阶段 1 设计规格（`robit-ai` LLM API 层） |
| [`docs/plans/phase2-implementation.md`](docs/plans/phase2-implementation.md) | 阶段 2 实现计划（`robit-agent` Agent 运行时） |
| [`docs/plans/config-unification.md`](docs/plans/config-unification.md) | 配置统一计划（`robit.toml` 取代 `llms.toml` + `settings.toml`） |
| [`docs/superpowers/specs/2026-06-10-robit-gui-design.md`](docs/superpowers/specs/2026-06-10-robit-gui-design.md) | robit-gui 设计规格（桌面 GUI 前端） |
| [`docs/superpowers/specs/2026-06-18-robit-chatbot-qq-design.md`](docs/superpowers/specs/2026-06-18-robit-chatbot-qq-design.md) | robit-chatbot & robit-qq 设计规格（多会话 Bot 基座 + QQ Bot 接入） |
| [`docs/superpowers/plans/2026-06-10-robit-gui-implementation.md`](docs/superpowers/plans/2026-06-10-robit-gui-implementation.md) | robit-gui 实现计划 |
| [`docs/superpowers/2026-06-11-robit-gui-progress.md`](docs/superpowers/2026-06-11-robit-gui-progress.md) | robit-gui 开发进度记录 |
