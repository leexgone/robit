# robit-gui Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Tauri v2 desktop GUI frontend for Robit that implements the `Frontend` trait, supports multi-session management with SQLite persistence, parallel Agent execution, and a modern React + shadcn/ui interface.

**Architecture:** Pure additive — new `crates/robit-gui` crate in the workspace. Rust backend manages `AppState` (sessions, agents, SQLite), implements `Frontend` trait as `GuiFrontend`, and exposes Tauri commands + events. React frontend handles UI rendering via Zustand state + shadcn/ui components. Zero changes to `robit-agent`, `robit-ai`, or `robit-tui`.

**Tech Stack:** Tauri v2, Rust (rusqlite, tokio), React 19 + TypeScript, Vite, Tailwind CSS v4, shadcn/ui, Zustand v5, next-themes

---

## File Structure Map

```
crates/robit-gui/                          ← NEW: entire crate
├── Cargo.toml                             ← dependencies
├── tauri.conf.json                        ← Tauri v2 config
├── build.rs                               ← Tauri build script
├── capabilities/default.json              ← Tauri v2 permissions
├── icons/                                 ← app icons (Tauri default)
├── src/
│   ├── main.rs                            ← Tauri entry: builder, setup, run
│   ├── lib.rs                             ← module declarations
│   ├── events.rs                          ← UiEvent enum (Serialize, tag-based)
│   ├── db.rs                              ← SQLite: init, CRUD for sessions & messages
│   ├── state.rs                           ← AppState + AgentHandle + AgentStatus
│   ├── frontend.rs                        ← GuiFrontend: Frontend trait impl
│   ├── commands.rs                        ← Tauri #[tauri::command] functions
│   └── config.rs                          ← ConfigInfo (non-sensitive config for frontend)
├── ui/                                    ← NEW: React frontend
│   ├── package.json
│   ├── tsconfig.json
│   ├── vite.config.ts
│   ├── index.html
│   └── src/
│       ├── main.tsx                       ← React entry, ThemeProvider, Tauri listeners
│       ├── App.tsx                        ← Root layout: StatusBar + sidebar + ChatPanel
│       ├── components/
│       │   ├── StatusBar.tsx              ← Top bar: model, tokens, theme toggle
│       │   ├── SessionSidebar.tsx         ← Left panel: session list, resizable
│       │   ├── SessionItem.tsx            ← Single session entry with context menu
│       │   ├── ChatPanel.tsx              ← Right panel: messages + input
│       │   ├── MessageList.tsx            ← Scrollable message container
│       │   ├── UserMessage.tsx            ← User message bubble
│       │   ├── AssistantMessage.tsx       ← AI response (Markdown rendered)
│       │   ├── ToolCard.tsx               ← Tool call/result card
│       │   ├── InputArea.tsx              ← Bottom input with send button
│       │   ├── ThemeToggle.tsx            ← Light/dark mode switch
│       │   └── ui/                        ← shadcn/ui components (copied)
│       │       ├── button.tsx
│       │       ├── input.tsx
│       │       ├── scroll-area.tsx
│       │       ├── dropdown-menu.tsx
│       │       ├── alert-dialog.tsx
│       │       ├── tooltip.tsx
│       │       └── separator.tsx
│       ├── lib/
│       │   ├── store.ts                   ← Zustand store
│       │   ├── commands.ts                ← Typed Tauri invoke wrappers
│       │   └── types.ts                   ← TypeScript type definitions
│       └── styles/
│           └── globals.css                ← Tailwind + shadcn/ui theme vars
└── tests/                                 ← NEW: Rust integration tests
    └── integration.rs                     ← Tests for commands, frontend, state
```

**Modified files (workspace only):**
- `Cargo.toml` (root) — add `"crates/robit-gui"` to workspace members

---

### Task 1: Scaffold Tauri v2 Project with Rust Crate

**Files:**
- Create: `crates/robit-gui/Cargo.toml`
- Create: `crates/robit-gui/tauri.conf.json`
- Create: `crates/robit-gui/build.rs`
- Create: `crates/robit-gui/capabilities/default.json`
- Create: `crates/robit-gui/src/main.rs`
- Create: `crates/robit-gui/src/lib.rs`
- Modify: `Cargo.toml` (root) — add workspace member

- [ ] **Step 1: Add workspace member to root Cargo.toml**

Add `"crates/robit-gui"` to the `members` array in `Cargo.toml`:

```toml
members = [
    "crates/robit-ai",
    "crates/robit-agent",
    "crates/robit-tui",
    "crates/robit-gui",
    "examples/robit-chat",
    "examples/robit-agent",
]
```

- [ ] **Step 2: Create crates/robit-gui/Cargo.toml**

```toml
[package]
name = "robit-gui"
version = "0.1.0"
edition = "2021"

[lib]
name = "robit_gui"
crate-type = ["lib", "cdylib", "staticlib"]

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
robit-agent = { path = "../robit-agent" }
robit-ai = { path = "../robit-ai" }
tauri = { version = "2", features = [] }
tauri-plugin-shell = "2"
rusqlite = { version = "0.31", features = ["bundled"] }
tokio.workspace = true
tokio-util = "0.7"
serde.workspace = true
serde_json.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
uuid.workspace = true
anyhow.workspace = true
dirs.workspace = true
```

- [ ] **Step 3: Create crates/robit-gui/build.rs**

```rust
fn main() {
    tauri_build::build()
}
```

- [ ] **Step 4: Create crates/robit-gui/tauri.conf.json**

```json
{
  "$schema": "https://raw.githubusercontent.com/nicedoc/tauri-docs-v2/main/src/content/docs/_schema/config.schema.json",
  "productName": "Robit",
  "version": "0.1.0",
  "identifier": "com.robit.app",
  "build": {
    "frontendDist": "../ui/dist",
    "devUrl": "http://localhost:1420",
    "beforeDevCommand": "cd ui && npm run dev",
    "beforeBuildCommand": "cd ui && npm run build"
  },
  "app": {
    "title": "Robit",
    "windows": [
      {
        "title": "Robit - AI Programming Agent",
        "width": 1200,
        "height": 800,
        "minWidth": 800,
        "minHeight": 600
      }
    ],
    "security": {
      "csp": "default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self'; img-src 'self' data: https:; connect-src 'self' ipc: http://ipc.localhost"
    }
  },
  "plugins": {
    "shell": {
      "open": true
    }
  }
}
```

- [ ] **Step 5: Create crates/robit-gui/capabilities/default.json**

```json
{
  "$schema": "https://raw.githubusercontent.com/nicedoc/tauri-docs-v2/main/src/content/docs/_schema/capability.schema.json",
  "identifier": "default",
  "windows": ["main"],
  "permissions": [
    "core:default",
    "shell:allow-open"
  ]
}
```

- [ ] **Step 6: Create crates/robit-gui/src/lib.rs**

```rust
pub mod commands;
pub mod config;
pub mod db;
pub mod events;
pub mod frontend;
pub mod state;
```

- [ ] **Step 7: Create minimal crates/robit-gui/src/main.rs**

```rust
//! robit-gui — Tauri v2 desktop GUI for the Robit AI programming agent.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;
mod db;
mod events;
mod frontend;
mod state;

use std::sync::Arc;

use robit_ai::config::load_config;
use robit_ai::LlmClient;
use tauri::Manager;

use state::AppState;

fn main() {
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

    let app_state = AppState::new(db_path, client, config).expect("Failed to initialize app state");

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

- [ ] **Step 8: Verify crate compiles (will fail until all modules exist, but should parse)**

Run: `cargo check -p robit-gui 2>&1 | head -20`
Expected: Errors about missing modules (events, db, state, etc.) — this confirms the crate is wired correctly.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml crates/robit-gui/Cargo.toml crates/robit-gui/tauri.conf.json crates/robit-gui/build.rs crates/robit-gui/capabilities/default.json crates/robit-gui/src/main.rs crates/robit-gui/src/lib.rs
git commit -m "feat(robit-gui): scaffold Tauri v2 project with workspace integration"
```

---

### Task 2: UiEvent Types and DB Schema

**Files:**
- Create: `crates/robit-gui/src/events.rs`
- Create: `crates/robit-gui/src/db.rs`
- Create: `crates/robit-gui/src/config.rs`

- [ ] **Step 1: Create crates/robit-gui/src/events.rs**

```rust
use serde::Serialize;

/// Events pushed from Rust backend to React frontend via Tauri events.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum UiEvent {
    /// Streaming text delta from LLM response.
    TextDelta {
        session_id: String,
        delta: String,
    },
    /// LLM requested a tool call.
    ToolCallRequested {
        session_id: String,
        tool_call_id: String,
        name: String,
        arguments: String,
        requires_confirm: bool,
    },
    /// Tool execution completed.
    ToolCallResult {
        session_id: String,
        tool_call_id: String,
        content: String,
        is_error: bool,
    },
    /// Current turn is complete.
    TurnComplete {
        session_id: String,
    },
    /// An error occurred.
    Error {
        session_id: String,
        message: String,
    },
    /// A skill was triggered.
    SkillTriggered {
        session_id: String,
        name: String,
        description: String,
    },
}

/// Session metadata returned to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub model: String,
    pub status: String,       // "idle" | "ready" | "running"
    pub created_at: String,
    pub updated_at: String,
}

/// Message data returned to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct MessageData {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<String>,
    pub created_at: String,
}

/// Non-sensitive configuration exposed to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigInfo {
    pub model: String,
    pub version: String,
    pub tools_enabled: usize,
    pub tools_total: usize,
    pub auto_approve: bool,
}
```

- [ ] **Step 2: Create crates/robit-gui/src/db.rs**

