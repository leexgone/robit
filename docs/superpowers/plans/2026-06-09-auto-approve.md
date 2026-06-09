# Auto-Approve 工具自动批准功能 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `auto_approve` configuration option to robit, enabling users to skip tool confirmation prompts via config file or `--auto-approve` command-line flag.

**Architecture:** Add `auto_approve: Option<bool>` to `AppConfig` in `robit-ai`, pass it through `Agent::new()`, check it in `run_one_step()` before requesting confirmation. Add `clap` for CLI args.

**Tech Stack:** Rust, `clap`, `serde`, `toml`

---

## File Map

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `Cargo.toml` | Add `clap` to workspace dependencies |
| Modify | `crates/robit-ai/src/config.rs` | Add `auto_approve` to `AppConfig` |
| Modify | `crates/robit-agent/src/agent.rs` | Add `auto_approve` field to `Agent`, modify `new()` and `run_one_step()` |
| Modify | `crates/robit-tui/Cargo.toml` | Add `clap.workspace = true` |
| Modify | `crates/robit-tui/src/main.rs` | Add `clap` CLI args, parse config, pass `auto_approve` to `Agent::new()` |
| Modify | `examples/robit-agent/Cargo.toml` | Add `clap.workspace = true` |
| Modify | `examples/robit-agent/src/main.rs` | Add `clap` CLI args, parse config, pass `auto_approve` to `Agent::new()` |

---

### Task 1: Add `clap` to workspace dependencies

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add `clap` to workspace dependencies**

In `Cargo.toml`, add to `[workspace.dependencies]` after `pulldown-cmark`:

```toml
# CLI
clap = { version = "4.5", features = ["derive"] }
```

- [ ] **Step 2: Verify workspace is valid**

Run: `cargo check --workspace 2>&1 | head -20`
Expected: No errors (existing projects should still build without clap yet).

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "chore: add clap to workspace dependencies"
```

---

### Task 2: Add `auto_approve` to `AppConfig`

**Files:**
- Modify: `crates/robit-ai/src/config.rs`

- [ ] **Step 1: Add `auto_approve` field to `AppConfig`**

In `crates/robit-ai/src/config.rs`, modify the `AppConfig` struct (around line 82-90):

```rust
#[derive(Debug, Deserialize, Default)]
pub struct AppConfig {
    pub log_level: Option<String>,
    pub max_steps: Option<usize>,
    pub enabled_tools: Option<Vec<String>>,
    pub enabled_skills: Option<Vec<String>>,
    pub context: Option<ContextConfig>,
    pub retry: Option<RetryConfig>,
    pub auto_approve: Option<bool>,
}
```

- [ ] **Step 2: Update the test to include `auto_approve`**

In `crates/robit-ai/src/config.rs`, find the `test_parse_enabled_skills` test and add a new test after it:

```rust
#[test]
fn test_parse_auto_approve() {
    let toml_str = r#"
        default_model = "deepseek/deepseek-chat"

        [providers.deepseek]
        base_url = "https://api.deepseek.com"
        api_key = "sk-test"

        [[providers.deepseek.models]]
        id = "deepseek-chat"

        [app]
        auto_approve = true
    "#;

    let config: RobitConfig = toml::from_str(toml_str).unwrap();
    let app = config.app.as_ref().unwrap();
    assert_eq!(app.auto_approve, Some(true));
}

