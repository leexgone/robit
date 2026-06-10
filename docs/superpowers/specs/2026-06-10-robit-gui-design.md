# robit-gui Design Specification

> **Date:** 2026-06-10
> **Status:** Draft
> **Related:** [[robit-tui]], [[Frontend trait]], [[architecture]]

## 1. Overview

**robit-gui** is a desktop GUI frontend for the Robit AI programming agent framework, built with Tauri v2. It implements the `Frontend` trait from `robit-agent` and provides a rich multi-session chat experience with session persistence, tool confirmation, and a modern Web-based UI.

### 1.1 Goals

- Replace the terminal-based `robit-tui` with a desktop GUI experience
- Support **multi-session management**: create, switch, delete conversations
- **Session persistence** via SQLite: all chat history survives restarts
- **Multiple agents can run in parallel** across sessions (background execution)
- Maintain zero changes to `robit-agent` and `robit-ai` — purely a new Frontend implementation

### 1.2 Non-Goals (MVP)

- File browser or built-in editor (no IDE features)
- Plugin system
- Remote/cloud sync
- E2E testing
- Mobile support (Tauri v2 enables it, but out of scope for MVP)

---

## 2. Architecture

### 2.1 Workspace Structure

```
crates/robit-gui/          ← New crate
├── Cargo.toml
├── tauri.conf.json
├── capabilities/
│   └── default.json        ← Tauri v2 permissions
├── src/
│   ├── main.rs             ← Tauri entry point, builder setup
│   ├── lib.rs              ← Module declarations
│   ├── state.rs            ← AppState manager (core)
│   ├── commands.rs         ← Tauri IPC commands
│   ├── frontend.rs         ← Frontend trait implementation (GuiFrontend)
│   ├── db.rs               ← SQLite layer (rusqlite)
│   └── events.rs           ← UiEvent payload types
├── ui/                     ← React frontend (Vite)
│   ├── package.json
│   ├── vite.config.ts
│   ├── tailwind.config.ts
│   ├── src/
│   │   ├── main.tsx
│   │   ├── App.tsx
│   │   ├── components/
│   │   │   ├── StatusBar.tsx
│   │   │   ├── SessionSidebar.tsx
│   │   │   ├── SessionItem.tsx
│   │   │   ├── ChatPanel.tsx
│   │   │   ├── MessageList.tsx
│   │   │   ├── UserMessage.tsx
│   │   │   ├── AssistantMessage.tsx
│   │   │   ├── ToolCard.tsx
│   │   │   ├── InputArea.tsx
│   │   │   └── ThemeToggle.tsx
│   │   ├── lib/
│   │   │   ├── store.ts      ← Zustand store
│   │   │   ├── commands.ts   ← Tauri invoke wrappers
│   │   │   └── events.ts     ← Tauri event listeners
│   │   └── styles/
│   │       └── globals.css
│   └── index.html
└── icons/                   ← App icons
```

### 2.2 Dependency Graph

```
robit-gui
  ├── depends on: robit-agent (Frontend trait, Agent, tools)
  ├── depends on: robit-ai (LlmClient, via robit-agent)
  ├── depends on: tauri v2 (desktop framework)
  ├── depends on: rusqlite { features = ["bundled"] } (SQLite)
  └── depends on: tauri-plugin-shell v2 (shell commands)
```

### 2.3 Design Approach: Balanced (Option B)

Rust side manages core state: session lifecycle, message persistence, Agent instances. React side handles pure UI rendering and user interaction.

- **Rust owns**: AppState, Agent instances, SQLite, configuration, tool execution
- **React owns**: UI rendering, layout, theme, user input, event display
- **IPC boundary**: Tauri commands (invoke) + Tauri events (emit)

---

## 3. Core Data Structures

### 3.1 AppState (Tauri Managed State)

