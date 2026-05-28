# CLAUDE.md

此文档为编程工具在此代码仓库中工作使用。

## 项目概述

**robit** 是一个基于 Rust 的单体仓库，构建 AI 编程代理框架。它提供可扩展的编程代理，包含终端 UI、统一多提供商 LLM API 和代理运行时，支持 Windows、Linux 和 macOS 系统，兼容常用终端。

## 仓库结构

| 包 | 说明 |
| --- | --- |
| `crates/robit-ai` | 多提供商 LLM API，支持 OpenAI 协议，适配 DeepSeek、QWen 等模型 |
| `crates/robit-agent` | 代理运行时（Agent 循环、工具执行、会话管理）。定义 `Frontend` trait 供前端实现。依赖 `robit-ai` |
| `crates/robit-tui` | 终端前端，实现 `Frontend` trait。依赖 `robit-agent` |
| `crates/robit-feishu` | _（计划）_ 飞书前端，实现 `Frontend` trait |
| `crates/robit-qq` | _（计划）_ QQ 前端，实现 `Frontend` trait |

## 技术选型

| 领域 | 选择 | 说明 |
| ------ | ------ | ------ |
| 异步运行时 | `tokio` | 生态成熟，流式 HTTP 支持好 |
| HTTP 客户端 | `reqwest` | 支持 SSE 流式响应 |
| 序列化 | `serde` + `serde_json` | — |
| TUI 框架 | `ratatui` + `crossterm` | 跨平台，社区活跃 |
| 错误处理 | `thiserror`（库）+ `anyhow`（应用） | — |
| 日志 | `tracing` | 结构化日志，异步友好 |
| CLI 参数 | `clap` | — |
| Token 计数 | `tiktoken-rs` | 上下文窗口管理 |

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
    |   |-- llms.json       # 模型注册配置文件
    |   |-- settings.json   # 程序功能配置文件
    |   |-- skills/         # 全局技能目录
```

## 文档索引

| 文档 | 内容 |
| ------ | ------ |
| [`docs/architecture.md`](docs/architecture.md) | Agent 运行时机制、Frontend trait、会话管理、技能系统 |
| [`docs/protocol.md`](docs/protocol.md) | 消息数据结构、Agent 事件定义 |
| [`docs/llm-config.md`](docs/llm-config.md) | LLM 提供商配置结构（`llms.json`） |
| [`docs/roadmap.md`](docs/roadmap.md) | 构建路线图（4 个阶段） |
