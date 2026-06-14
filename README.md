# robit

[中文文档](README.cn.md)

robit (robo it) is an LLM-powered AI automation agent framework. It provides an extensible agent runtime, terminal UI, desktop GUI, unified multi-provider LLM API, and configurable tool and skill systems.

This repository is a Rust monorepo focused on personal automation, programming assistance, and multi-frontend agent experiments.

## Features

- **Unified LLM configuration**: Connect to OpenAI-compatible providers such as DeepSeek and QWen.
- **Agent runtime**: Event-driven loop with streaming output, tool calls, user confirmation, and context management.
- **Terminal frontend**: The `robit` TUI is built with `ratatui` and `crossterm`, and supports Windows, Linux, and macOS.
- **Desktop GUI**: `robit-gui` is built with Tauri v2 and React for a native desktop experience.
- **Tool system**: Built-in tools include `read`, `bash`, `write`, `edit`, `grep`, `find`, and `ls`; tools can be enabled or disabled through configuration.
- **Skill system**: Load predefined prompt templates from Markdown/YAML files and trigger them with slash commands.
- **Project/global configuration**: Project-local `.robit/config.toml` can override the global `~/.robit/config.toml`.

## Repository Layout

```text
crates/
  robit-ai       # Multi-provider LLM API and configuration loading
  robit-agent    # Agent runtime, tool system, skill system, Frontend trait
  robit-tui      # Terminal frontend; crate/package name and binary command are robit
  robit-gui      # Desktop GUI frontend (Tauri v2 + React)
examples/
  robit-chat     # REPL for validating the LLM API layer
  robit-agent    # stdin/stdout frontend for validating the agent runtime
docs/            # Architecture, protocol, roadmap, and implementation plans
```

## Installation and Usage

### Prerequisites

- Rust stable toolchain
- For `robit-gui`: Node.js, npm, and the Tauri platform prerequisites
- An API key for an OpenAI-compatible model provider, such as DeepSeek or QWen

### Clone and Build

```bash
git clone https://github.com/leexgone/robit.git
cd robit
cargo check --workspace
```

### Run the Terminal App

```bash
cargo run -p robit
```

Specify a working directory:

```bash
cargo run -p robit -- --workdir /path/to/project
```

Auto-approve tool calls:

```bash
cargo run -p robit -- --auto-approve
```

Install locally from this checkout:

```bash
cargo install --path crates/robit-tui
robit
```

> After publishing to crates.io, the intended installation command is `cargo install robit`.

### Run the Desktop GUI

```bash
cargo run -p robit-gui
```

By default, GUI session history is stored at `<workdir>/.robit/memory/robit.db`. Use global storage when you want all projects to share one session database:

```bash
cargo run -p robit-gui -- --global-storage
```

With global storage enabled, the GUI uses `~/.robit/memory/robit.db`.

The GUI frontend builds and loads the React app according to [crates/robit-gui/tauri.conf.json](crates/robit-gui/tauri.conf.json).

## Configuration

robit uses a unified `config.toml` file. The lookup order is:

1. `workdir/.robit/config.toml` or `.robit/config.toml` in the current directory
2. `~/.robit/config.toml`

API keys support `${ENV_VAR}` substitution. robit also attempts to load `~/.robit/.env` automatically.

Minimal configuration example:

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
global_storage = false  # true stores GUI sessions in ~/.robit/memory/robit.db

[app.context]
max_output_lines = 500
max_output_bytes = 51200
reserve_ratio = 0.2
```

Store secrets in `~/.robit/.env`:

```bash
DEEPSEEK_API_KEY=your-api-key
```

## TUI Commands and Shortcuts

After starting `robit`, enter natural-language tasks directly or use slash commands:

| Command | Description |
| --- | --- |
| `/exit`, `/quit` | Exit the application |
| `/clear` | Clear the current conversation history |
| `/model` | Show the current model |
| `/tools` | Show the number of enabled tools |
| `/skills` | Show available skills |
| `/scroll` | Toggle scroll browsing mode |

Keyboard shortcuts:

- `Enter`: send message
- `Tab`: toggle single-line/multi-line input
- `Ctrl+J`: send message in multi-line mode
- `Ctrl+C`: cancel the current task while the agent is busy
- `Ctrl+D`: exit the application
- `F8`: toggle scroll mode
- `Y` / `N`: approve or reject a tool call that requires confirmation

## Tool System

Built-in tools:

| Tool | Description |
| --- | --- |
| `read` | Read file contents with output truncation |
| `bash` | Execute shell commands |
| `write` | Create or overwrite files |
| `edit` | Perform exact find-and-replace edits |
| `grep` | Search file contents |
| `find` | Find files by pattern |
| `ls` | List directory contents |
| `load_skill` | Load skill content; always enabled |

`read` and `load_skill` are always registered. Other tools can be controlled through `[app].enabled_tools`.

## Skill System

Skills are prompt templates stored as Markdown/YAML files. They can be placed in:

```text
~/.robit/skills/
.robit/skills/
```

Use `/skills` in the TUI to inspect loaded skills. See [docs/architecture.md](docs/architecture.md) for the skill file format and registration mechanism.

## Release Guidance

- `robit-ai` and `robit-agent`: suitable for crates.io as Rust library crates.
- `robit`: suitable for crates.io as the terminal application; users can install it with `cargo install robit`.
- `robit-gui`: better distributed through GitHub Releases, a website, Homebrew Cask, Winget, Scoop, AppImage, deb/rpm, dmg/msi, or similar desktop distribution channels.

## Documentation

- [docs/architecture.md](docs/architecture.md): agent runtime, Frontend trait, tool system, skill system, and context management
- [docs/protocol.md](docs/protocol.md): message structures and agent events
- [docs/roadmap.md](docs/roadmap.md): project roadmap
- [docs/plans/phase2-implementation.md](docs/plans/phase2-implementation.md): agent runtime implementation plan
- [docs/superpowers/2026-06-11-robit-gui-progress.md](docs/superpowers/2026-06-11-robit-gui-progress.md): GUI development progress

## Project Status

robit is in early development. The core LLM API, agent runtime, and TUI frontend are already usable at a basic level; the GUI frontend is still being iterated on. APIs, configuration format, and release strategy may change before a stable release.

## License

This project is licensed under the [Apache License 2.0](LICENSE).

Apache-2.0 allows use, copy, modification, and distribution, including commercial use. When redistributing, preserve the copyright notice and license text, and comply with the patent grant, NOTICE, and other terms of the license.
