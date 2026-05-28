# robit-ai 阶段 1 设计规格

> **状态**: Draft
> **创建日期**: 2026-05-28
> **范围**: `crates/robit-ai` — LLM API 层

## 概述

`robit-ai` 是 robit 框架的 LLM API 层，负责：

- 加载多提供商配置（`llms.toml`）
- 通过 OpenAI 兼容协议与 LLM 通信
- 支持流式响应（SSE）
- 向上层 crate 提供统一的 `LlmClient`

**阶段 1 目标**: 能够与 LLM 完成一次多轮流式对话（REPL 验证）。

## 技术决策

### 使用 `async-openai` 作为底层客户端

**决策**: 不手写 SSE 解析和消息类型，直接使用社区成熟的 `async-openai` 库。

**理由**:
- OpenAI 兼容协议覆盖 DeepSeek、QWen 等国内提供商
- 自带 SSE 流式解析、DeltaToolCall 参数拼接、重试和错误处理
- 轻量客户端，不引入 Agent/Chain 等重型抽象

**影响**:
- `types.rs` 和 `stream.rs` 模块**完全移除**
- 所有消息类型直接使用 `async-openai::types`
- `LlmClient` 变成 `async-openai::Client` 的薄封装

### 验证方式：交互式 REPL

**决策**: 阶段 1 使用 `examples/chat.rs` 实现 stdin/stdout 交互式对话循环。

**理由**:
- 多轮对话能暴露上下文传递、历史管理等问题
- REPL 后续可作为 `robit-agent` 阶段的简易前端
- 工作量与最小验证差异不大

## Crate 结构

```txt
crates/
└── robit-ai/             # LLM API 层（library crate）
    ├── Cargo.toml
    └── src/
        ├── lib.rs        # 公共 API 导出（re-export async-openai 常用类型）
        ├── config.rs     # 配置加载（llms.toml, settings.toml, .env）
        ├── client.rs     # LlmClient（async-openai 的薄封装）
        └── error.rs      # LlmError（包装 async-openai 错误）

examples/
└── robit-chat/           # REPL 交互式对话验证（binary crate）
    ├── Cargo.toml
    └── src/
        └── main.rs       # stdin/stdout 多轮对话循环
```

- `robit-ai` 是纯 library crate，不包含 binary
- `robit-chat` 是独立 binary crate，依赖 `robit-ai`，用于阶段 1 验证
- `examples/` 目录统一存放验证工程，后续各阶段的验证项目也放在这里

## 模块设计

### 1. `lib.rs` — 公共 API 导出

```rust
pub mod config;
pub mod client;
pub mod error;

// Re-export async-openai 的核心类型，让上层 crate 不直接依赖 async-openai
pub use async_openai::types::{
    ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessage,
    ChatCompletionRequestUserMessage,
    ChatCompletionRequestAssistantMessage,
    ChatCompletionMessageToolCall,
    ChatCompletionTool,
    ChatCompletionResponseStream,
    CreateChatCompletionResponse,
    CreateChatCompletionStreamResponse,
    Role,
    Usage,
};

pub use client::LlmClient;
pub use config::{LlmConfig, SettingsConfig, ProviderConfig, ModelConfig};
pub use error::LlmError;
```

### 2. `config.rs` — 配置加载

#### 数据结构

```rust
// === llms.toml ===
#[derive(Debug, Deserialize)]
pub struct LlmConfig {
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
    pub providers: HashMap<String, ProviderConfig>,
}

#[derive(Debug, Deserialize)]
pub struct ProviderConfig {
    pub name: Option<String>,
    pub base_url: String,
    pub api_key: String,
    pub models: Vec<ModelConfig>,
}

#[derive(Debug, Deserialize)]
pub struct ModelConfig {
    pub id: String,
    pub name: Option<String>,
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub supports_images: Option<bool>,
    pub supports_tools: Option<bool>,
}

// === settings.toml ===
#[derive(Debug, Deserialize)]
pub struct SettingsConfig {
    pub model: Option<String>,
    pub enabled_tools: Option<Vec<String>>,
    pub context: Option<ContextConfig>,
    pub retry: Option<RetryConfig>,
}

#[derive(Debug, Deserialize)]
pub struct ContextConfig {
    pub max_output_lines: Option<usize>,
    pub max_output_bytes: Option<usize>,
    pub reserve_ratio: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct RetryConfig {
    pub max_retries: Option<u32>,
    pub initial_backoff_ms: Option<u64>,
    pub max_backoff_ms: Option<u64>,
}
```