```rust
pub struct AppState {
    /// SQLite connection (single conn, Mutex protected)
    db: Mutex<Connection>,

    /// Shared LLM client (all sessions reuse)
    llm_client: Arc<LlmClient>,

    /// Shared tool registry (all sessions reuse)
    tool_registry: Arc<ToolRegistry>,

    /// Active Agent instances, keyed by session ID
    agents: Mutex<HashMap<SessionId, AgentHandle>>,

    /// Currently active session (shown in ChatPanel)
    active_session: Mutex<Option<SessionId>>,
}
```

### 3.2 AgentHandle (Per-Session)

```rust
pub struct AgentHandle {
    /// Agent instance
    agent: Agent,

    /// Send messages to the Agent loop
    message_tx: mpsc::Sender<FrontendMessage>,

    /// Current status
    status: AgentStatus,  // Idle | Ready | Running

    /// Cancel token for interrupting a running Agent
    cancel_token: CancellationToken,
}
```

### 3.3 GuiFrontend (Frontend Trait Implementation)

```rust
pub struct GuiFrontend {
    /// Send UiEvents to the Tauri event bridge
    event_tx: mpsc::Sender<UiEvent>,

    /// Pending tool confirmations (oneshot for blocking wait)
    confirmations: Mutex<HashMap<String, oneshot::Sender<bool>>>,

    /// Owning session ID
    session_id: SessionId,
}
```

Implements `Frontend` trait:
- `on_event(event)` → wraps as `UiEvent` with `session_id`, sends via `event_tx`
- `request_tool_confirmation(info)` → creates oneshot channel, emits to frontend, awaits response

### 3.4 UiEvent (Serialized to Frontend)

```rust
#[derive(Serialize, Clone)]
#[serde(tag = "type")]
pub enum UiEvent {
    TextDelta { session_id: String, delta: String },
    ToolCallRequested {
        session_id: String,
        tool_call_id: String,
        name: String,
        arguments: String,
        requires_confirm: bool,
    },
    ToolCallResult {
        session_id: String,
        tool_call_id: String,
        content: String,
        is_error: bool,
    },
    TurnComplete { session_id: String },
    Error { session_id: String, message: String },
    SkillTriggered { session_id: String, name: String, description: String },
}
```

---

## 4. Session Management

### 4.1 State Machine

```
Idle ──create_agent──▶ Ready ──user_input──▶ Running
  ▲                                            │
  └────────── cancel / switch_session ─────────┘
                       (or TurnComplete → Ready)
```

- **Idle**: Session exists in DB but Agent is not loaded (e.g., at app startup, or session not in focus)
- **Ready**: Agent is loaded in memory, waiting for user input
- **Running**: Agent loop is executing (LLM call or tool execution in progress)

### 4.2 Multi-Session Model (Parallel Agents)

Multiple sessions can be Running simultaneously. When the user switches away from a session that is Running, its Agent continues in the background. The sidebar shows a "Running" indicator. When a background session completes, a notification badge appears.

### 4.3 Tauri Commands

| Command | Parameters | Returns | Description |
|---------|-----------|---------|-------------|
| `create_session` | `model: String` | `SessionInfo` | Create session + Agent |
| `list_sessions` | — | `Vec<SessionInfo>` | List all active sessions |
| `switch_session` | `session_id: String` | `Vec<Message>` | Switch active + load history |
| `send_message` | `session_id, content` | `()` | Send user message (async) |
| `cancel` | `session_id` | `()` | Interrupt running Agent |
| `delete_session` | `session_id` | `()` | Soft-delete session |
| `rename_session` | `session_id, title` | `()` | Rename session |
| `get_messages` | `session_id` | `Vec<Message>` | Get session history |
| `confirm_tool` | `session_id, tool_call_id, approved` | `()` | Tool confirmation reply |
| `get_config` | — | `ConfigInfo` | Get current config (model, tools, etc.) |

### 4.4 Lifecycle Scenarios

**App Startup:**
1. Read `robit.toml` → init `LlmClient` + `ToolRegistry`
2. Open SQLite (create tables if missing)
3. `SELECT * FROM sessions WHERE is_active = 1` → sidebar list
4. All sessions start as Idle; user clicks one → `switch_session` → create Agent, load history