#[test]
fn test_parse_auto_approve_default_false() {
    let toml_str = r#"
        default_model = "deepseek/deepseek-chat"

        [providers.deepseek]
        base_url = "https://api.deepseek.com"
        api_key = "sk-test"

        [[providers.deepseek.models]]
        id = "deepseek-chat"
    "#;

    let config: RobitConfig = toml::from_str(toml_str).unwrap();
    let app = config.app.as_ref().unwrap();
    assert_eq!(app.auto_approve, None);
}
```

- [ ] **Step 3: Run tests to verify**

Run: `cargo test -p robit-ai 2>&1`
Expected: All tests pass, including the 2 new ones.

- [ ] **Step 4: Commit**

```bash
git add crates/robit-ai/src/config.rs
git commit -m "feat(config): add auto_approve option to AppConfig"
```

---

### Task 3: Add `auto_approve` to `Agent`

**Files:**
- Modify: `crates/robit-agent/src/agent.rs`

- [ ] **Step 1: Add `auto_approve` field to `Agent` struct**

In `crates/robit-agent/src/agent.rs`, modify the `Agent` struct (around line 58-66):

```rust
pub struct Agent {
    llm_client: Arc<LlmClient>,
    tools: Arc<ToolRegistry>,
    skills: Arc<SkillRegistry>,
    sessions: HashMap<SessionId, AgentSession>,
    default_session_id: SessionId,
    context_manager: ContextManager,
    frontend: Arc<dyn Frontend>,
    auto_approve: bool,
}
```

- [ ] **Step 2: Modify `Agent::new()` to accept and store `auto_approve`**

Modify the `Agent::new()` function signature and implementation (around line 70-104):

```rust
impl Agent {
    /// Create a new Agent with the given dependencies.
    pub fn new(
        llm_client: Arc<LlmClient>,
        tools: Arc<ToolRegistry>,
        skills: Arc<SkillRegistry>,
        frontend: Arc<dyn Frontend>,
        context_config: Option<&ContextConfig>,
        context_window: Option<u64>,
        working_dir: PathBuf,
        auto_approve: bool,
    ) -> Self {
        let prompt_builder = PromptBuilder::new();
        let context_manager = ContextManager::new(context_window, context_config);

        // Build system prompt with tools AND skills
        let tool_refs: Vec<&dyn crate::tool::Tool> = tools.tools();
        let skill_descs = skills.skill_descriptions();
        let system_prompt = prompt_builder.build_system_prompt(&tool_refs, &skill_descs, &working_dir);

        // Create default session
        let session_id = new_session_id();
        let session = AgentSession::new(session_id.clone(), working_dir, system_prompt);

        let mut sessions = HashMap::new();
        sessions.insert(session_id.clone(), session);

        Self {
            llm_client,
            tools,
            skills,
            sessions,
            default_session_id: session_id,
            context_manager,
            frontend,
            auto_approve,
        }
    }
```

- [ ] **Step 3: Modify the confirmation check in `run_one_step()`**

Find the confirmation check in `run_one_step()` (around line 359-364) and modify to:

```rust
            // Check confirmation
            let approved = if self.tools.requires_confirmation(&tc.function.name) && !self.auto_approve {
                self.frontend.request_tool_confirmation(&tc_info).await?
            } else {
                true
            };
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p robit-agent 2>&1 | head -20`
Expected: No errors (note: binaries that call `Agent::new()` will fail - that's expected, we'll fix them in subsequent tasks).

- [ ] **Step 5: Commit**

```bash
git add crates/robit-agent/src/agent.rs
git commit -m "feat(agent): add auto_approve support to Agent"
```

---

### Task 4: Add `clap` and `auto_approve` to `robit-tui`

**Files:**
- Modify: `crates/robit-tui/Cargo.toml`
- Modify: `crates/robit-tui/src/main.rs`

- [ ] **Step 1: Add `clap` dependency to `robit-tui/Cargo.toml`**

Add to `crates/robit-tui/Cargo.toml`:

```toml
clap.workspace = true
```

- [ ] **Step 2: Add `clap` imports and CLI struct to `main.rs`**

Add at the top of `crates/robit-tui/src/main.rs` after other imports:

```rust
use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "robit")]
#[command(about = "AI Programming Agent with TUI")]
struct Cli {
    /// 自动批准所有工具调用，跳过用户确认
    #[arg(long)]
    auto_approve: bool,
}
```

- [ ] **Step 3: Parse CLI args and determine `auto_approve` value**

Modify the `main()` function:

First, parse the CLI args at the very beginning of `main()`:

```rust
fn main() -> Result<()> {
    // Parse CLI args first
    let cli = Cli::parse();

    // Initialize tracing (logs go to file, not terminal)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("robit_tui=info".parse()?),
        )
        .with_writer(|| {
            // Discard log output in TUI mode (use RUST_LOG to enable file logging)
            io::sink()
        })
        .init();

    let config = load_config()?;

    // Determine auto_approve: CLI flag takes priority, then config, then default false
    let auto_approve = cli.auto_approve || config.app.as_ref().and_then(|a| a.auto_approve).unwrap_or(false);