```rust
use rusqlite::{Connection, Result as SqliteResult, params};
use std::path::Path;

use crate::events::{MessageData, SessionInfo};

/// Initialize the database: create tables and indexes if they don't exist.
pub fn init_db(conn: &Connection) -> SqliteResult<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS sessions (
            id          TEXT PRIMARY KEY,
            title       TEXT NOT NULL,
            model       TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            is_active   INTEGER DEFAULT 1
        );

        CREATE TABLE IF NOT EXISTS messages (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id   TEXT NOT NULL REFERENCES sessions(id),
            role         TEXT NOT NULL,
            content      TEXT NOT NULL,
            tool_name    TEXT,
            tool_call_id TEXT,
            tokens       INTEGER,
            created_at   TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_messages_session
            ON messages(session_id);
        CREATE INDEX IF NOT EXISTS idx_messages_created
            ON messages(session_id, created_at);
        "
    )?;
    Ok(())
}

/// Insert a new session.
pub fn insert_session(conn: &Connection, id: &str, title: &str, model: &str) -> SqliteResult<()> {
    let now = chrono_now();
    conn.execute(
        "INSERT INTO sessions (id, title, model, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, title, model, now, now],
    )?;
    Ok(())
}

/// List all active sessions, ordered by most recently updated.
pub fn list_sessions(conn: &Connection) -> SqliteResult<Vec<SessionInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, model, created_at, updated_at FROM sessions WHERE is_active = 1 ORDER BY updated_at DESC"
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(SessionInfo {
            id: row.get(0)?,
            title: row.get(1)?,
            model: row.get(2)?,
            status: "idle".to_string(),
            created_at: row.get(3)?,
            updated_at: row.get(4)?,
        })
    })?;
    rows.collect()
}

/// Get a single session by ID.
pub fn get_session(conn: &Connection, id: &str) -> SqliteResult<Option<SessionInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, model, created_at, updated_at FROM sessions WHERE id = ?1 AND is_active = 1"
    )?;
    let mut rows = stmt.query_map(params![id], |row| {
        Ok(SessionInfo {
            id: row.get(0)?,
            title: row.get(1)?,
            model: row.get(2)?,
            status: "idle".to_string(),
            created_at: row.get(3)?,
            updated_at: row.get(4)?,
        })
    })?;
    match rows.next() {
        Some(Ok(session)) => Ok(Some(session)),
        _ => Ok(None),
    }
}

/// Update a session's title.
pub fn update_session_title(conn: &Connection, id: &str, title: &str) -> SqliteResult<()> {
    let now = chrono_now();
    conn.execute(
        "UPDATE sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
        params![title, now, id],
    )?;
    Ok(())
}

/// Update a session's updated_at timestamp.
pub fn touch_session(conn: &Connection, id: &str) -> SqliteResult<()> {
    let now = chrono_now();
    conn.execute(
        "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
        params![now, id],
    )?;
    Ok(())
}

/// Soft-delete a session.
pub fn delete_session(conn: &Connection, id: &str) -> SqliteResult<()> {
    conn.execute(
        "UPDATE sessions SET is_active = 0 WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

/// Insert a message into a session.
pub fn insert_message(
    conn: &Connection,
    session_id: &str,
    role: &str,
    content: &str,
    tool_name: Option<&str>,
    tool_call_id: Option<&str>,
) -> SqliteResult<i64> {
    let now = chrono_now();
    conn.execute(
        "INSERT INTO messages (session_id, role, content, tool_name, tool_call_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![session_id, role, content, tool_name, tool_call_id, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Get all messages for a session, ordered by creation time.
pub fn get_messages(conn: &Connection, session_id: &str) -> SqliteResult<Vec<MessageData>> {
    let mut stmt = conn.prepare(
        "SELECT id, role, content, tool_name, tool_call_id, created_at FROM messages WHERE session_id = ?1 ORDER BY id ASC"
    )?;
    let rows = stmt.query_map(params![session_id], |row| {
        Ok(MessageData {
            id: row.get(0)?,
            role: row.get(1)?,
            content: row.get(2)?,
            tool_name: row.get(3)?,
            tool_call_id: row.get(4)?,
            created_at: row.get(5)?,
        })
    })?;
    rows.collect()
}

/// Get an ISO 8601 timestamp string.
fn chrono_now() -> String {
    // Avoid pulling in chrono dependency — use std::time + manual formatting.
    // For simplicity in MVP, format via SystemTime.
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();

    // Simple ISO 8601-like format: YYYY-MM-DDTHH:MM:SS
    // We use a simple approach: convert to days since epoch
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Calculate year/month/day from days since Unix epoch
    let (year, month, day) = days_to_date(days as i64);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        year, month, day, hours, minutes, seconds
    )
}

fn days_to_date(mut days: i64) -> (i64, u32, u32) {
    days += 719468; // Shift epoch to year 0
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { (mp + 3) as u32 } else { (mp - 9) as u32 };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}
```

- [ ] **Step 3: Create crates/robit-gui/src/config.rs**

```rust
use crate::events::ConfigInfo;

/// Build ConfigInfo from loaded configuration.
pub fn build_config_info(config: &robit_ai::config::RobitConfig) -> ConfigInfo {
    let model = config
        .default_model
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    let auto_approve = config
        .app
        .as_ref()
        .and_then(|a| a.auto_approve)
        .unwrap_or(false);

    ConfigInfo {
        model,
        version: env!("CARGO_PKG_VERSION").to_string(),
        tools_enabled: 0, // Updated after tool registry is built
        tools_total: 0,
        auto_approve,
    }
}
```

- [ ] **Step 4: Verify compilation of new modules**

Run: `cargo check -p robit-gui`
Expected: Compilation errors about missing `state` and `frontend` and `commands` modules — expected at this stage.

- [ ] **Step 5: Commit**

```bash
git add crates/robit-gui/src/events.rs crates/robit-gui/src/db.rs crates/robit-gui/src/config.rs
git commit -m "feat(robit-gui): add UiEvent types, SQLite DB layer, and config module"
```

---

### Task 3: AppState and AgentHandle

**Files:**
- Create: `crates/robit-gui/src/state.rs`

- [ ] **Step 1: Create crates/robit-gui/src/state.rs**

```rust
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use robit_agent::agent::Agent;
use robit_agent::event::{FrontendMessage, SessionId, new_session_id};
use robit_agent::frontend::Frontend;
use robit_agent::skill::SkillRegistry;
use robit_agent::tool::bash::BashTool;
use robit_agent::tool::edit::EditTool;
use robit_agent::tool::load_skill::LoadSkillTool;
use robit_agent::tool::read::ReadTool;
use robit_agent::tool::write::WriteTool;
use robit_agent::ToolRegistry;
use robit_ai::config::RobitConfig;
use robit_ai::LlmClient;
use rusqlite::Connection;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::db;
use crate::events::{ConfigInfo, SessionInfo};
use crate::frontend::GuiFrontend;

/// Agent run status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Ready,
    Running,
}

/// Handle to a running Agent instance for one session.
pub struct AgentHandle {
    /// Send messages to the Agent loop.
    pub message_tx: mpsc::Sender<FrontendMessage>,
    /// Current status.
    pub status: AgentStatus,
    /// Cancel token for interrupting a running Agent.
    pub cancel_token: CancellationToken,
}

/// Application state managed by Tauri.
pub struct AppState {
    /// SQLite connection (Mutex-protected, single connection).
    pub db: Mutex<Connection>,

    /// Shared LLM client (reused across all sessions).
    pub llm_client: Arc<LlmClient>,

    /// Shared tool registry (reused across all sessions).
    pub tool_registry: Arc<ToolRegistry>,

    /// Shared skill registry.
    pub skill_registry: Arc<SkillRegistry>,

    /// Active Agent handles, keyed by session ID.
    pub agents: Mutex<HashMap<SessionId, AgentHandle>>,

    /// Currently active session ID.
    pub active_session: Mutex<Option<SessionId>>,

    /// Loaded configuration.
    pub config: RobitConfig,

    /// Working directory.
    pub working_dir: PathBuf,

    /// Auto-approve all tool calls.
    pub auto_approve: bool,

    /// Context config.
    pub context_config: Option<robit_ai::config::ContextConfig>,

    /// Context window from resolved model.
    pub context_window: Option<u64>,
}

impl AppState {
    /// Create a new AppState, initializing the database.
    pub fn new(
        db_path: PathBuf,
        llm_client: Arc<LlmClient>,
        config: RobitConfig,
    ) -> Result<Self, String> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create DB dir: {}", e))?;
        }

        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open database: {}", e))?;

        db::init_db(&conn).map_err(|e| format!("Failed to init database: {}", e))?;

        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        let auto_approve = config
            .app
            .as_ref()
            .and_then(|a| a.auto_approve)
            .unwrap_or(false);

        let context_config = config
            .app
            .as_ref()
            .and_then(|a| a.context.clone());

        let context_window = llm_client.resolved().context_window;

        // Load skills
        let global_skills_dir = dirs::home_dir().map(|h| h.join(".robit/skills"));
        let project_skills_dir = Some(working_dir.join(".robit/skills"));

        let (skills, skill_errors) = robit_agent::skill::loader::load_skills(
            global_skills_dir,
            project_skills_dir,
        );

        for err in &skill_errors {
            tracing::warn!("Skill load error: {:?}", err);
        }

        let enabled_skills = config
            .app
            .as_ref()
            .and_then(|a| a.enabled_skills.as_ref());

        let filtered_skills: Vec<_> = match enabled_skills {
            Some(list) => skills
                .into_iter()
                .filter(|s| list.contains(&s.frontmatter.name))
                .collect(),
            None => skills,
        };

        let base_tool_names = ["read", "bash", "write", "edit"];
        let skill_registry = Arc::new(SkillRegistry::new(filtered_skills, &base_tool_names));

        let tool_registry = Arc::new(create_tools(&config, Arc::clone(&skill_registry)));

        Ok(Self {
            db: Mutex::new(conn),
            llm_client,
            tool_registry,
            skill_registry,
            agents: Mutex::new(HashMap::new()),
            active_session: Mutex::new(None),
            config,
            working_dir,
            auto_approve,
            context_config,
            context_window,
        })
    }

    /// Get the session list from the database, merging with in-memory Agent statuses.
    pub fn session_list(&self) -> Result<Vec<SessionInfo>, String> {
        let db = self.db.lock().map_err(|e| format!("DB lock: {}", e))?;
        let mut sessions = db::list_sessions(&db).map_err(|e| format!("DB error: {}", e))?;

        let agents = self.agents.lock().map_err(|e| format!("Agents lock: {}", e))?;
        for session in &mut sessions {
            if let Some(handle) = agents.get(&session.id) {
                session.status = match handle.status {
                    AgentStatus::Idle => "idle",
                    AgentStatus::Ready => "ready",
                    AgentStatus::Running => "running",
                }.to_string();
            }
        }

        Ok(sessions)
    }

    /// Build ConfigInfo for the frontend.
    pub fn config_info(&self) -> ConfigInfo {
        let tools = self.tool_registry.tool_names();
        let mut info = crate::config::build_config_info(&self.config);
        info.tools_enabled = tools.len();
        info.tools_total = tools.len();
        info
    }

    /// Create an Agent for a session and spawn its background task.
    pub fn spawn_agent(
        &self,
        session_id: &str,
        app_handle: &tauri::AppHandle,
    ) -> Result<AgentHandle, String> {
        let (event_tx, mut event_rx) = mpsc::channel::<crate::events::UiEvent>(64);
        let (message_tx, message_rx) = mpsc::channel::<FrontendMessage>(16);

        let gui_frontend = Arc::new(GuiFrontend {
            event_tx,
            confirmations: Mutex::new(HashMap::new()),
            session_id: session_id.to_string(),
        });

        let working_dir = self.working_dir.clone();
        let auto_approve = self.auto_approve;
        let llm_client = Arc::clone(&self.llm_client);
        let tools = Arc::clone(&self.tool_registry);
        let skills = Arc::clone(&self.skill_registry);
        let context_config = self.context_config.clone();
        let context_window = self.context_window;

        let agent = Agent::new(
            llm_client,
            tools,
            skills,
            gui_frontend,
            context_config.as_ref(),
            context_window,
            working_dir,
            auto_approve,
        );

        let cancel_token = CancellationToken::new();
        let ct = cancel_token.clone();
        let sid = session_id.to_string();

        // Spawn the Agent loop in a background task
        tokio::spawn(async move {
            agent.run(message_rx).await;
            tracing::info!("Agent task ended for session {}", sid);
        });

        // Spawn event bridge: receives UiEvents and emits to Tauri frontend
        let app_handle_clone = app_handle.clone();
        let sid_clone = session_id.to_string();
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let _ = app_handle_clone.emit("agent-event", &event);
            }
            tracing::info!("Event bridge ended for session {}", sid_clone);
        });

        // Spawn cancel watcher
        let ct_watcher = cancel_token.clone();
        let msg_tx = message_tx.clone();
        tokio::spawn(async move {
            ct_watcher.cancelled().await;
            let _ = msg_tx.send(FrontendMessage::Cancel).await;
        });

        Ok(AgentHandle {
            message_tx,
            status: AgentStatus::Ready,
            cancel_token,
        })
    }
}

/// Build the tool registry (same as robit-tui).
fn create_tools(
    config: &RobitConfig,
    skills: Arc<SkillRegistry>,
) -> ToolRegistry {
    let mut tools = ToolRegistry::new();
    let context_config = config.app.as_ref().and_then(|a| a.context.as_ref());
    let max_lines = context_config.and_then(|c| c.max_output_lines).unwrap_or(500);
    let max_bytes = context_config
        .and_then(|c| c.max_output_bytes)
        .unwrap_or(51200);
    tools.register(ReadTool::new(max_lines, max_bytes));
    tools.register(BashTool::new(max_bytes));
    tools.register(WriteTool::new());
    tools.register(EditTool::new());
    tools.register(LoadSkillTool::new(skills));
    tools
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p robit-gui`
Expected: Only `commands.rs` and `frontend.rs` missing errors should remain.