**Create Session:**
1. Frontend: `invoke("create_session", { model })`
2. Rust: `INSERT INTO sessions` → get `session_id`
3. Rust: Create `AgentHandle` (Agent + channels + cancel_token)
4. Rust: Spawn Agent background task
5. Rust: `agents.insert(id, handle)`, `active_session = id`
6. Return `SessionInfo` to frontend → switch to new session

**Switch Session:**
1. Frontend: `invoke("switch_session", { session_id })`
2. Rust: If target Agent doesn't exist, create it from DB history
3. Rust: `active_session = target_id`
4. Rust: Load messages from DB for that session
5. Return messages to frontend → render history

**Send Message:**
1. Frontend: `invoke("send_message", { session_id, content })`
2. Rust: Save user message to DB
3. Rust: `agent_handle.message_tx.send(FrontendMessage::UserInput(content))`
4. Agent loop runs → emits AgentEvent → GuiFrontend → UiEvent → Tauri event → React
5. On TurnComplete: save assistant messages to DB

**Cancel:**
1. Frontend: `invoke("cancel", { session_id })` or Esc key
2. Rust: `agent_handle.cancel_token.cancel()`
3. Agent loop detects cancellation → emits `AgentEvent::Error(Cancelled)`
4. Status returns to Ready

**Delete Session:**
1. Frontend: invoke `delete_session`, show AlertDialog for confirmation
2. Rust: Cancel Agent if running → `agents.remove(session_id)`
3. Rust: `UPDATE sessions SET is_active = 0` (soft delete)
4. If deleted session was active → switch to nearest session or empty state

---

## 5. Database Schema (SQLite via rusqlite)

### 5.1 Tables

```sql
CREATE TABLE sessions (
    id          TEXT PRIMARY KEY,     -- UUID v4
    title       TEXT NOT NULL,        -- Auto-generated from first message
    model       TEXT NOT NULL,        -- "deepseek/deepseek-chat"
    created_at  TEXT NOT NULL,        -- ISO 8601
    updated_at  TEXT NOT NULL,        -- ISO 8601
    is_active   INTEGER DEFAULT 1    -- Soft delete flag
);

CREATE TABLE messages (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id   TEXT NOT NULL REFERENCES sessions(id),
    role         TEXT NOT NULL,       -- "user" | "assistant" | "tool" | "system"
    content      TEXT NOT NULL,
    tool_name    TEXT,                -- Tool name (when role = "tool")
    tool_call_id TEXT,                -- Links request and result
    tokens       INTEGER,            -- Estimated token count
    created_at   TEXT NOT NULL        -- ISO 8601
);

CREATE INDEX idx_messages_session ON messages(session_id);
CREATE INDEX idx_messages_created ON messages(session_id, created_at);
```

### 5.2 What is NOT stored

- Running Agent instances (in-memory only)
- API Keys (read from `robit.toml` / `.env`, never exposed to frontend)
- Full file contents from tool results (truncated summaries only)

---

## 6. React Frontend

### 6.1 Tech Stack

| Layer | Choice |
|-------|--------|
| Framework | React 19 + TypeScript |
| Build | Vite |
| Styling | Tailwind CSS v4 |
| Component Library | shadcn/ui (Radix UI primitives) |
| Icons | lucide-react |
| State Management | Zustand v5 |
| Theme | next-themes (light/dark) |
| Markdown | react-markdown + react-syntax-highlighter |
| Tauri Bridge | @tauri-apps/api v2 |

### 6.2 Layout

```
┌──────────────────────────────────────────────────┐
│ ● robit v0.2.0 │ deepseek/deepseek-chat │ tools  │ ← StatusBar
├────────────┬─────────────────────────────────────┤
│ 📋 Sessions│                                     │
│            │  👤 User: Help me fix main.rs       │
│ ▶ Fix bug  │                                     │
│   Refactor │  🤖 Robit: Let me check that file.  │
│ 🟢 Review  │                                     │
│   Learn    │  ┌─ 🔧 read ────────────────────┐   │
│            │  │ src/main.rs             ✓ 0.3s│   │ ← ToolCard
│  + New     │  └──────────────────────────────┘   │
│            │                                     │
│  (resize)  │                                     │
│            ├─────────────────────────────────────┤
│            │ > Type a message...     [Send]      │ ← InputArea
└────────────┴─────────────────────────────────────┘
```

