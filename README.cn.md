# robit

robit（robo it）是一个基于 LLM 的 AI 自动化代理框架。它提供可扩展的 Agent 运行时、终端交互界面、桌面 GUI、统一的多提供商 LLM API，以及可配置的工具和技能系统。

当前仓库是 Rust monorepo，主要面向个人自动化、编程辅助和多前端 Agent 实验。

## 功能特性

- **统一 LLM 配置**：通过 OpenAI 兼容接口接入 DeepSeek、QWen 等模型提供商。
- **Agent 运行时**：事件驱动循环，支持流式输出、工具调用、用户确认和上下文管理。
- **终端前端**：`robit` TUI 基于 `ratatui` + `crossterm`，支持 Windows、Linux 和 macOS。
- **桌面 GUI**：`robit-gui` 基于 Tauri v2 + React，提供桌面应用体验。
- **多会话 Bot 框架**：`robit-chatbot` 提供平台无关的 Bot 基础设施，支持每个对话独立的 Agent 会话。
- **QQ Bot 前端**：`robit-qq` 实现 QQ 官方 Bot 接入（WebSocket 网关 + HTTP 发消息），支持群聊和私聊。
- **工具系统**：内置 `read`、`bash`、`write`、`edit`、`grep`、`find`、`ls` 等工具，可通过配置启用/禁用。
- **技能系统**：支持将预定义提示词模板作为 Markdown/YAML 技能加载，并通过斜杠命令触发。
- **项目/全局配置**：支持项目本地 `.robit/config.toml` 覆盖全局 `~/.robit/config.toml`。

## 仓库结构

```text
crates/
  robit-ai       # 多提供商 LLM API 与配置加载
  robit-agent    # Agent 运行时、工具系统、技能系统、Frontend trait
  robit-tui      # 终端前端；crate/package 名称和二进制命令均为 robit
  robit-gui      # 桌面 GUI 前端（Tauri v2 + React）
  robit-chatbot  # 多会话 Bot 基础设施（PlatformAdapter trait、ChatbotManager）
  robit-qq       # QQ 官方 Bot 前端（WebSocket + HTTP API），二进制命令 robit-qq
examples/
  robit-chat     # LLM API 层验证用 REPL
  robit-agent    # Agent 运行时验证用 stdin/stdout 前端
docs/            # 架构、协议、路线图和实现计划
```

## 安装与运行

### 前置要求

- Rust stable toolchain
- 对于 `robit-gui`：Node.js、npm，以及 Tauri 对应平台依赖
- 一个 OpenAI 兼容的模型服务 API Key，例如 DeepSeek 或 QWen

### 克隆并构建

```bash
git clone https://github.com/leexgone/robit.git
cd robit
cargo check --workspace
```

### 运行终端版

```bash
cargo run -p robit
```

指定工作目录：

```bash
cargo run -p robit -- --workdir /path/to/project
```

自动批准工具调用：

```bash
cargo run -p robit -- --auto-approve
```

安装到本机后运行：

```bash
cargo install --path crates/robit-tui
robit
```

> 后续发布到 crates.io 后，目标安装方式是 `cargo install robit`。

### 运行桌面 GUI

```bash
cargo run -p robit-gui
```

GUI 会话历史默认存储在 `<workdir>/.robit/memory/robit.db`。如果希望所有项目共享同一个会话数据库，可以启用全局存储：

```bash
cargo run -p robit-gui -- --global-storage
```

启用全局存储后，GUI 使用 `~/.robit/memory/robit.db`。

GUI 前端会根据 [crates/robit-gui/tauri.conf.json](crates/robit-gui/tauri.conf.json) 构建并加载 React 前端。

### 运行 QQ Bot

```bash
cargo run -p robit-qq
```

指定工作目录：

```bash
cargo run -p robit-qq -- --workdir /path/to/project
```

启用全局存储：

```bash
cargo run -p robit-qq -- --global-storage
```

`robit-qq` 会为每个 QQ 群聊或私聊创建独立的 Agent 会话，并通过内联消息进行工具确认（回复"确认"/"同意"/"y"/"yes"或"取消"/"拒绝"/"n"/"no"）。

## 配置

robit 使用统一配置文件 `config.toml`。加载顺序为：

1. `workdir/.robit/config.toml` 或当前目录 `.robit/config.toml`
2. `~/.robit/config.toml`

API Key 支持 `${ENV_VAR}` 环境变量替换。程序会自动尝试加载 `~/.robit/.env`。

最小配置示例：

```toml
default_model = "deepseek/deepseek-chat"

[providers.deepseek]
name = "DeepSeek"
base_url = "https://api.deepseek.com/v1"
api_key = "${DEEPSEEK_API_KEY}"

[[providers.deepseek.models]]
id = "deepseek-chat"
name = "DeepSeek Chat"
context_window = 65536
max_tokens = 4096
temperature = 0.0
supports_tools = true

[app]
log_level = "INFO"
max_steps = 10
enabled_tools = ["read", "bash", "edit", "write", "grep", "find", "ls"]
auto_approve = false
global_storage = false  # true 时 GUI 会话存储到 ~/.robit/memory/robit.db

[app.context]
max_output_lines = 500
max_output_bytes = 51200
reserve_ratio = 0.2

# QQ Bot 配置（使用 robit-qq 时需要）
[channels.qq_bot]
app_id = "your-app-id"
app_secret = "${QQ_BOT_SECRET}"
bot_token = "${QQ_BOT_TOKEN}"

# Bot 平台共享配置（可选）
[app.bot]
auto_approve = false
confirm_timeout_secs = 60
session_timeout_minutes = 30

[app.bot.confirm_keywords]
approve = ["确认", "同意", "yes", "y", "approve", "ok", "允许"]
reject = ["取消", "拒绝", "no", "n", "reject", "cancel", "deny"]
```