- [ ] **Step 3: Commit**

```bash
git add crates/robit-gui/src/state.rs
git commit -m "feat(robit-gui): add AppState and AgentHandle with Agent lifecycle management"
```

---

### Task 4: GuiFrontend — Frontend Trait Implementation

**Files:**
- Create: `crates/robit-gui/src/frontend.rs`

- [ ] **Step 1: Create crates/robit-gui/src/frontend.rs**

```rust
use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};

use robit_agent::error::Result;
use robit_agent::event::AgentEvent;
use robit_agent::frontend::Frontend;
use robit_agent::tool::ToolCallInfo;

use crate::events::UiEvent;

/// Frontend trait implementation for the Tauri GUI.
///
/// Each session has its own GuiFrontend instance. Events are pushed via
/// a channel to a background task that emits Tauri events to the WebView.
/// Tool confirmations use oneshot channels for blocking wait.
pub struct GuiFrontend {
    /// Send UiEvents to the Tauri event bridge task.
    pub event_tx: mpsc::Sender<UiEvent>,

    /// Pending tool confirmation responders, keyed by tool_call_id.
    pub confirmations: Mutex<HashMap<String, oneshot::Sender<bool>>>,

    /// The session this frontend belongs to.
    pub session_id: String,
}

#[async_trait]
impl Frontend for GuiFrontend {
    async fn on_event(&self, event: AgentEvent) -> Result<()> {
        let ui_event = match event {
            AgentEvent::TextDelta(delta) => UiEvent::TextDelta {
                session_id: self.session_id.clone(),
                delta,
            },
            AgentEvent::ToolCallRequested {
                tool_call_id,
                name,
                arguments,
            } => {
                // Determine if this tool needs confirmation
                // We don't have access to ToolRegistry here, so we mark based on tool name.
                // Write tools (bash, write, edit) require confirmation by default.
                let requires_confirm = matches!(
                    name.as_str(),
                    "bash" | "write" | "edit"
                );
                UiEvent::ToolCallRequested {
                    session_id: self.session_id.clone(),
                    tool_call_id,
                    name,
                    arguments,
                    requires_confirm,
                }
            }
            AgentEvent::ToolCallResult {
                tool_call_id,
                result,
            } => UiEvent::ToolCallResult {
                session_id: self.session_id.clone(),
                tool_call_id,
                content: result.content,
                is_error: result.is_error,
            },
            AgentEvent::TurnComplete => UiEvent::TurnComplete {
                session_id: self.session_id.clone(),
            },
            AgentEvent::Error(e) => UiEvent::Error {
                session_id: self.session_id.clone(),
                message: e.to_string(),
            },
            AgentEvent::SkillTriggered { name, description } => UiEvent::SkillTriggered {
                session_id: self.session_id.clone(),
                name,
                description,
            },
        };

        let _ = self.event_tx.send(ui_event).await;
        Ok(())
    }

    async fn request_tool_confirmation(&self, info: &ToolCallInfo) -> Result<bool> {
        let (tx, rx) = oneshot::channel();

        // Store the sender so the confirm_tool command can resolve it
        {
            let mut confirmations = self
                .confirmations
                .lock()
                .map_err(|e| robit_agent::AgentError::InternalError(format!("Lock error: {}", e)))?;
            confirmations.insert(info.id.clone(), tx);
        }

        // The ToolCallRequested event was already emitted by on_event before this is called.
        // We just wait for the oneshot response from confirm_tool command.
        let approved = rx
            .await
            .unwrap_or(false);

        // Clean up
        if let Ok(mut confirmations) = self.confirmations.lock() {
            confirmations.remove(&info.id);
        }

        Ok(approved)
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p robit-gui`
Expected: Only `commands.rs` missing errors should remain.

- [ ] **Step 3: Commit**

```bash
git add crates/robit-gui/src/frontend.rs
git commit -m "feat(robit-gui): add GuiFrontend implementing Frontend trait with oneshot confirmations"
```

---

### Task 5: Tauri Commands

**Files:**
- Create: `crates/robit-gui/src/commands.rs`
- Modify: `crates/robit-gui/src/main.rs` (minor: update import paths if needed)

- [ ] **Step 1: Create crates/robit-gui/src/commands.rs**

```rust
use std::sync::Mutex;

use tauri::{AppHandle, State};

use robit_agent::event::{FrontendMessage, new_session_id};

use crate::db;
use crate::events::{ConfigInfo, MessageData, SessionInfo};
use crate::frontend::GuiFrontend;
use crate::state::{AgentHandle, AgentStatus, AppState};

/// Create a new session and its Agent.
#[tauri::command]
pub async fn create_session(
    app_handle: AppHandle,
    state: State<'_, AppState>,
    model: String,
) -> Result<SessionInfo, String> {
    let session_id = new_session_id();
    let title = "New Session".to_string();

    // Insert into DB
    {
        let db = state.db.lock().map_err(|e| format!("DB lock: {}", e))?;
        db::insert_session(&db, &session_id, &title, &model)
            .map_err(|e| format!("DB error: {}", e))?;
    }

    // Spawn agent
    let handle = state.spawn_agent(&session_id, &app_handle)?;

    // Register in agents map
    {
        let mut agents = state.agents.lock().map_err(|e| format!("Agents lock: {}", e))?;
        agents.insert(session_id.clone(), handle);
    }

    // Set as active session
    {
        let mut active = state.active_session.lock().map_err(|e| format!("Active session lock: {}", e))?;
        *active = Some(session_id.clone());
    }

    Ok(SessionInfo {
        id: session_id,
        title,
        model,
        status: "ready".to_string(),
        created_at: String::new(),
        updated_at: String::new(),
    })
}

/// List all active sessions.
#[tauri::command]
pub async fn list_sessions(
    state: State<'_, AppState>,
) -> Result<Vec<SessionInfo>, String> {
    state.session_list()
}

/// Switch to a different session and load its message history.
#[tauri::command]
pub async fn switch_session(
    app_handle: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<MessageData>, String> {
    // Ensure Agent exists for this session (create if not)
    let needs_agent = {
        let agents = state.agents.lock().map_err(|e| format!("Agents lock: {}", e))?;
        !agents.contains_key(&session_id)
    };

    if needs_agent {
        let handle = state.spawn_agent(&session_id, &app_handle)?;
        let mut agents = state.agents.lock().map_err(|e| format!("Agents lock: {}", e))?;
        agents.insert(session_id.clone(), handle);
    }

    // Set as active session
    {
        let mut active = state.active_session.lock().map_err(|e| format!("Active session lock: {}", e))?;
        *active = Some(session_id.clone());
    }

    // Load messages from DB
    let db = state.db.lock().map_err(|e| format!("DB lock: {}", e))?;
    db::get_messages(&db, &session_id).map_err(|e| format!("DB error: {}", e))
}

/// Send a user message to the active session's Agent.
#[tauri::command]
pub async fn send_message(
    state: State<'_, AppState>,
    session_id: String,
    content: String,
) -> Result<(), String> {
    // Save user message to DB
    {
        let db = state.db.lock().map_err(|e| format!("DB lock: {}", e))?;
        db::insert_message(&db, &session_id, "user", &content, None, None)
            .map_err(|e| format!("DB error: {}", e))?;
        db::touch_session(&db, &session_id).map_err(|e| format!("DB error: {}", e))?;
    }

    // Send to Agent
    let agents = state.agents.lock().map_err(|e| format!("Agents lock: {}", e))?;
    let handle = agents
        .get(&session_id)
        .ok_or_else(|| format!("Session {} not found", session_id))?;

    handle
        .message_tx
        .send(FrontendMessage::UserInput(content))
        .await
        .map_err(|e| format!("Failed to send message: {}", e))?;

    Ok(())
}

/// Cancel the running Agent in a session.
#[tauri::command]
pub async fn cancel(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), String> {
    let agents = state.agents.lock().map_err(|e| format!("Agents lock: {}", e))?;
    if let Some(handle) = agents.get(&session_id) {
        handle.cancel_token.cancel();
    }
    Ok(())
}

/// Soft-delete a session.
#[tauri::command]
pub async fn delete_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), String> {
    // Cancel agent if running
    {
        let agents = state.agents.lock().map_err(|e| format!("Agents lock: {}", e))?;
        if let Some(handle) = agents.get(&session_id) {
            handle.cancel_token.cancel();
        }
    }

    // Remove from agents map
    {
        let mut agents = state.agents.lock().map_err(|e| format!("Agents lock: {}", e))?;
        agents.remove(&session_id);
    }

    // Soft-delete in DB
    {
        let db = state.db.lock().map_err(|e| format!("DB lock: {}", e))?;
        db::delete_session(&db, &session_id).map_err(|e| format!("DB error: {}", e))?;
    }

    // If this was the active session, switch to the nearest one
    {
        let mut active = state.active_session.lock().map_err(|e| format!("Active session lock: {}", e))?;
        if active.as_deref() == Some(&session_id) {
            let db = state.db.lock().map_err(|e| format!("DB lock: {}", e))?;
            let sessions = db::list_sessions(&db).map_err(|e| format!("DB error: {}", e))?;
            *active = sessions.first().map(|s| s.id.clone());
        }
    }

    Ok(())
}

/// Rename a session.
#[tauri::command]
pub async fn rename_session(
    state: State<'_, AppState>,
    session_id: String,
    title: String,
) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| format!("DB lock: {}", e))?;
    db::update_session_title(&db, &session_id, &title)
        .map_err(|e| format!("DB error: {}", e))
}

/// Get messages for a session.
#[tauri::command]
pub async fn get_messages(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<MessageData>, String> {
    let db = state.db.lock().map_err(|e| format!("DB lock: {}", e))?;
    db::get_messages(&db, &session_id).map_err(|e| format!("DB error: {}", e))
}

/// Respond to a tool confirmation request.
#[tauri::command]
pub async fn confirm_tool(
    state: State<'_, AppState>,
    session_id: String,
    tool_call_id: String,
    approved: bool,
) -> Result<(), String> {
    // We need to access the GuiFrontend's confirmations map.
    // Since GuiFrontend is inside the Agent (not directly accessible),
    // we store confirmations in a separate map in AppState.
    // For now, we use a simpler approach: the oneshot sender is stored
    // in GuiFrontend, and we access it through a shared map.
    //
    // Since the Agent holds the GuiFrontend via Arc, and we need to
    // resolve the oneshot from here, we add a confirmations map to AppState.

    // For this implementation, we store confirmation channels in a
    // dedicated map on AppState that the GuiFrontend and commands share.
    // This is handled by storing confirmations on AppState.
    //
    // Actually, the simplest approach: store confirmations on AppState
    // and have GuiFrontend use that instead of its own map.

    // We'll refactor in the next step. For now, this command is a placeholder.
    tracing::warn!("confirm_tool not yet wired — needs confirmations map on AppState");

    Ok(())
}

/// Get non-sensitive configuration for the frontend.
#[tauri::command]
pub async fn get_config(
    state: State<'_, AppState>,
) -> Result<ConfigInfo, String> {
    Ok(state.config_info())
}
```

