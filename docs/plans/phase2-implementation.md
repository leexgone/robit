# 阶段 2 实现计划：Agent 运行时（`robit-agent`）

> **目标**：Agent 能够调用工具完成一个简单编程任务。
> **验证**：命令行 Agent 对话（stdin/stdout 前端），Agent 能使用 `read` 和 `bash` 工具完成任务。

## 前置条件

- 阶段 1 已完成：`robit-ai` 提供 `LlmClient`（流式/非流式 chat）、配置加载
- 架构文档（`docs/architecture.md`、`docs/protocol.md`）已有完整的 trait 和类型设计

## Crate 结构

```
crates/robit-agent/
├── Cargo.toml
└── src/
    ├── lib.rs              # 公共 API 导出
    ├── error.rs            # AgentError
    ├── event.rs            # AgentEvent + FrontendMessage + SessionId
    ├── tool/
    │   ├── mod.rs          # Tool trait, ToolResult, ToolContext, ToolRegistry
    │   ├── read.rs         # read 工具
    │   └── bash.rs         # bash 工具
    ├── frontend.rs         # Frontend trait 定义
    ├── prompt.rs           # 系统提示词构建
    ├── context.rs          # 上下文管理（输出截断 + 历史截断）
    └── agent.rs            # Agent + AgentSession 主循环

examples/robit-agent/
├── Cargo.toml
└── src/
    └── main.rs             # stdin/stdout 验证前端
```

## 依赖（`crates/robit-agent/Cargo.toml`）

```toml
[dependencies]
robit-ai = { path = "../robit-ai" }
async-openai = "0.27"
tokio = { version = "1", features = ["full"] }
async-trait = "0.1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tracing = "0.1"
futures = "0.3"
uuid = { version = "1", features = ["v4"] }
```

## 实现步骤（按依赖顺序）

### Step 1: 基础类型 — `event.rs` + `error.rs`

**`error.rs`** — 统一错误类型：

```rust
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(transparent)]
    LlmError(#[from] robit_ai::LlmError),
    #[error("配置错误: {0}")]
    ConfigError(String),
    #[error("工具执行错误: {0}")]
    ToolError(String),
    #[error("上下文溢出: 当前 {current} tokens，上限 {max} tokens")]
    ContextOverflow { current: usize, max: usize },
    #[error("Agent 内部错误: {0}")]
    InternalError(String),
}
```

**`event.rs`** — Agent ↔ Frontend 通信协议（严格按 `protocol.md`）：

```rust
pub type SessionId = String;  // uuid v4

pub enum AgentEvent {
    TextDelta(String),
    ToolCallRequested { tool_call_id: String, name: String, arguments: String },
    ToolCallResult { tool_call_id: String, result: ToolResult },
    TurnComplete,
    Error(AgentError),
}

pub enum FrontendMessage {
    UserInput(String),
    Cancel,
    ConfirmationResponse { tool_call_id: String, approved: bool },
}
```

### Step 2: 工具系统 — `tool/mod.rs`

按 `architecture.md` 定义 `Tool` trait、`ToolResult`、`ToolContext`、`ToolRegistry`：

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    fn requires_confirmation(&self) -> bool;
    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult, AgentError>;
}

pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
}

pub struct ToolContext {
    pub working_dir: PathBuf,
    pub session_id: SessionId,
    pub frontend: Arc<dyn Frontend>,
}

pub struct ToolRegistry { ... }  // register / tool_schemas / execute
```

`ToolRegistry` 关键方法：
- `register(tool)` — 注册工具
- `tool_schemas()` → `Vec<ChatCompletionTool>` — 生成 OpenAI function calling schema
- `execute(name, args, ctx)` → `ToolResult` — 按名称查找执行，不存在时返回 `ToolResult { is_error: true }`

### Step 3: 核心工具 — `tool/read.rs` + `tool/bash.rs`

**`read` 工具**：
- 参数：`file_path`（必填）、`offset`（可选，默认 0）、`limit`（可选，默认全部）
- 行为：读取文件，附加行号输出
- 输出截断：遵守 `max_output_lines` / `max_output_bytes`，截断时附加提示
- `requires_confirmation: false`
- MVP 不处理图片

**`bash` 工具**：
- 参数：`command`（必填）、`timeout`（可选，默认 120000ms）、`working_dir`（可选）
- 行为：通过系统 shell 执行命令（Windows: `cmd /C`，Unix: `sh -c`），捕获 stdout+stderr
- 超时：`tokio::time::timeout` 控制
- 非零退出码 → `ToolResult { is_error: true }`，内容包含退出码和输出
- `requires_confirmation: true`

### Step 4: Frontend trait — `frontend.rs`

**设计调整**：架构文档中 `event_receiver()` 返回 `mpsc::Receiver` 有所有权问题（只能一个消费者）。改为 Agent 创建 channel pair，通过构造函数注入：

```rust
#[async_trait]
pub trait Frontend: Send + Sync {
    /// Agent → Frontend：推送事件（文本、工具调用、错误等）
    async fn on_event(&self, event: AgentEvent) -> Result<(), AgentError>;