#### 配置加载流程

```txt
1. dotenvy 加载 ~/.robit/.env（环境变量）
2. 读取 ~/.robit/llms.toml → 解析为 LlmConfig
3. 遍历 providers，将 api_key 中的 ${ENV_VAR} 替换为实际环境变量值
4. 读取 ~/.robit/settings.toml → 解析为 SettingsConfig（不存在则用默认值）
5. 确定最终使用的 provider + model：
   settings.toml 的 model 字段 > llms.toml 的 default_model > 第一个可用模型
```

#### 环境变量替换

```rust
fn resolve_env_var(value: &str) -> String {
    if let Some(var_name) = value.strip_prefix("${").and_then(|s| s.strip_suffix("}")) {
        std::env::var(var_name).unwrap_or_else(|_| value.to_string())
    } else {
        value.to_string()
    }
}
```

### 3. `client.rs` — LlmClient

```rust
pub struct LlmClient {
    client: async_openai::Client<async_openai::config::OpenAIConfig>,
    model: String,
}

impl LlmClient {
    /// 根据配置创建客户端
    pub fn from_config(
        llm_config: &LlmConfig,
        settings: &SettingsConfig,
    ) -> Result<Self, LlmError> { ... }

    /// 流式对话，返回 async Stream
    pub async fn chat_stream(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Option<Vec<ChatCompletionTool>>,
    ) -> Result<ChatCompletionResponseStream, LlmError> { ... }

    /// 非流式对话（用于简单场景或测试）
    pub async fn chat(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Option<Vec<ChatCompletionTool>>,
    ) -> Result<CreateChatCompletionResponse, LlmError> { ... }

    /// 获取当前模型名称
    pub fn model(&self) -> &str { ... }
}
```

### 4. `error.rs` — 错误类型

```rust
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("配置错误: {0}")]
    ConfigError(String),

    #[error("网络连接失败: {0}")]
    ConnectionError(String),

    #[error("认证失败，请检查 API Key 配置")]
    AuthenticationError,

    #[error("请求速率受限，请稍后重试")]
    RateLimitError { retry_after: Option<u64> },

    #[error("模型不可用: {model}")]
    ModelNotFound { model: String },

    #[error("服务端错误 ({status}): {message}")]
    ServerError { status: u16, message: String },

    #[error("响应格式错误: {0}")]
    ParseError(String),

    #[error(transparent)]
    OpenAiError(#[from] async_openai::error::OpenAIError),
}
```

### 5. `robit-chat` — REPL 验证（独立 binary crate）

`examples/robit-chat/src/main.rs`：

```rust
// 简单的 stdin/stdout 交互式对话
// 1. 加载配置 → 创建 LlmClient
// 2. 循环读取用户输入
// 3. 追加到消息历史
// 4. 调用 chat_stream
// 5. 逐 token 打印到 stdout
// 6. 将完整回复追加到消息历史
// 7. 回到步骤 2
```

## Cargo.toml 依赖

### workspace `Cargo.toml`（根目录）

```toml
[workspace]
members = [
    "crates/robit-ai",
    "examples/robit-chat",
]
resolver = "2"
```

### `crates/robit-ai/Cargo.toml`

```toml
[package]
name = "robit-ai"
version = "0.1.0"
edition = "2021"

[dependencies]
async-openai = "0.27"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
thiserror = "2"
tracing = "0.1"
dotenvy = "0.15"
dirs = "6"
futures = "0.3"
```

### `examples/robit-chat/Cargo.toml`

```toml
[package]
name = "robit-chat"
version = "0.1.0"
edition = "2021"

[dependencies]
robit-ai = { path = "../../crates/robit-ai" }
tokio = { version = "1", features = ["full"] }
futures = "0.3"
```

## 验证标准

阶段 1 完成的判定：

1. `cargo build` 编译通过
2. 创建 `~/.robit/llms.toml` 和 `~/.robit/.env` 配置文件
3. `cargo run -p robit-chat` 启动 REPL
4. 输入消息后能收到流式回复（逐 token 显示）
5. 多轮对话中上下文正确传递

## 不在阶段 1 范围内

- 工具调用（阶段 2）
- 重试策略实现（后续增加）
- 多提供商切换（阶段 1 只用一个提供商验证）
- Token 计数（后续增加）