- [ ] **Step 2: Refactor confirmations to live on AppState**

Update `crates/robit-gui/src/state.rs` — add `confirmations` field to `AppState`:

Add this field inside `pub struct AppState {`:

```rust
    /// Pending tool confirmation responders, keyed by "session_id:tool_call_id".
    pub confirmations: Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
```

And in `AppState::new()`, add initialization:

```rust
            confirmations: Mutex::new(HashMap::new()),
```

- [ ] **Step 3: Update GuiFrontend to use AppState confirmations**

Update `crates/robit-gui/src/frontend.rs` — change `GuiFrontend` struct:

```rust
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};

use robit_agent::error::Result;
use robit_agent::event::AgentEvent;
use robit_agent::frontend::Frontend;
use robit_agent::tool::ToolCallInfo;

use crate::events::UiEvent;

pub struct GuiFrontend {
    pub event_tx: mpsc::Sender<UiEvent>,
    /// Shared confirmations map from AppState (keyed by "session_id:tool_call_id").
    pub confirmations: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    pub session_id: String,
}

#[async_trait]
impl Frontend for GuiFrontend {
    async fn on_event(&self, event: AgentEvent) -> Result<()> {
        let ui_event = match event {
            AgentEvent::TextDelta(delta) => UiEvent::TextDelta {
                session_id: self.session_id.clone(),
                delta,
            },
            AgentEvent::ToolCallRequested {
                tool_call_id,
                name,
                arguments,
            } => {
                let requires_confirm = matches!(name.as_str(), "bash" | "write" | "edit");
                UiEvent::ToolCallRequested {
                    session_id: self.session_id.clone(),
                    tool_call_id,
                    name,
                    arguments,
                    requires_confirm,
                }
            }
            AgentEvent::ToolCallResult { tool_call_id, result } => UiEvent::ToolCallResult {
                session_id: self.session_id.clone(),
                tool_call_id,
                content: result.content,
                is_error: result.is_error,
            },
            AgentEvent::TurnComplete => UiEvent::TurnComplete {
                session_id: self.session_id.clone(),
            },
            AgentEvent::Error(e) => UiEvent::Error {
                session_id: self.session_id.clone(),
                message: e.to_string(),
            },
            AgentEvent::SkillTriggered { name, description } => UiEvent::SkillTriggered {
                session_id: self.session_id.clone(),
                name,
                description,
            },
        };
        let _ = self.event_tx.send(ui_event).await;
        Ok(())
    }

    async fn request_tool_confirmation(&self, info: &ToolCallInfo) -> Result<bool> {
        let (tx, rx) = oneshot::channel();
        let key = format!("{}:{}", self.session_id, info.id);

        {
            let mut map = self.confirmations.lock()
                .map_err(|e| robit_agent::AgentError::InternalError(format!("Lock: {}", e)))?;
            map.insert(key, tx);
        }

        let approved = rx.await.unwrap_or(false);

        // Cleanup
        let key = format!("{}:{}", self.session_id, info.id);
        if let Ok(mut map) = self.confirmations.lock() {
            map.remove(&key);
        }

        Ok(approved)
    }
}
```

- [ ] **Step 4: Update AppState::spawn_agent to use shared confirmations**

In `crates/robit-gui/src/state.rs`, update `spawn_agent` method to pass the shared confirmations map:

Change the GuiFrontend creation inside `spawn_agent` from:

```rust
        let gui_frontend = Arc::new(GuiFrontend {
            event_tx,
            confirmations: Mutex::new(HashMap::new()),
            session_id: session_id.to_string(),
        });
```

To:

```rust
        let confirmations = Arc::clone(&self.confirmations);
        let gui_frontend = Arc::new(GuiFrontend {
            event_tx,
            confirmations,
            session_id: session_id.to_string(),
        });
```

Also add `use std::sync::Arc;` import to state.rs if not already present.

- [ ] **Step 5: Update confirm_tool command**

Update `crates/robit-gui/src/commands.rs` — replace the placeholder `confirm_tool`:

```rust
/// Respond to a tool confirmation request.
#[tauri::command]
pub async fn confirm_tool(
    state: State<'_, AppState>,
    session_id: String,
    tool_call_id: String,
    approved: bool,
) -> Result<(), String> {
    let key = format!("{}:{}", session_id, tool_call_id);
    let mut map = state.confirmations.lock()
        .map_err(|e| format!("Confirmations lock: {}", e))?;
    if let Some(tx) = map.remove(&key) {
        let _ = tx.send(approved);
    }
    Ok(())
}
```

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p robit-gui`
Expected: Compilation should succeed now (all modules present).

- [ ] **Step 7: Commit**

```bash
git add crates/robit-gui/src/commands.rs crates/robit-gui/src/frontend.rs crates/robit-gui/src/state.rs crates/robit-gui/src/main.rs
git commit -m "feat(robit-gui): add Tauri commands and wire GuiFrontend confirmations via AppState"
```

---

### Task 6: React Frontend — Project Setup & Dependencies

**Files:**
- Create: `crates/robit-gui/ui/package.json`
- Create: `crates/robit-gui/ui/tsconfig.json`
- Create: `crates/robit-gui/ui/vite.config.ts`
- Create: `crates/robit-gui/ui/index.html`
- Create: `crates/robit-gui/ui/src/styles/globals.css`
- Create: `crates/robit-gui/ui/tailwind.config.ts` (Tailwind v4 might use CSS-based config)
- Create: `crates/robit-gui/ui/postcss.config.js`

- [ ] **Step 1: Create crates/robit-gui/ui/package.json**

```json
{
  "name": "robit-gui-ui",
  "private": true,
  "version": "0.1.0",
  "type": "module",
  "scripts": {
    "dev": "vite --port 1420",
    "build": "tsc && vite build",
    "preview": "vite preview"
  },
  "dependencies": {
    "react": "^19.0.0",
    "react-dom": "^19.0.0",
    "@tauri-apps/api": "^2.0.0",
    "@radix-ui/react-dialog": "^1.1.0",
    "@radix-ui/react-dropdown-menu": "^2.1.0",
    "@radix-ui/react-scroll-area": "^1.2.0",
    "@radix-ui/react-alert-dialog": "^1.1.0",
    "@radix-ui/react-tooltip": "^1.1.0",
    "@radix-ui/react-slot": "^1.1.0",
    "zustand": "^5.0.0",
    "next-themes": "^0.4.0",
    "lucide-react": "^0.400.0",
    "react-markdown": "^9.0.0",
    "react-syntax-highlighter": "^15.6.0",
    "class-variance-authority": "^0.7.0",
    "clsx": "^2.1.0",
    "tailwind-merge": "^2.5.0"
  },
  "devDependencies": {
    "typescript": "^5.6.0",
    "@types/react": "^19.0.0",
    "@types/react-dom": "^19.0.0",
    "@types/react-syntax-highlighter": "^15.5.0",
    "vite": "^6.0.0",
    "@vitejs/plugin-react": "^4.3.0",
    "tailwindcss": "^4.0.0",
    "@tailwindcss/vite": "^4.0.0",
    "autoprefixer": "^10.4.0"
  }
}
```

- [ ] **Step 2: Create crates/robit-gui/ui/vite.config.ts**

```typescript
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
```

- [ ] **Step 3: Create crates/robit-gui/ui/tsconfig.json**

```json
{
  "compilerOptions": {
    "target": "ES2020",
    "useDefineForClassFields": true,
    "lib": ["ES2020", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "skipLibCheck": true,
    "moduleResolution": "bundler",
    "allowImportingTsExtensions": true,
    "isolatedModules": true,
    "moduleDetection": "force",
    "noEmit": true,
    "jsx": "react-jsx",
    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "noFallthroughCasesInSwitch": true,
    "forceConsistentCasingInFileNames": true,
    "paths": {
      "@/*": ["./src/*"]
    },
    "baseUrl": "."
  },
  "include": ["src"]
}
```

- [ ] **Step 4: Create crates/robit-gui/ui/index.html**

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Robit</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```

- [ ] **Step 5: Create crates/robit-gui/ui/src/styles/globals.css**

```css
@import "tailwindcss";

@custom-variant dark (&:is(.dark *));

@theme inline {
  --color-background: var(--background);
  --color-foreground: var(--foreground);
  --color-card: var(--card);
  --color-card-foreground: var(--card-foreground);
  --color-primary: var(--primary);
  --color-primary-foreground: var(--primary-foreground);
  --color-secondary: var(--secondary);
  --color-secondary-foreground: var(--secondary-foreground);
  --color-muted: var(--muted);
  --color-muted-foreground: var(--muted-foreground);
  --color-accent: var(--accent);
  --color-accent-foreground: var(--accent-foreground);
  --color-destructive: var(--destructive);
  --color-destructive-foreground: var(--destructive-foreground);
  --color-border: var(--border);
  --color-input: var(--input);
  --color-ring: var(--ring);
  --color-sidebar: var(--sidebar);
  --color-sidebar-foreground: var(--sidebar-foreground);
  --color-sidebar-accent: var(--sidebar-accent);
  --color-sidebar-accent-foreground: var(--sidebar-accent-foreground);
  --radius-sm: calc(var(--radius) - 4px);
  --radius-md: calc(var(--radius) - 2px);
  --radius-lg: var(--radius);
  --radius-xl: calc(var(--radius) + 4px);
}

:root {
  --background: oklch(1 0 0);
  --foreground: oklch(0.145 0 0);
  --card: oklch(1 0 0);
  --card-foreground: oklch(0.145 0 0);
  --primary: oklch(0.205 0 0);
  --primary-foreground: oklch(0.985 0 0);
  --secondary: oklch(0.97 0 0);
  --secondary-foreground: oklch(0.205 0 0);
  --muted: oklch(0.97 0 0);
  --muted-foreground: oklch(0.556 0 0);
  --accent: oklch(0.97 0 0);
  --accent-foreground: oklch(0.205 0 0);
  --destructive: oklch(0.577 0.245 27.325);
  --destructive-foreground: oklch(0.985 0 0);
  --border: oklch(0.922 0 0);
  --input: oklch(0.922 0 0);
  --ring: oklch(0.708 0 0);
  --sidebar: oklch(0.985 0 0);
  --sidebar-foreground: oklch(0.145 0 0);
  --sidebar-accent: oklch(0.97 0 0);
  --sidebar-accent-foreground: oklch(0.205 0 0);
  --radius: 0.625rem;
}

.dark {
  --background: oklch(0.145 0 0);
  --foreground: oklch(0.985 0 0);
  --card: oklch(0.145 0 0);
  --card-foreground: oklch(0.985 0 0);
  --primary: oklch(0.985 0 0);
  --primary-foreground: oklch(0.205 0 0);
  --secondary: oklch(0.269 0 0);
  --secondary-foreground: oklch(0.985 0 0);
  --muted: oklch(0.269 0 0);
  --muted-foreground: oklch(0.708 0 0);
  --accent: oklch(0.269 0 0);
  --accent-foreground: oklch(0.985 0 0);
  --destructive: oklch(0.396 0.141 25.723);
  --destructive-foreground: oklch(0.985 0 0);
  --border: oklch(0.269 0 0);
  --input: oklch(0.269 0 0);
  --ring: oklch(0.439 0 0);
  --sidebar: oklch(0.205 0 0);
  --sidebar-foreground: oklch(0.985 0 0);
  --sidebar-accent: oklch(0.269 0 0);
  --sidebar-accent-foreground: oklch(0.985 0 0);
}

* {
  border-color: var(--border);
}

body {
  background-color: var(--background);
  color: var(--foreground);
  font-family: system-ui, -apple-system, sans-serif;
}
```

- [ ] **Step 6: Install dependencies**

Run: `cd crates/robit-gui/ui && npm install`

- [ ] **Step 7: Verify frontend builds**

Run: `cd crates/robit-gui/ui && npx tsc --noEmit`
Expected: Errors about missing `main.tsx` and components (expected at this stage).

- [ ] **Step 8: Commit**

```bash
git add crates/robit-gui/ui/package.json crates/robit-gui/ui/tsconfig.json crates/robit-gui/ui/vite.config.ts crates/robit-gui/ui/index.html crates/robit-gui/ui/src/styles/globals.css crates/robit-gui/ui/package-lock.json
git commit -m "feat(robit-gui): scaffold React frontend with Vite, Tailwind v4, and dependencies"
```

---

### Task 7: React — Type Definitions, Store, and Tauri Bridge

**Files:**
- Create: `crates/robit-gui/ui/src/lib/types.ts`
- Create: `crates/robit-gui/ui/src/lib/store.ts`
- Create: `crates/robit-gui/ui/src/lib/commands.ts`

- [ ] **Step 1: Create crates/robit-gui/ui/src/lib/types.ts**

```typescript
export interface SessionInfo {
  id: string;
  title: string;
  model: string;
  status: "idle" | "ready" | "running";
  created_at: string;
  updated_at: string;
}

export interface MessageData {
  id: number;
  role: "user" | "assistant" | "tool" | "system";
  content: string;
  tool_name?: string;
  tool_call_id?: string;
  created_at: string;
}

export interface ConfigInfo {
  model: string;
  version: string;
  tools_enabled: number;
  tools_total: number;
  auto_approve: boolean;
}

export type UiEvent =
  | { type: "TextDelta"; session_id: string; delta: string }
  | {
      type: "ToolCallRequested";
      session_id: string;
      tool_call_id: string;
      name: string;
      arguments: string;
      requires_confirm: boolean;
    }
  | {
      type: "ToolCallResult";
      session_id: string;
      tool_call_id: string;
      content: string;
      is_error: boolean;
    }
  | { type: "TurnComplete"; session_id: string }
  | { type: "Error"; session_id: string; message: string }
  | {
      type: "SkillTriggered";
      session_id: string;
      name: string;
      description: string;
    };

export interface ToolCallInfo {
  tool_call_id: string;
  name: string;
  arguments: string;
  status: "running" | "success" | "error" | "awaiting_confirmation";
  output?: string;
  requires_confirm: boolean;
}
```

- [ ] **Step 2: Create crates/robit-gui/ui/src/lib/commands.ts**

```typescript
import { invoke } from "@tauri-apps/api/core";
import type { SessionInfo, MessageData, ConfigInfo } from "./types";

export async function createSession(model: string): Promise<SessionInfo> {
  return invoke("create_session", { model });
}

export async function listSessions(): Promise<SessionInfo[]> {
  return invoke("list_sessions");
}

export async function switchSession(
  sessionId: string
): Promise<MessageData[]> {
  return invoke("switch_session", { sessionId });
}

export async function sendMessage(
  sessionId: string,
  content: string
): Promise<void> {
  return invoke("send_message", { sessionId, content });
}

export async function cancel(sessionId: string): Promise<void> {
  return invoke("cancel", { sessionId });
}

export async function deleteSession(sessionId: string): Promise<void> {
  return invoke("delete_session", { sessionId });
}

export async function renameSession(
  sessionId: string,
  title: string
): Promise<void> {
  return invoke("rename_session", { sessionId, title });
}

export async function getMessages(
  sessionId: string
): Promise<MessageData[]> {
  return invoke("get_messages", { sessionId });
}

export async function confirmTool(
  sessionId: string,
  toolCallId: string,
  approved: boolean
): Promise<void> {
  return invoke("confirm_tool", { sessionId, toolCallId, approved });
}

export async function getConfig(): Promise<ConfigInfo> {
  return invoke("get_config");
}
```

- [ ] **Step 3: Create crates/robit-gui/ui/src/lib/store.ts**

```typescript
import { create } from "zustand";
import type {
  SessionInfo,
  MessageData,
  ConfigInfo,
  ToolCallInfo,
} from "./types";

interface AppStore {
  // Session list
  sessions: SessionInfo[];
  activeSessionId: string | null;

  // Messages grouped by session
  messages: Record<string, MessageData[]>;

  // Streaming text buffer per session
  streamingBuffer: Record<string, string>;

  // Agent status per session
  agentStatus: Record<string, "idle" | "ready" | "running">;

  // Pending tool confirmations
  pendingConfirms: Record<string, ToolCallInfo>;

  // Config
  config: ConfigInfo | null;

  // Sidebar width
  sidebarWidth: number;

  // Actions
  setSessions: (sessions: SessionInfo[]) => void;
  setActiveSession: (id: string | null) => void;
  setMessages: (sessionId: string, messages: MessageData[]) => void;
  appendStreaming: (sessionId: string, delta: string) => void;
  commitStreaming: (sessionId: string) => void;
  clearStreaming: (sessionId: string) => void;
  setAgentStatus: (
    sessionId: string,
    status: "idle" | "ready" | "running"
  ) => void;
  addToolCard: (sessionId: string, info: ToolCallInfo) => void;
  updateToolCard: (
    sessionId: string,
    toolCallId: string,
    updates: Partial<ToolCallInfo>
  ) => void;
  removeToolCard: (sessionId: string, toolCallId: string) => void;
  setConfig: (config: ConfigInfo) => void;
  setSidebarWidth: (width: number) => void;
  addSession: (session: SessionInfo) => void;
  removeSession: (sessionId: string) => void;
  updateSessionTitle: (sessionId: string, title: string) => void;
}

export const useStore = create<AppStore>((set, get) => ({
  sessions: [],
  activeSessionId: null,
  messages: {},
  streamingBuffer: {},
  agentStatus: {},
  pendingConfirms: {},
  config: null,
  sidebarWidth: Number(localStorage.getItem("sidebarWidth") || 220),

  setSessions: (sessions) => set({ sessions }),

  setActiveSession: (id) => set({ activeSessionId: id }),

  setMessages: (sessionId, messages) =>
    set((state) => ({
      messages: { ...state.messages, [sessionId]: messages },
    })),

  appendStreaming: (sessionId, delta) =>
    set((state) => ({
      streamingBuffer: {
        ...state.streamingBuffer,
        [sessionId]: (state.streamingBuffer[sessionId] || "") + delta,
      },
    })),

  commitStreaming: (sessionId) => {
    const buffer = get().streamingBuffer[sessionId];
    if (!buffer) return;
    const msg: MessageData = {
      id: Date.now(),
      role: "assistant",
      content: buffer,
      created_at: new Date().toISOString(),
    };
    set((state) => ({
      messages: {
        ...state.messages,
        [sessionId]: [...(state.messages[sessionId] || []), msg],
      },
      streamingBuffer: { ...state.streamingBuffer, [sessionId]: "" },
    }));
  },

  clearStreaming: (sessionId) =>
    set((state) => ({
      streamingBuffer: { ...state.streamingBuffer, [sessionId]: "" },
    })),

  setAgentStatus: (sessionId, status) =>
    set((state) => ({
      agentStatus: { ...state.agentStatus, [sessionId]: status },
      sessions: state.sessions.map((s) =>
        s.id === sessionId ? { ...s, status } : s
      ),
    })),

  addToolCard: (sessionId, info) => {
    set((state) => ({
      pendingConfirms: {
        ...state.pendingConfirms,
        [info.tool_call_id]: info,
      },
    }));
  },

  updateToolCard: (sessionId, toolCallId, updates) => {
    set((state) => ({
      pendingConfirms: {
        ...state.pendingConfirms,
        [toolCallId]: {
          ...state.pendingConfirms[toolCallId],
          ...updates,
        },
      },
    }));
  },

  removeToolCard: (sessionId, toolCallId) => {
    set((state) => {
      const next = { ...state.pendingConfirms };
      delete next[toolCallId];
      return { pendingConfirms: next };
    });
  },

  setConfig: (config) => set({ config }),

  setSidebarWidth: (width) => {
    localStorage.setItem("sidebarWidth", String(width));
    set({ sidebarWidth: width });
  },

  addSession: (session) =>
    set((state) => ({
      sessions: [session, ...state.sessions],
      agentStatus: {
        ...state.agentStatus,
        [session.id]: "ready",
      },
    })),

  removeSession: (sessionId) =>
    set((state) => ({
      sessions: state.sessions.filter((s) => s.id !== sessionId),
    })),

  updateSessionTitle: (sessionId, title) =>
    set((state) => ({
      sessions: state.sessions.map((s) =>
        s.id === sessionId ? { ...s, title } : s
      ),
    })),
}));
```

- [ ] **Step 4: Verify TypeScript compilation**

Run: `cd crates/robit-gui/ui && npx tsc --noEmit`
Expected: Should pass (only missing main.tsx and components).

- [ ] **Step 5: Commit**

```bash
git add crates/robit-gui/ui/src/lib/types.ts crates/robit-gui/ui/src/lib/store.ts crates/robit-gui/ui/src/lib/commands.ts
git commit -m "feat(robit-gui): add TypeScript types, Zustand store, and Tauri command wrappers"
```

---

### Task 8: React — shadcn/ui Components and Utility

**Files:**
- Create: `crates/robit-gui/ui/src/lib/utils.ts`
- Create: `crates/robit-gui/ui/src/components/ui/button.tsx`
- Create: `crates/robit-gui/ui/src/components/ui/input.tsx`
- Create: `crates/robit-gui/ui/src/components/ui/scroll-area.tsx`
- Create: `crates/robit-gui/ui/src/components/ui/dropdown-menu.tsx`
- Create: `crates/robit-gui/ui/src/components/ui/alert-dialog.tsx`
- Create: `crates/robit-gui/ui/src/components/ui/tooltip.tsx`
- Create: `crates/robit-gui/ui/src/components/ui/separator.tsx`

- [ ] **Step 1: Create crates/robit-gui/ui/src/lib/utils.ts**

```typescript
import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
```

- [ ] **Step 2: Create crates/robit-gui/ui/src/components/ui/button.tsx**

```tsx
import * as React from "react";
import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

const buttonVariants = cva(
  "inline-flex items-center justify-center whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50",
  {
    variants: {
      variant: {
        default:
          "bg-primary text-primary-foreground shadow hover:bg-primary/90",
        destructive:
          "bg-destructive text-destructive-foreground shadow-sm hover:bg-destructive/90",
        outline:
          "border border-input bg-background shadow-sm hover:bg-accent hover:text-accent-foreground",
        secondary:
          "bg-secondary text-secondary-foreground shadow-sm hover:bg-secondary/80",
        ghost: "hover:bg-accent hover:text-accent-foreground",
        link: "text-primary underline-offset-4 hover:underline",
      },
      size: {
        default: "h-9 px-4 py-2",
        sm: "h-8 rounded-md px-3 text-xs",
        lg: "h-10 rounded-md px-8",
        icon: "h-9 w-9",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  asChild?: boolean;
}

const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, asChild = false, ...props }, ref) => {
    const Comp = asChild ? Slot : "button";
    return (
      <Comp
        className={cn(buttonVariants({ variant, size, className }))}
        ref={ref}
        {...props}
      />
    );
  }
);
Button.displayName = "Button";

export { Button, buttonVariants };
```

- [ ] **Step 3: Create crates/robit-gui/ui/src/components/ui/input.tsx**

```tsx
import * as React from "react";
import { cn } from "@/lib/utils";

const Input = React.forwardRef<
  HTMLInputElement,
  React.InputHTMLAttributes<HTMLInputElement>
>(({ className, type, ...props }, ref) => {
  return (
    <input
      type={type}
      className={cn(
        "flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors file:border-0 file:bg-transparent file:text-sm file:font-medium placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50",
        className
      )}
      ref={ref}
      {...props}
    />
  );
});
Input.displayName = "Input";

export { Input };
```

- [ ] **Step 4: Create crates/robit-gui/ui/src/components/ui/scroll-area.tsx**

```tsx
import * as React from "react";
import * as ScrollAreaPrimitive from "@radix-ui/react-scroll-area";
import { cn } from "@/lib/utils";

const ScrollArea = React.forwardRef<
  React.ElementRef<typeof ScrollAreaPrimitive.Root>,
  React.ComponentPropsWithoutRef<typeof ScrollAreaPrimitive.Root>
>(({ className, children, ...props }, ref) => (
  <ScrollAreaPrimitive.Root
    ref={ref}
    className={cn("relative overflow-hidden", className)}
    {...props}
  >
    <ScrollAreaPrimitive.Viewport className="h-full w-full rounded-[inherit]">
      {children}
    </ScrollAreaPrimitive.Viewport>
    <ScrollBar />
    <ScrollAreaPrimitive.Corner />
  </ScrollAreaPrimitive.Root>
));
ScrollArea.displayName = ScrollAreaPrimitive.Root.displayName;

const ScrollBar = React.forwardRef<
  React.ElementRef<typeof ScrollAreaPrimitive.ScrollAreaScrollbar>,
  React.ComponentPropsWithoutRef<
    typeof ScrollAreaPrimitive.ScrollAreaScrollbar
  >
>(({ className, orientation = "vertical", ...props }, ref) => (
  <ScrollAreaPrimitive.ScrollAreaScrollbar
    ref={ref}
    orientation={orientation}
    className={cn(
      "flex touch-none select-none transition-colors",
      orientation === "vertical" &&
        "h-full w-2.5 border-l border-l-transparent p-[1px]",
      orientation === "horizontal" &&
        "h-2.5 flex-col border-t border-t-transparent p-[1px]",
      className
    )}
    {...props}
  >
    <ScrollAreaPrimitive.ScrollAreaThumb className="relative flex-1 rounded-full bg-border" />
  </ScrollAreaPrimitive.ScrollAreaScrollbar>
));
ScrollBar.displayName = ScrollAreaPrimitive.ScrollAreaScrollbar.displayName;

export { ScrollArea, ScrollBar };
```

- [ ] **Step 5: Create remaining UI components**

Create `dropdown-menu.tsx`, `alert-dialog.tsx`, `tooltip.tsx`, and `separator.tsx` — these are standard shadcn/ui components. Use the official shadcn/ui source code. For brevity in the plan, install them via:

Run: `cd crates/robit-gui/ui && npx shadcn@latest add button input scroll-area dropdown-menu alert-dialog tooltip separator --yes`

(If `shadcn` CLI is not available, copy the components manually from https://ui.shadcn.com/docs/components)

- [ ] **Step 6: Verify TypeScript compilation**

Run: `cd crates/robit-gui/ui && npx tsc --noEmit`
Expected: Should pass.

- [ ] **Step 7: Commit**

```bash
git add crates/robit-gui/ui/src/lib/utils.ts crates/robit-gui/ui/src/components/ui/
git commit -m "feat(robit-gui): add shadcn/ui base components and utility functions"
```

---

### Task 9: React — Core Components (StatusBar, ThemeToggle, InputArea)

**Files:**
- Create: `crates/robit-gui/ui/src/components/ThemeToggle.tsx`
- Create: `crates/robit-gui/ui/src/components/StatusBar.tsx`
- Create: `crates/robit-gui/ui/src/components/InputArea.tsx`

- [ ] **Step 1: Create crates/robit-gui/ui/src/components/ThemeToggle.tsx**

```tsx
import { useTheme } from "next-themes";
import { Moon, Sun } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useEffect, useState } from "react";

export function ThemeToggle() {
  const { theme, setTheme } = useTheme();
  const [mounted, setMounted] = useState(false);

  useEffect(() => setMounted(true), []);

  if (!mounted) return <div className="h-9 w-9" />;

  return (
    <Button
      variant="ghost"
      size="icon"
      onClick={() => setTheme(theme === "dark" ? "light" : "dark")}
      aria-label="Toggle theme"
    >
      {theme === "dark" ? (
        <Sun className="h-4 w-4" />
      ) : (
        <Moon className="h-4 w-4" />
      )}
    </Button>
  );
}
```

- [ ] **Step 2: Create crates/robit-gui/ui/src/components/StatusBar.tsx**

```tsx
import { useStore } from "@/lib/store";
import { ThemeToggle } from "./ThemeToggle";
import { Bot } from "lucide-react";

export function StatusBar() {
  const config = useStore((s) => s.config);

  return (
    <div className="flex items-center justify-between h-9 px-3 bg-secondary border-b text-xs text-muted-foreground select-none shrink-0">
      <div className="flex items-center gap-3">
        <div className="flex items-center gap-1.5">
          <Bot className="h-3.5 w-3.5" />
          <span className="font-medium text-foreground">
            robit v{config?.version || "0.1.0"}
          </span>
        </div>
        <span className="text-border">│</span>
        <span>{config?.model || "Loading..."}</span>
        <span className="text-border">│</span>
        <span>
          工具: {config?.tools_enabled || 0}/{config?.tools_total || 0}
        </span>
      </div>
      <ThemeToggle />
    </div>
  );
}
```

- [ ] **Step 3: Create crates/robit-gui/ui/src/components/InputArea.tsx**

```tsx
import { useState, useRef, useCallback, KeyboardEvent } from "react";
import { Send } from "lucide-react";
import { Button } from "@/components/ui/button";

interface InputAreaProps {
  onSend: (content: string) => void;
  disabled?: boolean;
}

export function InputArea({ onSend, disabled }: InputAreaProps) {
  const [value, setValue] = useState("");
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const handleSend = useCallback(() => {
    const trimmed = value.trim();
    if (!trimmed || disabled) return;
    onSend(trimmed);
    setValue("");
    // Reset textarea height
    if (textareaRef.current) {
      textareaRef.current.style.height = "auto";
    }
  }, [value, disabled, onSend]);

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
    if (e.key === "Escape") {
      // Cancel signal could be sent here
    }
  };

  const handleInput = () => {
    const el = textareaRef.current;
    if (el) {
      el.style.height = "auto";
      el.style.height = Math.min(el.scrollHeight, 200) + "px";
    }
  };

  return (
    <div className="border-t p-3 shrink-0">
      <div className="flex items-end gap-2">
        <textarea
          ref={textareaRef}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={handleKeyDown}
          onInput={handleInput}
          placeholder="输入消息... (Enter 发送, Shift+Enter 换行, Esc 取消)"
          disabled={disabled}
          rows={1}
          className="flex-1 resize-none rounded-md border border-input bg-background px-3 py-2 text-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50"
        />
        <Button
          size="icon"
          onClick={handleSend}
          disabled={disabled || !value.trim()}
          className="shrink-0"
        >
          <Send className="h-4 w-4" />
        </Button>
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/robit-gui/ui/src/components/ThemeToggle.tsx crates/robit-gui/ui/src/components/StatusBar.tsx crates/robit-gui/ui/src/components/InputArea.tsx
git commit -m "feat(robit-gui): add StatusBar, ThemeToggle, and InputArea components"
```

---

### Task 10: React — Message Components

**Files:**
- Create: `crates/robit-gui/ui/src/components/UserMessage.tsx`
- Create: `crates/robit-gui/ui/src/components/AssistantMessage.tsx`
- Create: `crates/robit-gui/ui/src/components/ToolCard.tsx`
- Create: `crates/robit-gui/ui/src/components/MessageList.tsx`

- [ ] **Step 1: Create crates/robit-gui/ui/src/components/UserMessage.tsx**

```tsx
import { User } from "lucide-react";

interface UserMessageProps {
  content: string;
}

export function UserMessage({ content }: UserMessageProps) {
  return (
    <div className="flex gap-3 px-4 py-3">
      <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-primary/10">
        <User className="h-4 w-4 text-primary" />
      </div>
      <div className="flex-1 pt-1">
        <div className="text-sm font-medium text-muted-foreground mb-1">
          You
        </div>
        <div className="text-sm whitespace-pre-wrap">{content}</div>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Create crates/robit-gui/ui/src/components/AssistantMessage.tsx**

```tsx
import { Bot } from "lucide-react";
import ReactMarkdown from "react-markdown";
import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
import { oneDark } from "react-syntax-highlighter/dist/esm/styles/prism";

interface AssistantMessageProps {
  content: string;
  isStreaming?: boolean;
}

export function AssistantMessage({
  content,
  isStreaming,
}: AssistantMessageProps) {
  return (
    <div className="flex gap-3 px-4 py-3">
      <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-accent">
        <Bot className="h-4 w-4 text-accent-foreground" />
      </div>
      <div className="flex-1 pt-1 min-w-0">
        <div className="text-sm font-medium text-muted-foreground mb-1">
          Robit
        </div>
        <div className="prose prose-sm dark:prose-invert max-w-none text-sm">
          <ReactMarkdown
            components={{
              code({ className, children, ...props }) {
                const match = /language-(\w+)/.exec(className || "");
                const codeStr = String(children).replace(/\n$/, "");
                if (match) {
                  return (
                    <SyntaxHighlighter
                      style={oneDark}
                      language={match[1]}
                      PreTag="div"
                    >
                      {codeStr}
                    </SyntaxHighlighter>
                  );
                }
                return (
                  <code className={className} {...props}>
                    {children}
                  </code>
                );
              },
            }}
          >
            {content || (isStreaming ? "▊" : "")}
          </ReactMarkdown>
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Create crates/robit-gui/ui/src/components/ToolCard.tsx**

```tsx
import { Wrench, Check, X, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { ToolCallInfo } from "@/lib/types";
import { useStore } from "@/lib/store";

interface ToolCardProps {
  info: ToolCallInfo;
}

export function ToolCard({ info }: ToolCardProps) {
  const activeSessionId = useStore((s) => s.activeSessionId);

  const handleConfirm = (approved: boolean) => {
    if (!activeSessionId) return;
    import("@/lib/commands").then(({ confirmTool }) => {
      confirmTool(activeSessionId, info.tool_call_id, approved);
    });
  };

  const statusIcon = () => {
    switch (info.status) {
      case "running":
        return <Loader2 className="h-4 w-4 animate-spin text-blue-500" />;
      case "success":
        return <Check className="h-4 w-4 text-green-500" />;
      case "error":
        return <X className="h-4 w-4 text-red-500" />;
      case "awaiting_confirmation":
        return <Wrench className="h-4 w-4 text-yellow-500" />;
    }
  };

  const statusText = () => {
    switch (info.status) {
      case "running":
        return "执行中...";
      case "success":
        return "完成";
      case "error":
        return "失败";
      case "awaiting_confirmation":
        return "等待确认";
    }
  };

  return (
    <div className="mx-4 my-2 border rounded-lg overflow-hidden bg-card">
      <div className="flex items-center gap-2 px-3 py-2 bg-secondary/50 border-b text-xs">
        {statusIcon()}
        <span className="font-medium">🔧 {info.name}</span>
        <span className="text-muted-foreground">{statusText()}</span>
      </div>
      <div className="px-3 py-2">
        <div className="text-xs text-muted-foreground mb-1">参数:</div>
        <pre className="text-xs bg-muted p-2 rounded overflow-x-auto whitespace-pre-wrap">
          {info.arguments}
        </pre>
        {info.output && (
          <>
            <div className="text-xs text-muted-foreground mt-2 mb-1">
              输出:
            </div>
            <pre className="text-xs bg-muted p-2 rounded overflow-x-auto max-h-40 overflow-y-auto whitespace-pre-wrap">
              {info.output}
            </pre>
          </>
        )}
      </div>
      {info.status === "awaiting_confirmation" && info.requires_confirm && (
        <div className="flex gap-2 px-3 py-2 border-t bg-secondary/30">
          <Button
            size="sm"
            variant="default"
            className="bg-green-600 hover:bg-green-700"
            onClick={() => handleConfirm(true)}
          >
            <Check className="h-3 w-3 mr-1" />
            允许
          </Button>
          <Button
            size="sm"
            variant="destructive"
            onClick={() => handleConfirm(false)}
          >
            <X className="h-3 w-3 mr-1" />
            拒绝
          </Button>
        </div>
      )}
    </div>
  );
}
```

- [ ] **Step 4: Create crates/robit-gui/ui/src/components/MessageList.tsx**

```tsx
import { useEffect, useRef } from "react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useStore } from "@/lib/store";
import { UserMessage } from "./UserMessage";
import { AssistantMessage } from "./AssistantMessage";
import { ToolCard } from "./ToolCard";

export function MessageList() {
  const activeSessionId = useStore((s) => s.activeSessionId);
  const messages = useStore((s) =>
    activeSessionId ? s.messages[activeSessionId] || [] : []
  );
  const streamingBuffer = useStore((s) =>
    activeSessionId ? s.streamingBuffer[activeSessionId] || "" : ""
  );
  const pendingConfirms = useStore((s) => s.pendingConfirms);
  const viewportRef = useRef<HTMLDivElement>(null);

  // Auto-scroll to bottom on new content
  useEffect(() => {
    const viewport = viewportRef.current;
    if (viewport) {
      viewport.scrollTop = viewport.scrollHeight;
    }
  }, [messages, streamingBuffer, pendingConfirms]);

  const toolCards = Object.values(pendingConfirms);

  return (
    <ScrollArea className="flex-1">
      <div ref={viewportRef} className="py-2">
        {messages.map((msg) => {
          if (msg.role === "user") {
            return <UserMessage key={msg.id} content={msg.content} />;
          }
          if (msg.role === "assistant") {
            return <AssistantMessage key={msg.id} content={msg.content} />;
          }
          // Tool messages are rendered as ToolCards via pendingConfirms
          return null;
        })}

        {/* Streaming text (in-progress assistant response) */}
        {streamingBuffer && (
          <AssistantMessage content={streamingBuffer} isStreaming />
        )}

        {/* Tool cards for current turn */}
        {toolCards.map((info) => (
          <ToolCard key={info.tool_call_id} info={info} />
        ))}
      </div>
    </ScrollArea>
  );
}
```

- [ ] **Step 5: Verify TypeScript compilation**

Run: `cd crates/robit-gui/ui && npx tsc --noEmit`
Expected: Should pass.

- [ ] **Step 6: Commit**

```bash
git add crates/robit-gui/ui/src/components/UserMessage.tsx crates/robit-gui/ui/src/components/AssistantMessage.tsx crates/robit-gui/ui/src/components/ToolCard.tsx crates/robit-gui/ui/src/components/MessageList.tsx
git commit -m "feat(robit-gui): add message components with Markdown rendering and tool cards"
```

---

### Task 11: React — SessionSidebar and ChatPanel

**Files:**
- Create: `crates/robit-gui/ui/src/components/SessionItem.tsx`
- Create: `crates/robit-gui/ui/src/components/SessionSidebar.tsx`
- Create: `crates/robit-gui/ui/src/components/ChatPanel.tsx`

- [ ] **Step 1: Create crates/robit-gui/ui/src/components/SessionItem.tsx**

```tsx
import { useState } from "react";
import { MessageSquare, MoreHorizontal, Pencil, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Input } from "@/components/ui/input";
import type { SessionInfo } from "@/lib/types";
import { useStore } from "@/lib/store";
import {
  switchSession,
  deleteSession,
  renameSession,
} from "@/lib/commands";

interface SessionItemProps {
  session: SessionInfo;
}

export function SessionItem({ session }: SessionItemProps) {
  const activeSessionId = useStore((s) => s.activeSessionId);
  const setActiveSession = useStore((s) => s.setActiveSession);
  const setMessages = useStore((s) => s.setMessages);
  const removeSession = useStore((s) => s.removeSession);
  const updateSessionTitle = useStore((s) => s.updateSessionTitle);

  const [isEditing, setIsEditing] = useState(false);
  const [editTitle, setEditTitle] = useState(session.title);
  const [showDeleteDialog, setShowDeleteDialog] = useState(false);

  const isActive = session.id === activeSessionId;

  const handleClick = async () => {
    if (isActive) return;
    try {
      const msgs = await switchSession(session.id);
      setActiveSession(session.id);
      setMessages(session.id, msgs);
    } catch (e) {
      console.error("Failed to switch session:", e);
    }
  };

  const handleRename = async () => {
    if (!editTitle.trim()) return;
    try {
      await renameSession(session.id, editTitle.trim());
      updateSessionTitle(session.id, editTitle.trim());
    } catch (e) {
      console.error("Failed to rename session:", e);
    }
    setIsEditing(false);
  };

  const handleDelete = async () => {
    try {
      await deleteSession(session.id);
      removeSession(session.id);
    } catch (e) {
      console.error("Failed to delete session:", e);
    }
    setShowDeleteDialog(false);
  };

  return (
    <>
      <div
        onClick={handleClick}
        className={`
          group flex items-center gap-2 px-2 py-1.5 rounded-md cursor-pointer text-sm transition-colors
          ${
            isActive
              ? "bg-accent text-accent-foreground"
              : "hover:bg-accent/50 text-foreground"
          }
        `}
      >
        <MessageSquare className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        {isEditing ? (
          <Input
            value={editTitle}
            onChange={(e) => setEditTitle(e.target.value)}
            onBlur={handleRename}
            onKeyDown={(e) => {
              if (e.key === "Enter") handleRename();
              if (e.key === "Escape") setIsEditing(false);
            }}
            className="h-6 text-xs"
            autoFocus
            onClick={(e) => e.stopPropagation()}
          />
        ) : (
          <span className="flex-1 truncate">{session.title}</span>
        )}
        {session.status === "running" && (
          <span className="h-2 w-2 rounded-full bg-green-500 animate-pulse shrink-0" />
        )}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon"
              className="h-5 w-5 opacity-0 group-hover:opacity-100 shrink-0"
              onClick={(e) => e.stopPropagation()}
            >
              <MoreHorizontal className="h-3 w-3" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-36">
            <DropdownMenuItem
              onClick={(e) => {
                e.stopPropagation();
                setIsEditing(true);
              }}
            >
              <Pencil className="h-3.5 w-3.5 mr-2" />
              重命名
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={(e) => {
                e.stopPropagation();
                setShowDeleteDialog(true);
              }}
              className="text-destructive"
            >
              <Trash2 className="h-3.5 w-3.5 mr-2" />
              删除
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      <AlertDialog open={showDeleteDialog} onOpenChange={setShowDeleteDialog}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>删除会话</AlertDialogTitle>
            <AlertDialogDescription>
              确定要删除会话 "{session.title}" 吗？此操作可以撤销（软删除）。
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction onClick={handleDelete}>删除</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
```

- [ ] **Step 2: Create crates/robit-gui/ui/src/components/SessionSidebar.tsx**

```tsx
import { useCallback, useRef, useEffect } from "react";
import { Plus } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { SessionItem } from "./SessionItem";
import { useStore } from "@/lib/store";
import { createSession } from "@/lib/commands";

export function SessionSidebar() {
  const sessions = useStore((s) => s.sessions);
  const config = useStore((s) => s.config);
  const activeSessionId = useStore((s) => s.activeSessionId);
  const sidebarWidth = useStore((s) => s.sidebarWidth);
  const setSidebarWidth = useStore((s) => s.setSidebarWidth);
  const setActiveSession = useStore((s) => s.setActiveSession);
  const addSession = useStore((s) => s.addSession);

  const isResizing = useRef(false);
  const sidebarRef = useRef<HTMLDivElement>(null);

  const handleCreateSession = async () => {
    try {
      const model = config?.model || "deepseek/deepseek-chat";
      const session = await createSession(model);
      addSession(session);
      setActiveSession(session.id);
    } catch (e) {
      console.error("Failed to create session:", e);
    }
  };

  const startResize = useCallback(() => {
    isResizing.current = true;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
  }, []);

  useEffect(() => {
    const onMouseMove = (e: MouseEvent) => {
      if (!isResizing.current) return;
      const newWidth = Math.min(400, Math.max(160, e.clientX));
      setSidebarWidth(newWidth);
    };
    const onMouseUp = () => {
      isResizing.current = false;
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseup", onMouseUp);
    return () => {
      document.removeEventListener("mousemove", onMouseMove);
      document.removeEventListener("mouseup", onMouseUp);
    };
  }, [setSidebarWidth]);

  return (
    <div ref={sidebarRef} className="flex shrink-0" style={{ width: sidebarWidth }}>
      <div className="flex flex-col flex-1 border-r min-w-0">
        <div className="flex items-center justify-between px-3 py-2 border-b">
          <span className="text-xs font-medium text-muted-foreground">
            会话列表
          </span>
          <Button
            variant="ghost"
            size="icon"
            className="h-6 w-6"
            onClick={handleCreateSession}
          >
            <Plus className="h-3.5 w-3.5" />
          </Button>
        </div>
        <ScrollArea className="flex-1">
          <div className="p-2 space-y-0.5">
            {sessions.map((session) => (
              <SessionItem key={session.id} session={session} />
            ))}
            {sessions.length === 0 && (
              <p className="text-xs text-muted-foreground text-center py-8">
                没有会话，点击 + 创建
              </p>
            )}
          </div>
        </ScrollArea>
      </div>
      {/* Resize handle */}
      <div
        className="w-1 cursor-col-resize hover:bg-accent transition-colors shrink-0"
        onMouseDown={startResize}
      />
    </div>
  );
}
```

- [ ] **Step 3: Create crates/robit-gui/ui/src/components/ChatPanel.tsx**

```tsx
import { useCallback } from "react";
import { MessageList } from "./MessageList";
import { InputArea } from "./InputArea";
import { useStore } from "@/lib/store";
import { sendMessage } from "@/lib/commands";

export function ChatPanel() {
  const activeSessionId = useStore((s) => s.activeSessionId);
  const agentStatus = useStore((s) =>
    activeSessionId ? s.agentStatus[activeSessionId] : "idle"
  );
  const isBusy = agentStatus === "running";

  const handleSend = useCallback(
    async (content: string) => {
      if (!activeSessionId) return;
      try {
        setAgentStatus(activeSessionId, "running");
        await sendMessage(activeSessionId, content);
        // Note: actual message rendering happens via Tauri events
      } catch (e) {
        console.error("Failed to send message:", e);
        setAgentStatus(activeSessionId, "ready");
      }
    },
    [activeSessionId, setAgentStatus]
  );

  if (!activeSessionId) {
    return (
      <div className="flex-1 flex items-center justify-center text-muted-foreground">
        <div className="text-center">
          <p className="text-lg mb-2">Robit AI Programming Agent</p>
          <p className="text-sm">选择一个会话或创建新会话开始</p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex-1 flex flex-col min-w-0">
      <MessageList />
      <InputArea onSend={handleSend} disabled={isBusy} />
    </div>
  );
}
```

- [ ] **Step 4: Verify TypeScript compilation**

Run: `cd crates/robit-gui/ui && npx tsc --noEmit`
Expected: Should pass.

- [ ] **Step 5: Commit**

```bash
git add crates/robit-gui/ui/src/components/SessionItem.tsx crates/robit-gui/ui/src/components/SessionSidebar.tsx crates/robit-gui/ui/src/components/ChatPanel.tsx
git commit -m "feat(robit-gui): add SessionSidebar with resize, SessionItem with context menu, and ChatPanel"
```

---

### Task 12: React — App Root, Event Listener, and Entry Point

**Files:**
- Create: `crates/robit-gui/ui/src/App.tsx`
- Create: `crates/robit-gui/ui/src/main.tsx`

- [ ] **Step 1: Create crates/robit-gui/ui/src/App.tsx**

```tsx
import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { ThemeProvider } from "next-themes";
import { StatusBar } from "@/components/StatusBar";
import { SessionSidebar } from "@/components/SessionSidebar";
import { ChatPanel } from "@/components/ChatPanel";
import { useStore } from "@/lib/store";
import { listSessions, getConfig } from "@/lib/commands";
import type { UiEvent } from "@/lib/types";

export function App() {
  const setSessions = useStore((s) => s.setSessions);
  const setConfig = useStore((s) => s.setConfig);
  const appendStreaming = useStore((s) => s.appendStreaming);
  const commitStreaming = useStore((s) => s.commitStreaming);
  const clearStreaming = useStore((s) => s.clearStreaming);
  const setAgentStatus = useStore((s) => s.setAgentStatus);
  const addToolCard = useStore((s) => s.addToolCard);
  const updateToolCard = useStore((s) => s.updateToolCard);

  // Load initial data
  useEffect(() => {
    (async () => {
      try {
        const [sessions, config] = await Promise.all([
          listSessions(),
          getConfig(),
        ]);
        setSessions(sessions);
        setConfig(config);
      } catch (e) {
        console.error("Failed to load initial data:", e);
      }
    })();
  }, []);

  // Listen for Agent events
  useEffect(() => {
    const unlisten = listen<UiEvent>("agent-event", (event) => {
      const { payload } = event;
      const sid = payload.session_id;

      switch (payload.type) {
        case "TextDelta":
          appendStreaming(sid, payload.delta);
          break;

        case "ToolCallRequested":
          addToolCard(sid, {
            tool_call_id: payload.tool_call_id,
            name: payload.name,
            arguments: payload.arguments,
            status: payload.requires_confirm
              ? "awaiting_confirmation"
              : "running",
            requires_confirm: payload.requires_confirm,
          });
          break;

        case "ToolCallResult":
          updateToolCard(sid, payload.tool_call_id, {
            status: payload.is_error ? "error" : "success",
            output: payload.content,
          });
          break;

        case "TurnComplete":
          commitStreaming(sid);
          setAgentStatus(sid, "ready");
          break;

        case "Error":
          clearStreaming(sid);
          setAgentStatus(sid, "ready");
          break;

        case "SkillTriggered":
          // Skill triggered — could show a notification
          break;
      }
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  return (
    <ThemeProvider attribute="class" defaultTheme="system" enableSystem>
      <div className="h-screen flex flex-col bg-background text-foreground">
        <StatusBar />
        <div className="flex flex-1 overflow-hidden">
          <SessionSidebar />
          <ChatPanel />
        </div>
      </div>
    </ThemeProvider>
  );
}
```

- [ ] **Step 2: Create crates/robit-gui/ui/src/main.tsx**

```tsx
import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import "./styles/globals.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
```

- [ ] **Step 3: Verify TypeScript and Vite build**

Run: `cd crates/robit-gui/ui && npx tsc --noEmit`
Expected: Should pass.

Run: `cd crates/robit-gui/ui && npm run build`
Expected: Successful build to `ui/dist/`.

- [ ] **Step 4: Commit**

```bash
git add crates/robit-gui/ui/src/App.tsx crates/robit-gui/ui/src/main.tsx
git commit -m "feat(robit-gui): add App root with Tauri event listener, ThemeProvider, and entry point"
```

---

### Task 13: Full Integration — Build and Verify

**Files:**
- Modify: `crates/robit-gui/src/main.rs` (finalize if needed)

- [ ] **Step 1: Verify full Rust compilation**

Run: `cargo check -p robit-gui`
Expected: No errors.

- [ ] **Step 2: Verify workspace compilation**

Run: `cargo check --workspace`
Expected: All crates compile, including robit-tui (unchanged).

- [ ] **Step 3: Test Tauri dev mode launches**

Run: `cd crates/robit-gui && cargo tauri dev`
Expected: Window opens with React UI. Verify:
- StatusBar shows model and tool count
- Sidebar shows empty state with "+" button
- Theme toggle switches light/dark
- Creating a new session works (requires valid robit.toml config)

- [ ] **Step 4: Verify robit-tui still works**

Run: `cargo run -p robit-tui`
Expected: TUI launches normally with no regressions.

- [ ] **Step 5: Commit any final adjustments**

```bash
git add -A
git commit -m "feat(robit-gui): finalize integration — full Tauri + React GUI with multi-session support"
```

---

### Task 14: Rust Integration Tests

**Files:**
- Create: `crates/robit-gui/tests/integration.rs`

- [ ] **Step 1: Create crates/robit-gui/tests/integration.rs**

```rust
use std::path::PathBuf;

use robit_gui::db;
use robit_gui::events::{MessageData, SessionInfo};

/// Test DB init creates tables without error.
#[test]
fn test_db_init() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();
}

/// Test session CRUD operations.
#[test]
fn test_session_crud() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();

    // Insert
    db::insert_session(&conn, "test-id", "Test Session", "deepseek/deepseek-chat").unwrap();

    // List
    let sessions = db::list_sessions(&conn).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "test-id");
    assert_eq!(sessions[0].title, "Test Session");

    // Get
    let session = db::get_session(&conn, "test-id").unwrap().unwrap();
    assert_eq!(session.id, "test-id");

    // Update title
    db::update_session_title(&conn, "test-id", "Renamed").unwrap();
    let session = db::get_session(&conn, "test-id").unwrap().unwrap();
    assert_eq!(session.title, "Renamed");

    // Soft delete
    db::delete_session(&conn, "test-id").unwrap();
    let sessions = db::list_sessions(&conn).unwrap();
    assert_eq!(sessions.len(), 0);
}

/// Test message insert and retrieval.
#[test]
fn test_message_crud() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();

    db::insert_session(&conn, "s1", "Session 1", "m").unwrap();

    let id1 = db::insert_message(&conn, "s1", "user", "Hello", None, None).unwrap();
    let id2 = db::insert_message(&conn, "s1", "assistant", "Hi there!", None, None).unwrap();

    assert!(id1 > 0);
    assert!(id2 > id1);

    let messages = db::get_messages(&conn, "s1").unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "Hello");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[1].content, "Hi there!");
}

/// Test UiEvent serialization.
#[test]
fn test_ui_event_serialization() {
    let event = robit_gui::events::UiEvent::TextDelta {
        session_id: "abc".to_string(),
        delta: "hello".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"type\":\"TextDelta\""));
    assert!(json.contains("\"session_id\":\"abc\""));
    assert!(json.contains("\"delta\":\"hello\""));
}
```

- [ ] **Step 2: Run integration tests**

Run: `cargo test -p robit-gui`
Expected: All 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/robit-gui/tests/integration.rs
git commit -m "test(robit-gui): add integration tests for DB CRUD and UiEvent serialization"
```

---

## Completion Checklist

- [ ] `cargo check --workspace` passes (all crates, including robit-tui)
- [ ] `cargo test -p robit-gui` passes (4 integration tests)
- [ ] `cargo test -p robit-tui` passes (existing tests unaffected)
- [ ] `cd crates/robit-gui/ui && npx tsc --noEmit` passes (TypeScript)
- [ ] `cd crates/robit-gui/ui && npm run build` succeeds (Vite production build)
- [ ] `cargo tauri dev` launches window with working UI
- [ ] `cargo run -p robit-tui` launches TUI normally (no regressions)