- Sidebar width: default 220px, resizable via drag handle (range: 160px – 400px)
- Sidebar persisted to localStorage

### 6.3 Component Tree

```
App
├── StatusBar          — model, token count, tool count, theme toggle
├── SessionSidebar     — session list (left panel, resizable)
│   ├── SessionItem × N  — title, status indicator, context menu (rename/delete)
│   └── NewSessionButton
└── ChatPanel          — active session chat (right panel)
    ├── MessageList    — scrollable message area
    │   ├── UserMessage
    │   ├── AssistantMessage  (Markdown rendered)
    │   └── ToolCard
    │       ├── ToolCardHeader  (icon + name + status)
    │       ├── ToolCardBody    (args / output)
    │       └── ConfirmButtons  ([Allow] [Deny] — only for write ops)
    └── InputArea
        ├── TextInput   (multi-line, Enter send, Shift+Enter newline)
        └── SendButton
```

### 6.4 State Management (Zustand)

```typescript
interface AppStore {
  // Session list
  sessions: SessionInfo[];
  activeSessionId: string | null;

  // Messages grouped by session
  messages: Record<string, Message[]>;

  // Streaming text buffer (in-progress TextDelta)
  streamingBuffer: Record<string, string>;

  // Agent status per session
  agentStatus: Record<string, 'idle' | 'ready' | 'running'>;

  // Pending tool confirmations
  pendingConfirms: Record<string, ToolCallInfo>;

  // Theme
  theme: 'light' | 'dark' | 'system';
}
```

### 6.5 Key Interactions

| Interaction | Trigger | Behavior |
|-------------|---------|----------|
| Streaming text | `TextDelta` event | Append to `streamingBuffer`, render Markdown live |
| Tool card display | `ToolCallRequested` | Insert card in message list (running state) |
| Tool result | `ToolCallResult` | Update card to success/failure, show output |
| Write op confirm | `requires_confirm: true` | Show [Allow] [Deny] buttons on card, invoke `confirm_tool` |
| Turn complete | `TurnComplete` | Commit streaming buffer to messages, re-enable input, save to DB |
| Background session done | non-active TurnComplete | Show green badge on sidebar session item |
| Cancel | Esc / cancel button | Invoke `cancel`, re-enable input |
| Delete session | Context menu → Delete | AlertDialog confirm → invoke `delete_session` |
| Theme toggle | Button in StatusBar | Toggle light/dark via next-themes |

### 6.6 Multi-Session Event Routing

All sessions share the Tauri global event bus under the `"agent-event"` event name. Each `UiEvent` carries a `session_id`. The React listener dispatches to the correct session's state. Non-active session completions trigger sidebar badges.

---

## 7. Error Handling

| Error Type | Source | Handling |
|------------|--------|----------|
| Agent errors | `robit-agent` (reuse existing) | `UiEvent::Error` → red system message in chat |
| SQLite errors | `rusqlite` | Tauri command returns `Result` → toast notification in UI |
| IPC serialization | `serde_json` | Tauri framework handles, returns 500 |
| Channel disconnect | `tokio::mpsc` | Agent task exits, AgentHandle cleaned up automatically |

---

## 8. Security

1. **CSP**: Tauri v2 default strict CSP; dev mode allows localhost HMR; production build locks CSP
2. **Tool execution**: Reuses `robit-agent` confirmation mechanism; write operations require user click in GUI (overridable via `auto_approve` config)
3. **API Key**: Only read from `robit.toml` and `.env` on Rust side; never exposed to JavaScript frontend
4. **SQLite**: Parameterized queries via rusqlite; database file at `~/.robit/robit.db`

---

## 9. Configuration

Reuses existing `robit.toml` configuration. No new config format needed.