可在 `~/.robit/.env` 中保存密钥：

```bash
DEEPSEEK_API_KEY=your-api-key
```

## TUI 快捷操作

启动 `robit` 后，可直接输入自然语言任务，也可以使用以下命令：

| 命令 | 说明 |
| --- | --- |
| `/exit`、`/quit` | 退出程序 |
| `/clear` | 清空当前对话历史 |
| `/model` | 显示当前模型 |
| `/tools` | 显示启用的工具数量 |
| `/skills` | 显示可用技能 |
| `/scroll` | 切换滚动浏览模式 |

键盘操作：

- `Enter`：发送消息
- `Tab`：切换单行/多行输入
- `Ctrl+J`：多行模式下发送消息
- `Ctrl+C`：Agent 忙碌时取消当前任务
- `Ctrl+D`：退出程序
- `F8`：切换滚动模式
- `Y` / `N`：确认或拒绝需要确认的工具调用

## 工具系统

内置工具包括：

| 工具 | 说明 |
| --- | --- |
| `read` | 读取文件内容，支持输出截断 |
| `bash` | 执行 Shell 命令 |
| `write` | 创建或覆盖文件 |
| `edit` | 精确查找替换 |
| `grep` | 搜索文件内容 |
| `find` | 按模式查找文件 |
| `ls` | 列出目录内容 |
| `load_skill` | 加载技能内容，始终启用 |

`read` 和 `load_skill` 会始终注册。其他工具可通过 `[app].enabled_tools` 控制。

## 技能系统

技能是 Markdown/YAML 文件形式的提示词模板，可放在：

```text
~/.robit/skills/
.robit/skills/
```

TUI 中可以通过 `/skills` 查看已加载技能。技能文件格式和注册机制详见 [docs/architecture.md](docs/architecture.md)。

## 发布版本

预编译二进制包可在 [GitHub Releases](https://github.com/leexgone/robit/releases) 下载。每个发布版本包含适用于 Linux、macOS 和 Windows 的 CLI 和 GUI 包。

### CLI（终端版）

| 平台 | 资产 | 说明 |
| --- | --- | --- |
| Linux x86_64 | `robit-linux-x86_64.tar.gz` | 解压后运行 `./robit` |
| macOS (Apple Silicon) | `robit-macos-aarch64.tar.gz` | 解压后运行 `./robit` |
| Windows x86_64 | `robit-windows-x86_64.zip` | 解压后运行 `robit.exe` |

### GUI（桌面版）

| 平台 | 资产 | 说明 |
| --- | --- | --- |
| Linux | `Robit_*_amd64.AppImage` | 便携版，无需安装 |
| Linux (Debian/Ubuntu) | `Robit_*_amd64.deb` | `sudo dpkg -i Robit_*.deb` |
| Linux (Fedora/RHEL) | `Robit-*.x86_64.rpm` | `sudo rpm -i Robit-*.rpm` |
| macOS (Apple Silicon) | `Robit_*_aarch64.dmg` | 打开后拖入 Applications |
| Windows | `Robit_*_x64-setup.exe` | 运行安装程序 |
| Windows (MSI) | `Robit_*_x64_en-US.msi` | 企业部署 |

### 从源码安装

- `robit-ai`、`robit-agent`：适合发布到 crates.io，供 Rust 项目依赖。
- `robit`：终端程序，适合发布到 crates.io，用户可通过 `cargo install robit` 安装。
- `robit-gui`：桌面 GUI 更适合通过 GitHub Releases、官网、Homebrew Cask、Winget、Scoop、AppImage、deb/rpm、dmg/msi 等渠道分发。

## 文档

- [docs/architecture.md](docs/architecture.md)：Agent 运行时、Frontend trait、工具系统、技能系统和上下文管理
- [docs/protocol.md](docs/protocol.md)：消息结构与 Agent 事件定义
- [docs/roadmap.md](docs/roadmap.md)：阶段路线图
- [docs/plans/phase2-implementation.md](docs/plans/phase2-implementation.md)：Agent 运行时实现计划
- [docs/superpowers/specs/2026-06-18-robit-chatbot-qq-design.md](docs/superpowers/specs/2026-06-18-robit-chatbot-qq-design.md)：robit-chatbot & robit-qq 设计规格
- [docs/superpowers/2026-06-11-robit-gui-progress.md](docs/superpowers/2026-06-11-robit-gui-progress.md)：GUI 开发进度

## 当前状态

robit 目前处于早期开发阶段。核心 LLM API、Agent 运行时和 TUI 前端已具备基本可用能力；GUI 前端正在迭代中。接口、配置格式和发布方式在正式版本前仍可能调整。

## License

本项目采用 [Apache License 2.0](LICENSE) 授权。

Apache-2.0 允许使用、复制、修改和分发本项目代码，也允许用于商业用途；分发时需保留版权声明和许可证文本，并遵守许可证中的专利授权、NOTICE 等相关条款。
