# robit-gui --workdir Flag Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `--workdir / -w` command line flag to robit-gui to specify the working directory for the agent.

**Architecture:** 
- Use `clap` to parse CLI arguments (following robit-tui pattern)
- Pass working directory from main.rs to AppState::new()
- Validate workdir exists and is a directory if provided
- Canonicalize the path for consistency
- If not specified, fall back to current directory (existing behavior)

**Tech Stack:** Rust, clap, Tauri

---

### Task 1: Add clap dependency to Cargo.toml

**Files:**
- Modify: `crates/robit-gui/Cargo.toml`

- [ ] **Step 1: Add clap dependency**

Add to `[dependencies]` section:

```toml
clap = { workspace = true, features = ["derive"] }
```

- [ ] **Step 2: Verify the change**

Check that `clap` is now in dependencies.

- [ ] **Step 3: Commit**

```bash
git add crates/robit-gui/Cargo.toml
git commit -m "feat(robit-gui): add clap dependency for CLI args"
```

---

### Task 2: Modify AppState::new() to accept working_dir

**Files:**
- Modify: `crates/robit-gui/src/state.rs`

- [ ] **Step 1: Update AppState::new() signature**

Change from:
```rust
pub fn new(
    db_path: PathBuf,
    llm_client: Arc<LlmClient>,
    config: RobitConfig,
) -> Result<Self, String> {
```

To:
```rust
pub fn new(
    db_path: PathBuf,
    llm_client: Arc<LlmClient>,
    config: RobitConfig,
    working_dir: Option<PathBuf>,
) -> Result<Self, String> {
```

- [ ] **Step 2: Update working_dir initialization with validation**

Replace line 102:
```rust
let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
```

With:
```rust
// Resolve and validate working directory
let working_dir = match working_dir {
    Some(path) => {
        if !path.exists() {
            return Err(format!("Working directory does not exist: {}", path.display()));
        }
        if !path.is_dir() {
            return Err(format!("Path is not a directory: {}", path.display()));
        }
        // Canonicalize to get absolute path (resolves symlinks, etc.)
        std::fs::canonicalize(path)
            .map_err(|e| format!("Failed to resolve working directory path: {}", e))?
    }
    None => {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }
};
```

Also, since working_dir is now passed in, project_skills_dir should use the passed working_dir (this is already correct on line 117).

- [ ] **Step 3: Verify the changes**

Check that `AppState::new()` now accepts `working_dir: Option<PathBuf>` and uses it when provided.

- [ ] **Step 4: Commit**

```bash
git add crates/robit-gui/src/state.rs
git commit -m "feat(robit-gui): make AppState::new() accept working_dir parameter"
```

---

### Task 3: Add CLI argument parsing to main.rs

**Files:**
- Modify: `crates/robit-gui/src/main.rs`

- [ ] **Step 1: Add clap import**

At the top with other imports:
```rust
use clap::Parser;
```

- [ ] **Step 2: Add Cli struct**

Add before `fn main()`:
```rust
#[derive(Debug, Parser)]
#[command(name = "robit-gui")]
#[command(about = "AI Programming Agent with GUI")]
struct Cli {
    /// Working directory for the agent
    #[arg(long, short = 'w')]
    workdir: Option<std::path::PathBuf>,
}
```

- [ ] **Step 3: Parse CLI args in main()**

Add at the beginning of `main()`:
```rust
let cli = Cli::parse();
```

- [ ] **Step 4: Pass workdir to AppState::new()**

Change line 38 from:
```rust
let app_state = AppState::new(db_path, client, config).expect("Failed to initialize app state");
```

To:
```rust
let app_state = AppState::new(db_path, client, config, cli.workdir).expect("Failed to initialize app state");
```

- [ ] **Step 5: Verify the complete main.rs**

The full main.rs should look like:
```rust
//! robit-gui — Tauri v2 desktop GUI for the Robit AI programming agent.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(dead_code)] // Allow unused code for now, will be used in UI

mod commands;
mod config;
mod db;
mod events;
mod frontend;
mod state;

use std::sync::Arc;

use clap::Parser;
use robit_ai::config::load_config;
use robit_ai::LlmClient;

use state::AppState;

#[derive(Debug, Parser)]
#[command(name = "robit-gui")]
#[command(about = "AI Programming Agent with GUI")]
struct Cli {
    /// Working directory for the agent
    #[arg(long, short = 'w')]
    workdir: Option<std::path::PathBuf>,
}

fn main() {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("robit_gui=info".parse().unwrap()),
        )
        .init();

    let config = load_config().expect("Failed to load robit.toml configuration");
    let client = Arc::new(
        LlmClient::from_config(&config, None).expect("Failed to initialize LLM client"),
    );

    let db_path = dirs::home_dir()
        .expect("Cannot determine home directory")
        .join(".robit")
        .join("robit.db");

    let app_state = AppState::new(db_path, client, config, cli.workdir)
        .expect("Failed to initialize app state");

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::create_session,
            commands::list_sessions,
            commands::switch_session,
            commands::send_message,
            commands::cancel,
            commands::delete_session,
            commands::rename_session,
            commands::get_messages,
            commands::confirm_tool,
            commands::get_config,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 6: Build to verify**

```bash
cargo build -p robit-gui
```

Expected: Build succeeds.

- [ ] **Step 7: Commit**

```bash
git add crates/robit-gui/src/main.rs
git commit -m "feat(robit-gui): add --workdir/-w CLI flag"
```

---

### Task 4: Test the implementation

**Files:**
- No new files, test the binary

- [ ] **Step 1: Build robit-gui**

```bash
cargo build -p robit-gui
```

- [ ] **Step 2: Test help output**

```bash
cargo run -p robit-gui -- --help
```

Expected: Shows help with `--workdir` and `-w` options.

- [ ] **Step 3: Test with specific directory**

```bash
cargo run -p robit-gui -- --workdir "e:\GitHub\robit"
```

(or a directory of your choice)

Verify the agent uses that directory as working directory for file operations.

- [ ] **Step 4: Test without flag (default behavior)**

```bash
cargo run -p robit-gui
```

Verify it uses current directory (existing behavior preserved).

- [ ] **Step 5: Test with non-existent directory**

```bash
cargo run -p robit-gui -- --workdir "nonexistent\path"
```

Expected: Error message "Working directory does not exist: nonexistent\path"

- [ ] **Step 6: Test with file instead of directory**

First create a temporary file, then:
```bash
cargo run -p robit-gui -- --workdir "Cargo.toml"
```

Expected: Error message "Path is not a directory: Cargo.toml"

---

## Self-Review

**1. Spec coverage:** 
- ✅ Add `--workdir / -w` flag - covered in Task 3
- ✅ Pass working directory to AppState - covered in Task 2
- ✅ Validate workdir exists and is a directory - covered in Task 2
- ✅ Canonicalize path for consistency - covered in Task 2
- ✅ Fall back to current directory if not specified - covered in Task 2

**2. Placeholder scan:**
- ✅ No TBD/TODO
- ✅ All code blocks complete
- ✅ All steps have exact content

**3. Type consistency:**
- ✅ `working_dir: Option<PathBuf>` used consistently
- ✅ Parameter names match across files