```toml
# Example robit.toml (existing format, unchanged)
default_model = "deepseek/deepseek-chat"

[providers.deepseek]
# ... (existing)

[app]
log_level = "DEBUG"
max_steps = 10
enabled_tools = ["read", "bash", "edit", "write"]
auto_approve = false
```

The `get_config` Tauri command exposes non-sensitive config (model, enabled tools, max_steps) to the frontend for the StatusBar display. API keys are never sent to the frontend.

---

## 10. Build & Distribution

### 10.1 Development

```bash
# Full Tauri dev (Rust + Vite HMR)
cd crates/robit-gui && cargo tauri dev

# Frontend-only dev (mock data, no Rust backend)
cd crates/robit-gui/ui && npm run dev
```

### 10.2 Production Build

```bash
cd crates/robit-gui && cargo tauri build
```

### 10.3 Distribution Formats

| Platform | Format | Notes |
|----------|--------|-------|
| Windows | `.msi` + `.exe` (NSIS) | Standard installer, custom install path |
| macOS | `.dmg` | Drag-to-install, supports Apple notarization |
| Linux | `.AppImage` + `.deb` | AppImage universal; deb for Debian/Ubuntu |

CI via GitHub Actions for multi-platform builds.

---

## 11. Testing Strategy

| Level | Tool | Scope |
|-------|------|-------|
| Rust unit tests | `cargo test` | `db.rs` (CRUD), `state.rs` (session logic), `events.rs` (serialization) |
| Rust integration tests | `cargo test --test *` | Frontend trait impl, channel communication, Tauri commands |
| Frontend component tests | Vitest + React Testing Library | ChatPanel, ToolCard, SessionSidebar |
| E2E tests | Playwright / WebDriver (future) | Full user flows (post-MVP) |

---

## 12. Dependencies Summary

### Rust (`Cargo.toml`)

```toml
[dependencies]
robit-agent = { path = "../robit-agent" }
robit-ai = { path = "../robit-ai" }
tauri = { version = "2", features = [] }
rusqlite = { version = "0.31", features = ["bundled"] }
tokio.workspace = true
tokio-util = "0.7"
serde.workspace = true
serde_json.workspace = true
tracing.workspace = true
uuid.workspace = true
```

### React (`package.json`)

```json
{
  "dependencies": {
    "react": "^19",
    "react-dom": "^19",
    "@tauri-apps/api": "^2",
    "@tauri-apps/plugin-shell": "^2",
    "@radix-ui/react-dialog": "^1",
    "@radix-ui/react-dropdown-menu": "^2",
    "@radix-ui/react-scroll-area": "^1",
    "@radix-ui/react-tabs": "^1",
    "@radix-ui/react-tooltip": "^1",
    "@radix-ui/react-alert-dialog": "^1",
    "@radix-ui/react-slot": "^1",
    "zustand": "^5",
    "next-themes": "^0.4",
    "lucide-react": "^0.400",
    "react-markdown": "^9",
    "react-syntax-highlighter": "^15",
    "tailwindcss": "^4"
  }
}
```

---

## 13. Key Design Decisions Log

| # | Decision | Rationale |
|---|----------|-----------|
| 1 | Tauri v2 | Latest stable, better IPC, future-proof |
| 2 | React + TypeScript | Rich ecosystem, shadcn/ui compatibility |
| 3 | Tailwind CSS v4 + shadcn/ui | Modern aesthetics, source-controllable components |
| 4 | Rust-side SQLite (rusqlite) | Data consistency with Agent lifecycle |
| 5 | Zustand state management | Lightweight, ideal for Tauri app size |
| 6 | Multiple parallel agents | Non-blocking UX when a session runs long tools |
| 7 | Sidebar session list (simple) | Clean, familiar navigation pattern |
| 8 | Inline tool confirmation | Matches existing security model; auto_approve support |
| 9 | next-themes for light/dark | Zero-config with shadcn/ui CSS variables |
| 10 | Resizable sidebar | User preference, persisted to localStorage |
