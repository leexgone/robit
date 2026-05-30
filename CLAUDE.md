# CLAUDE.md

此文档为编程工具在此代码仓库中工作使用。

## 项目概述

**robit** 是一个基于 Rust 的单体仓库，构建 AI 编程代理框架。它提供可扩展的编程代理，包含终端 UI、统一多提供商 LLM API 和代理运行时，支持 Windows、Linux 和 macOS 系统，兼容常用终端。

## 仓库结构

### 核心包（`crates/`）

| 包 | 说明 |
| --- | --- |
| `crates/robit-ai` | 多提供商 LLM API，支持 OpenAI 协议，适配 DeepSeek、QWen 等模型 |
| `crates/robit-agent` | 代理运行时（Agent 循环、工具执行、会话管理）。定义 `Frontend` trait 供前端实现。依赖 `robit-ai` |
| `crates/robit-tui` | _（计划）_ 终端前端，实现 `Frontend` trait。依赖 `robit-agent` |
| `crates/robit-feishu` | _（计划）_ 飞书前端，实现 `Frontend` trait |
| `crates/robit-qq` | _（计划）_ QQ 前端，实现 `Frontend` trait |

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
| 错误处理 | `thiserror`（库）+ `anyhow`（应用） | — |
| 日志 | `tracing` + `tracing-subscriber` | 结构化日志，异步友好 |
| CLI 参数 | `clap` (derive) | — |
| 异步 trait | `async-trait` | Tool trait 和 Frontend trait |
| 环境变量 | `dotenvy` | 加载 `~/.robit/.env` |
| 主目录 | `dirs` | 跨平台获取 `~` 路径 |
| 正则搜索 | `regex` | `grep` 工具实现 |
| 文件查找 | `globset` | `find` 工具实现 |
| 字符编码 | `encoding_rs` | 处理非 UTF-8 文件 |

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

## 配置扩展

```txt
配置加载目录
    |--项目本地：cwd/.robit/skills/  # 项目技能目录
    |--全局：~/.robit/
    |   |-- .env            # 变量配置文件
    |   |-- llms.toml       # 模型注册配置文件
    |   |-- settings.toml   # 程序功能配置文件（按需扩展）
    |   |-- skills/         # 全局技能目录
    |   |-- prompts/        # 自定义提示词目录
    |       |-- system.txt  # 自定义系统提示词（可选，覆盖内置默认版）
```

### settings.toml

存储运行时配置，后续按需扩展：

```toml
# 当前使用的模型，格式 provider/model
model = "deepseek/deepseek-chat"

# 启用的工具列表（可选，未指定则使用默认值）
# 默认启用: read, bash, edit, write
# 默认禁用: grep, find, ls（需要时手动添加）
enabled_tools = ["read", "bash", "edit", "write"]

[context]  # 上下文管理配置（可选，以下为默认值）
max_output_lines = 500      # 单次工具输出最大行数
max_output_bytes = 51200    # 单次工具输出最大字节数 (50KB)
reserve_ratio = 0.2         # 为 LLM 响应预留的上下文比例 (20%)

[retry]  # 重试策略配置（可选，以下为默认值）
max_retries = 3             # 最大重试次数
initial_backoff_ms = 1000   # 初始退避时间
max_backoff_ms = 30000      # 最大退避时间
```

## 文档索引

| 文档 | 内容 |
| ------ | ------ |
| [`docs/architecture.md`](docs/architecture.md) | Agent 运行时、Frontend trait、会话管理、工具系统、技能系统、提示词系统、TUI 交互设计、上下文管理、错误处理策略 |
| [`docs/protocol.md`](docs/protocol.md) | 消息数据结构、Agent 事件定义 |
| [`docs/llm-config.md`](docs/llm-config.md) | LLM 提供商配置结构（`llms.toml`） |
| [`docs/roadmap.md`](docs/roadmap.md) | 构建路线图（4 个阶段） |
| [`docs/specs/2026-05-28-robit-ai-design.md`](docs/specs/2026-05-28-robit-ai-design.md) | 阶段 1 设计规格（`robit-ai` LLM API 层） |
| [`docs/plans/phase2-implementation.md`](docs/plans/phase2-implementation.md) | 阶段 2 实现计划（`robit-agent` Agent 运行时） |