```

- [ ] **Step 4: Pass `auto_approve` to `Agent::new()`**

Find the `Agent::new()` call (around line 104-112) and add the `auto_approve` argument:

```rust
    let agent = Agent::new(
        client,
        Arc::clone(&tools),
        Arc::clone(&skill_registry),
        frontend,
        context_config,
        context_window,
        working_dir,
        auto_approve,
    );
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p robit-tui 2>&1 | head -20`
Expected: No errors.

- [ ] **Step 6: Commit**

```bash
git add crates/robit-tui/Cargo.toml crates/robit-tui/src/main.rs
git commit -m "feat(robit-tui): add --auto-approve CLI flag and config support"
```

---

### Task 5: Add `clap` and `auto_approve` to `robit-agent-cli`

**Files:**
- Modify: `examples/robit-agent/Cargo.toml`
- Modify: `examples/robit-agent/src/main.rs`

- [ ] **Step 1: Add `clap` dependency to `examples/robit-agent/Cargo.toml`**

Add to `examples/robit-agent/Cargo.toml`:

```toml
clap.workspace = true
```

- [ ] **Step 2: Add `clap` imports and CLI struct to `main.rs`**

Add at the top of `examples/robit-agent/src/main.rs` after other imports:

```rust
use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "robit-agent-cli")]
#[command(about = "AI Programming Agent with stdin/stdout frontend")]
struct Cli {
    /// 自动批准所有工具调用，跳过用户确认
    #[arg(long)]
    auto_approve: bool,
}
```

- [ ] **Step 3: Parse CLI args and determine `auto_approve` value**

Modify the `main()` function:

First, parse the CLI args at the very beginning of `main()`:

```rust
fn main() -> anyhow::Result<()> {
    // Parse CLI args first
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("robit_agent=info".parse()?),
        )
        .init();

    let config = load_config()?;

    // Determine auto_approve: CLI flag takes priority, then config, then default false
    let auto_approve = cli.auto_approve || config.app.as_ref().and_then(|a| a.auto_approve).unwrap_or(false);
```

- [ ] **Step 4: Pass `auto_approve` to `Agent::new()`**

Find the `Agent::new()` call (around line 98-106) and add the `auto_approve` argument:

```rust
    let agent = Agent::new(
        client,
        tools,
        skill_registry,
        frontend,
        context_config,
        context_window,
        working_dir,
        auto_approve,
    );
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p robit-agent-cli 2>&1 | head -20`
Expected: No errors.

- [ ] **Step 6: Commit**

```bash
git add examples/robit-agent/Cargo.toml examples/robit-agent/src/main.rs
git commit -m "feat(robit-agent-cli): add --auto-approve CLI flag and config support"
```

---

### Task 6: Full build verification and test

**Files:** N/A

- [ ] **Step 1: Full workspace build**

Run: `cargo build --workspace 2>&1`
Expected: No errors, no warnings.

- [ ] **Step 2: Verify `--help` works**

Run: `cargo run -p robit-tui -- --help 2>&1`
Expected: Shows help text including `--auto-approve` flag.

Run: `cargo run -p robit-agent-cli -- --help 2>&1`
Expected: Shows help text including `--auto-approve` flag.

- [ ] **Step 3: Quick manual test with `--auto-approve`**

Run: `cargo run -p robit-agent-cli -- --auto-approve`

Type: "请用 bash 工具运行 `echo hello`"

Expected: No confirmation prompt, executes directly.

- [ ] **Step 4: Commit any leftover fixes**

```bash
git add -A
git commit -m "chore: build fixes if any"
```
(if no changes, skip this step)