    /// Agent → Frontend：请求工具确认（阻塞等待用户响应）
    async fn request_tool_confirmation(&self, info: &ToolCallInfo) -> Result<bool, AgentError>;
}
```

Agent 与 Frontend 的 channel 通信：
```rust
pub struct AgentChannels {
    pub event_tx: mpsc::Sender<AgentEvent>,         // Agent 发送事件给前端
    pub message_rx: mpsc::Receiver<FrontendMessage>, // Agent 接收前端消息
}
```

Agent 创建 channel pair，把 `event_rx` + `message_tx` 交给 Frontend 实现。

### Step 5: 提示词系统 — `prompt.rs`

```rust
pub struct PromptBuilder { ... }

impl PromptBuilder {
    pub fn build_system_prompt(tools: &[&dyn Tool]) -> String;
}
```

系统提示词组成（按 `architecture.md`）：
1. 身份定义（固定）
2. 工具使用说明（根据已注册工具动态生成）
3. 编程规范（固定）
4. 环境信息（OS、cwd、日期，运行时注入）
5. 技能注入（MVP 不实现，留空）

支持 `~/.robit/prompts/system.txt` 自定义覆盖。

### Step 6: 上下文管理 — `context.rs`

**第一层：工具输出截断**
- `truncate_output(content: &str, config: &ContextConfig) -> String`
- 按 `max_output_lines`（默认 500）和 `max_output_bytes`（默认 51200）截断
- 截断时附加提示：`"... (输出已截断，共 N 行，显示前 500 行。请使用 offset/limit 参数分段读取)"`

**第二层：历史消息截断**
- `ContextManager` 持有 `max_tokens` + `reserve_ratio`
- Token 估算：英文 `chars/4`，中文 `chars/2`（混合取 `chars/3`）
- 当估算 token 接近 `max_tokens * (1 - reserve_ratio)` 时，从最早的非系统消息按轮次移除
- 移除后插入摘要提示：`"[已省略 N 轮对话]"`

### Step 7: Agent 主循环 — `agent.rs`

```rust
pub struct AgentSession {
    session_id: SessionId,
    history: Vec<ChatCompletionRequestMessage>,
    context_manager: ContextManager,
}

pub struct Agent {
    llm_client: Arc<LlmClient>,
    tools: Arc<ToolRegistry>,
    sessions: HashMap<SessionId, AgentSession>,
    prompt_builder: PromptBuilder,
}
```

**Agent 主循环逻辑**（一次 turn）：

```
1. 将用户输入追加到 history
2. loop {
   a. context_manager.maybe_truncate(history)
   b. 组装 messages = [system_prompt] + history
   c. 调用 llm_client.chat_stream(messages, tool_schemas)
   d. 消费 stream：
      - text delta → emit TextDelta，累积 full_text
      - tool_call delta → 按 index 累积 arguments JSON string
   e. stream 结束后，构造 Assistant message（content + tool_calls），追加到 history
   f. 如果 tool_calls 为空 → emit TurnComplete，break
   g. 对每个 tool_call：
      - emit ToolCallRequested
      - 检查 requires_confirmation：
        - true → 调用 frontend.request_tool_confirmation()
          - 用户拒绝 → ToolResult { is_error: true, content: "用户拒绝" }
        - false → 直接执行
      - 执行工具，截断输出
      - 构造 Tool message（带 tool_call_id），追加到 history
      - emit ToolCallResult
   h. 继续 loop（回到 2a）
}
```

**流式 tool call 处理**：async-openai 的 stream 中 tool_calls 是分片的（index + id/name/arguments delta），需要在 stream 消费过程中按 index 组装完整的 tool call。

**Agent 公开方法**：
- `Agent::new(llm_client, tools, settings)` — 创建 Agent，初始化默认 session
- `Agent::run_turn(&self, user_input: &str, frontend: &dyn Frontend)` — 执行一轮对话
- `Agent::cancel(&self)` — 取消当前执行（通过 CancellationToken 或标志位）

### Step 8: 验证前端 — `examples/robit-agent/`

简单的 stdin/stdout 前端，实现 `Frontend` trait：

- `on_event`：
  - `TextDelta` → 直接 print 到 stdout
  - `ToolCallRequested` → 显示工具名和参数
  - `ToolCallResult` → 显示执行结果
  - `TurnComplete` → 打印换行
  - `Error` → 打印错误
- `request_tool_confirmation`：显示 `[Y/n]`，读取 stdin 单字符
- 主循环：读取用户输入 → `Agent::run_turn()` → 等待完成

### Step 9: 集成与验证

1. 更新 workspace `Cargo.toml`，添加 `crates/robit-agent` 和 `examples/robit-agent`
2. 确保 `cargo build --workspace` 编译通过
3. 更新 `docs/roadmap.md`，标记阶段 2 任务为已完成

**验证场景**：
- `cargo run -p robit-agent` 启动
- 输入 "帮我看看当前目录有什么文件" → Agent 调用 `bash` 执行 `dir`（Windows）
- 输入 "读取 Cargo.toml 的内容" → Agent 调用 `read`
- 工具调用结果正确回填，Agent 基于结果继续对话
- 需要确认的工具（`bash`）会提示 Y/N

## 不在阶段 2 范围内

- `edit`、`write`、`grep`、`find`、`ls` 工具（阶段 4）
- TUI 前端（阶段 3）
- 技能系统（阶段 4）
- 摘要压缩（第三层上下文管理，后续）
- 对话历史持久化（后续）
- 重试策略实现（后续）
- Token 精确计数 / tiktoken-rs（后续）
