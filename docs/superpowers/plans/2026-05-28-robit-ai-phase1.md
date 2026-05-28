# robit-ai Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `robit-ai` LLM API layer and a REPL chat client to verify multi-turn streaming conversations with LLM providers.

**Architecture:** `robit-ai` is a thin wrapper around `async-openai`, adding TOML-based multi-provider configuration (`llms.toml`), environment variable resolution (`.env`), and settings loading (`settings.toml`). `robit-chat` is a separate binary crate that uses `robit-ai` for interactive REPL validation.

**Tech Stack:** Rust, `async-openai` 0.27, `tokio`, `serde` + `toml`, `thiserror`, `dotenvy`, `dirs`, `futures`

**Spec:** `docs/specs/2026-05-28-robit-ai-design.md`

---

## File Structure

```txt
Cargo.toml                          # workspace root (MODIFY)
crates/robit-ai/
  Cargo.toml                        # library crate manifest (CREATE)
  src/
    lib.rs                          # public API + re-exports (CREATE)
    error.rs                        # LlmError type (CREATE)
    config.rs                       # LlmConfig, SettingsConfig, loader (CREATE)
    client.rs                       # LlmClient wrapper (CREATE)
examples/robit-chat/
  Cargo.toml                        # binary crate manifest (CREATE)
  src/
    main.rs                         # REPL chat loop (CREATE)
```

---

### Task 1: Workspace Setup

**Files:**
- Modify: `Cargo.toml` (root)
- Create: `crates/robit-ai/Cargo.toml`
- Create: `crates/robit-ai/src/lib.rs` (empty placeholder)
- Create: `examples/robit-chat/Cargo.toml`
- Create: `examples/robit-chat/src/main.rs` (empty placeholder)

- [ ] **Step 1: Update workspace `Cargo.toml`**

Replace the contents of the root `Cargo.toml`:

```toml
[workspace]
members = [
    "crates/robit-ai",
    "examples/robit-chat",
]
resolver = "2"
```

- [ ] **Step 2: Create `crates/robit-ai/Cargo.toml`**

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

- [ ] **Step 3: Create placeholder `crates/robit-ai/src/lib.rs`**

```rust
// robit-ai: LLM API layer for the Robit framework
```

- [ ] **Step 4: Create `examples/robit-chat/Cargo.toml`**

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

- [ ] **Step 5: Create placeholder `examples/robit-chat/src/main.rs`**

```rust
fn main() {
    println!("robit-chat placeholder");
}
```

- [ ] **Step 6: Verify workspace compiles**

Run: `cargo check`
Expected: No errors (warnings about unused dependencies are OK)

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/ examples/
git commit -m "feat: initialize workspace with robit-ai and robit-chat crates"
```

---

### Task 2: Implement Error Type (`error.rs`)

**Files:**
- Create: `crates/robit-ai/src/error.rs`

- [ ] **Step 1: Write `error.rs`**

```rust
//! Error types for the robit-ai crate.

use thiserror::Error;

/// Unified error type for LLM operations.
#[derive(Debug, Error)]
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

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p robit-ai`
Expected: No errors

- [ ] **Step 3: Commit**

```bash
git add crates/robit-ai/src/error.rs
git commit -m "feat(robit-ai): add LlmError type"
```

---

### Task 3: Implement Configuration Loading (`config.rs`)

**Files:**
- Create: `crates/robit-ai/src/config.rs`

- [ ] **Step 1: Write the data structures and parsing logic**

Create `crates/robit-ai/src/config.rs`:

```rust
//! Configuration loading for llms.toml, settings.toml, and .env files.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::LlmError;

// ============================================================================
// llms.toml structures
// ============================================================================

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

// ============================================================================
// settings.toml structures
// ============================================================================

#[derive(Debug, Deserialize, Default)]
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

// ============================================================================
// Resolved model reference (provider key + model id)
// ============================================================================

#[derive(Debug, Clone)]
pub struct ResolvedModel {
    pub provider_key: String,
    pub model_id: String,
    pub base_url: String,
    pub api_key: String,
}

// ============================================================================
// Loader
// ============================================================================

/// Returns the ~/.robit/ directory path.
fn robit_home() -> Result<PathBuf, LlmError> {
    let home = dirs::home_dir().ok_or_else(|| {
        LlmError::ConfigError("无法获取用户主目录".to_string())
    })?;
    Ok(home.join(".robit"))
}

/// Replace `${ENV_VAR}` patterns with actual environment variable values.
fn resolve_env_var(value: &str) -> String {
    if let Some(var_name) = value.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
        std::env::var(var_name).unwrap_or_else(|_| value.to_string())
    } else {
        value.to_string()
    }
}

/// Load .env from ~/.robit/.env if it exists.
pub fn load_env() {
    if let Ok(robit_dir) = robit_home() {
        let env_path = robit_dir.join(".env");
        if env_path.exists() {
            let _ = dotenvy::from_path(&env_path);
        }
    }
}

/// Load and parse ~/.robit/llms.toml.
/// Resolves `${ENV_VAR}` in api_key fields after loading.
pub fn load_llm_config() -> Result<LlmConfig, LlmError> {
    let path = robit_home()?.join("llms.toml");
    let content = std::fs::read_to_string(&path).map_err(|e| {
        LlmError::ConfigError(format!("无法读取 {}: {}", path.display(), e))
    })?;

    let mut config: LlmConfig = toml::from_str(&content).map_err(|e| {
        LlmError::ConfigError(format!("解析 llms.toml 失败: {}", e))
    })?;

    // Resolve environment variables in api_key fields
    for provider in config.providers.values_mut() {
        provider.api_key = resolve_env_var(&provider.api_key);
    }

    Ok(config)
}

/// Load and parse ~/.robit/settings.toml.
/// Returns default SettingsConfig if the file does not exist.
pub fn load_settings() -> Result<SettingsConfig, LlmError> {
    let path = robit_home()?.join("settings.toml");
    if !path.exists() {
        return Ok(SettingsConfig::default());
    }

    let content = std::fs::read_to_string(&path).map_err(|e| {
        LlmError::ConfigError(format!("无法读取 {}: {}", path.display(), e))
    })?;

    let config: SettingsConfig = toml::from_str(&content).map_err(|e| {
        LlmError::ConfigError(format!("解析 settings.toml 失败: {}", e))
    })?;

    Ok(config)
}

/// Resolve which provider and model to use.
///
/// Priority: settings.toml `model` > llms.toml `default_model` > first available model.
pub fn resolve_model(
    llm_config: &LlmConfig,
    settings: &SettingsConfig,
) -> Result<ResolvedModel, LlmError> {
    // Determine the "provider/model" string
    let model_ref = settings
        .model
        .clone()
        .or_else(|| llm_config.default_model.clone())
        .or_else(|| {
            // Fallback: first provider, first model
            llm_config.providers.iter().find_map(|(key, provider)| {
                provider.models.first().map(|m| format!("{}/{}", key, m.id))
            })
        })
        .ok_or_else(|| {
            LlmError::ConfigError("未找到可用的模型配置".to_string())
        })?;

    // Parse "provider/model"
    let parts: Vec<&str> = model_ref.splitn(2, '/').collect();
    if parts.len() != 2 {
        return Err(LlmError::ConfigError(format!(
            "模型引用格式错误: '{}'，应为 'provider/model'",
            model_ref
        )));
    }

    let provider_key = parts[0];
    let model_id = parts[1];

    let provider = llm_config
        .providers
        .get(provider_key)
        .ok_or_else(|| {
            LlmError::ConfigError(format!("提供商 '{}' 未在 llms.toml 中定义", provider_key))
        })?;

    // Verify model exists in provider
    let _model = provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .ok_or_else(|| {
            LlmError::ModelNotFound {
                model: model_ref.clone(),
            }
        })?;

    if provider.api_key.is_empty() || provider.api_key.starts_with("${") {
        return Err(LlmError::ConfigError(format!(
            "提供商 '{}' 的 API Key 未配置或环境变量未设置",
            provider_key
        )));
    }

    Ok(ResolvedModel {
        provider_key: provider_key.to_string(),
        model_id: model_id.to_string(),
        base_url: provider.base_url.clone(),
        api_key: provider.api_key.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_env_var_with_env_set() {
        std::env::set_var("ROBIT_TEST_KEY", "test-value-123");
        assert_eq!(resolve_env_var("${ROBIT_TEST_KEY}"), "test-value-123");
        std::env::remove_var("ROBIT_TEST_KEY");
    }

    #[test]
    fn test_resolve_env_var_without_env() {
        assert_eq!(
            resolve_env_var("${ROBIT_NONEXISTENT_KEY}"),
            "${ROBIT_NONEXISTENT_KEY}"
        );
    }

    #[test]
    fn test_resolve_env_var_plain_string() {
        assert_eq!(resolve_env_var("plain-key"), "plain-key");
    }

    #[test]
    fn test_parse_llm_config_toml() {
        let toml_str = r#"
            default_provider = "deepseek"
            default_model = "deepseek/deepseek-chat"

            [providers.deepseek]
            name = "DeepSeek"
            base_url = "https://api.deepseek.com/v1"
            api_key = "sk-test-key"

            [[providers.deepseek.models]]
            id = "deepseek-chat"
            name = "DeepSeek Chat"
            context_window = 65536
            max_output_tokens = 8192
            supports_images = false
            supports_tools = true
        "#;

        let config: LlmConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.default_provider.as_deref(), Some("deepseek"));
        assert_eq!(config.default_model.as_deref(), Some("deepseek/deepseek-chat"));
        assert!(config.providers.contains_key("deepseek"));

        let provider = &config.providers["deepseek"];
        assert_eq!(provider.base_url, "https://api.deepseek.com/v1");
        assert_eq!(provider.api_key, "sk-test-key");
        assert_eq!(provider.models.len(), 1);
        assert_eq!(provider.models[0].id, "deepseek-chat");
        assert_eq!(provider.models[0].context_window, Some(65536));
    }

    #[test]
    fn test_parse_settings_toml() {
        let toml_str = r#"
            model = "deepseek/deepseek-chat"
            enabled_tools = ["read", "bash"]

            [context]
            max_output_lines = 500
            reserve_ratio = 0.2

            [retry]
            max_retries = 3
        "#;

        let settings: SettingsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(settings.model.as_deref(), Some("deepseek/deepseek-chat"));
        assert_eq!(
            settings.enabled_tools.as_deref(),
            Some(&["read".to_string(), "bash".to_string()][..])
        );
        assert!(settings.context.is_some());
        assert_eq!(settings.context.as_ref().unwrap().max_output_lines, Some(500));
        assert!(settings.retry.is_some());
        assert_eq!(settings.retry.as_ref().unwrap().max_retries, Some(3));
    }

    #[test]
    fn test_parse_settings_default() {
        let settings = SettingsConfig::default();
        assert!(settings.model.is_none());
        assert!(settings.enabled_tools.is_none());
        assert!(settings.context.is_none());
        assert!(settings.retry.is_none());
    }

    #[test]
    fn test_resolve_model_from_settings() {
        let toml_str = r#"
            default_provider = "deepseek"
            default_model = "deepseek/deepseek-chat"

            [providers.deepseek]
            base_url = "https://api.deepseek.com/v1"
            api_key = "sk-test"

            [[providers.deepseek.models]]
            id = "deepseek-chat"
        "#;

        let llm_config: LlmConfig = toml::from_str(toml_str).unwrap();
        let settings = SettingsConfig {
            model: Some("deepseek/deepseek-chat".to_string()),
            ..Default::default()
        };

        let resolved = resolve_model(&llm_config, &settings).unwrap();
        assert_eq!(resolved.provider_key, "deepseek");
        assert_eq!(resolved.model_id, "deepseek-chat");
        assert_eq!(resolved.base_url, "https://api.deepseek.com/v1");
        assert_eq!(resolved.api_key, "sk-test");
    }

    #[test]
    fn test_resolve_model_fallback_to_default() {
        let toml_str = r#"
            default_model = "deepseek/deepseek-chat"

            [providers.deepseek]
            base_url = "https://api.deepseek.com/v1"
            api_key = "sk-test"

            [[providers.deepseek.models]]
            id = "deepseek-chat"
        "#;

        let llm_config: LlmConfig = toml::from_str(toml_str).unwrap();
        let settings = SettingsConfig::default();

        let resolved = resolve_model(&llm_config, &settings).unwrap();
        assert_eq!(resolved.model_id, "deepseek-chat");
    }

    #[test]
    fn test_resolve_model_not_found() {
        let toml_str = r#"
            [providers.deepseek]
            base_url = "https://api.deepseek.com/v1"
            api_key = "sk-test"

            [[providers.deepseek.models]]
            id = "deepseek-chat"
        "#;

        let llm_config: LlmConfig = toml::from_str(toml_str).unwrap();
        let settings = SettingsConfig {
            model: Some("deepseek/nonexistent".to_string()),
            ..Default::default()
        };

        let result = resolve_model(&llm_config, &settings);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_model_invalid_format() {
        let toml_str = r#"
            [providers.deepseek]
            base_url = "https://api.deepseek.com/v1"
            api_key = "sk-test"

            [[providers.deepseek.models]]
            id = "deepseek-chat"
        "#;

        let llm_config: LlmConfig = toml::from_str(toml_str).unwrap();
        let settings = SettingsConfig {
            model: Some("no-slash-here".to_string()),
            ..Default::default()
        };

        let result = resolve_model(&llm_config, &settings);
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p robit-ai`
Expected: All tests pass (8 tests)

- [ ] **Step 3: Commit**

```bash
git add crates/robit-ai/src/config.rs
git commit -m "feat(robit-ai): add config loading for llms.toml and settings.toml"
```

---

### Task 4: Implement LlmClient (`client.rs`)

**Files:**
- Create: `crates/robit-ai/src/client.rs`

- [ ] **Step 1: Write `client.rs`**

```rust
//! LlmClient: a thin wrapper around async-openai with multi-provider config support.

use async_openai::config::OpenAIConfig;
use async_openai::types::{
    ChatCompletionRequestMessage, ChatCompletionResponseStream, ChatCompletionTool,
    CreateChatCompletionRequest, CreateChatCompletionResponse,
};

use crate::config::{LlmConfig, ResolvedModel, SettingsConfig, resolve_model};
use crate::error::LlmError;

pub struct LlmClient {
    client: async_openai::Client<OpenAIConfig>,
    model: String,
    resolved: ResolvedModel,
}

impl LlmClient {
    /// Create a new LlmClient from loaded configuration.
    pub fn from_config(
        llm_config: &LlmConfig,
        settings: &SettingsConfig,
    ) -> Result<Self, LlmError> {
        let resolved = resolve_model(llm_config, settings)?;

        let config = OpenAIConfig::new()
            .with_api_base(&resolved.base_url)
            .with_api_key(&resolved.api_key);

        let client = async_openai::Client::with_config(config);

        Ok(Self {
            client,
            model: resolved.model_id.clone(),
            resolved,
        })
    }

    /// Streaming chat completion. Returns an async stream of response chunks.
    pub async fn chat_stream(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Option<Vec<ChatCompletionTool>>,
    ) -> Result<ChatCompletionResponseStream, LlmError> {
        let request = CreateChatCompletionRequest {
            model: self.model.clone(),
            messages,
            tools,
            stream: Some(true),
            ..Default::default()
        };

        let stream = self.client.chat().create_stream(request).await?;
        Ok(stream)
    }

    /// Non-streaming chat completion. Returns the full response.
    pub async fn chat(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Option<Vec<ChatCompletionTool>>,
    ) -> Result<CreateChatCompletionResponse, LlmError> {
        let request = CreateChatCompletionRequest {
            model: self.model.clone(),
            messages,
            tools,
            ..Default::default()
        };

        let response = self.client.chat().create(request).await?;
        Ok(response)
    }

    /// Get the current model ID (e.g. "deepseek-chat").
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Get the provider key (e.g. "deepseek").
    pub fn provider(&self) -> &str {
        &self.resolved.provider_key
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p robit-ai`
Expected: No errors

- [ ] **Step 3: Commit**

```bash
git add crates/robit-ai/src/client.rs
git commit -m "feat(robit-ai): add LlmClient wrapper around async-openai"
```

---

### Task 5: Wire Up `lib.rs`

**Files:**
- Modify: `crates/robit-ai/src/lib.rs`

- [ ] **Step 1: Replace `lib.rs` contents**

```rust
//! robit-ai: LLM API layer for the Robit framework.
//!
//! Provides a unified `LlmClient` for interacting with multiple LLM providers
//! through the OpenAI-compatible protocol. Configuration is loaded from
//! `~/.robit/llms.toml` and `~/.robit/settings.toml`.

pub mod client;
pub mod config;
pub mod error;

// Re-export async-openai core types so downstream crates don't need to depend on async-openai directly.
pub use async_openai::types::{
    ChatCompletionMessageToolCall, ChatCompletionRequestAssistantMessage,
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestUserMessage, ChatCompletionResponseStream, ChatCompletionTool,
    CreateChatCompletionResponse, CreateChatCompletionStreamResponse, Role, Usage,
};

pub use client::LlmClient;
pub use config::{
    ContextConfig, LlmConfig, ModelConfig, ProviderConfig, ResolvedModel, RetryConfig,
    SettingsConfig,
};
pub use error::LlmError;
```

- [ ] **Step 2: Verify full crate compiles and tests pass**

Run: `cargo test -p robit-ai`
Expected: All tests pass, no compilation errors

- [ ] **Step 3: Commit**

```bash
git add crates/robit-ai/src/lib.rs
git commit -m "feat(robit-ai): wire up lib.rs with module declarations and re-exports"
```

---

### Task 6: Implement REPL Chat (`robit-chat`)

**Files:**
- Modify: `examples/robit-chat/src/main.rs`

- [ ] **Step 1: Write `main.rs`**

```rust
//! robit-chat: REPL interactive chat for Phase 1 validation.
//!
//! Usage: cargo run -p robit-chat

use futures::StreamExt;
use robit_ai::{
    ChatCompletionRequestMessage, ChatCompletionRequestUserMessage,
    ChatCompletionRequestAssistantMessage, ChatCompletionRequestSystemMessage,
    LlmClient,
};
use robit_ai::config::{load_env, load_llm_config, load_settings};
use std::io::{self, BufRead, Write};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load environment variables from ~/.robit/.env
    load_env();

    // Load configuration
    let llm_config = load_llm_config()?;
    let settings = load_settings()?;

    // Create LLM client
    let client = LlmClient::from_config(&llm_config, &settings)?;
    println!(
        "Robit Chat | provider: {} | model: {}",
        client.provider(),
        client.model()
    );
    println!("输入消息开始对话，输入 exit 或 Ctrl+D 退出\n");

    // Conversation history
    let mut messages: Vec<ChatCompletionRequestMessage> = vec![
        ChatCompletionRequestMessage::System(
            ChatCompletionRequestSystemMessage {
                content: "你是 Robit，一个 AI 编程代理。请直接回答用户问题。".into(),
                name: None,
            }
            .into(),
        ),
    ];

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        // Read user input
        print!("> ");
        stdout.flush()?;

        let mut input = String::new();
        if stdin.lock().read_line(&mut input)? == 0 {
            // EOF (Ctrl+D)
            break;
        }

        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        if input == "exit" || input == "/exit" {
            break;
        }

        // Append user message to history
        messages.push(ChatCompletionRequestMessage::User(
            ChatCompletionRequestUserMessage {
                content: input.to_string().into(),
                name: None,
            }
            .into(),
        ));

        // Stream response
        print!("Robit: ");
        stdout.flush()?;

        let mut stream = client.chat_stream(messages.clone(), None).await?;
        let mut full_response = String::new();

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    if let Some(choice) = chunk.choices.first() {
                        if let Some(content) = &choice.delta.content {
                            print!("{}", content);
                            stdout.flush()?;
                            full_response.push_str(content);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("\n[错误] {}", e);
                    break;
                }
            }
        }

        println!(); // newline after response

        // Append assistant response to history
        if !full_response.is_empty() {
            messages.push(ChatCompletionRequestMessage::Assistant(
                ChatCompletionRequestAssistantMessage {
                    content: Some(full_response).into(),
                    name: None,
                    tool_calls: None,
                    refusal: None,
                    audio: None,
                }
                .into(),
            ));
        }
    }

    println!("\n再见！");
    Ok(())
}
```

- [ ] **Step 2: Add `anyhow` dependency to `robit-chat`**

Update `examples/robit-chat/Cargo.toml`:

```toml
[package]
name = "robit-chat"
version = "0.1.0"
edition = "2021"

[dependencies]
robit-ai = { path = "../../crates/robit-ai" }
tokio = { version = "1", features = ["full"] }
futures = "0.3"
anyhow = "1"
```

- [ ] **Step 3: Verify compilation**

Run: `cargo build -p robit-chat`
Expected: No errors

- [ ] **Step 4: Commit**

```bash
git add examples/robit-chat/
git commit -m "feat(robit-chat): implement REPL interactive chat for Phase 1 validation"
```

---

### Task 7: Manual End-to-End Verification

**Files:**
- No code changes. Requires user to have `~/.robit/llms.toml` and `~/.robit/.env` configured.

- [ ] **Step 1: Create sample config files for testing**

Create `~/.robit/llms.toml` (adjust provider/key for your setup):

```toml
default_provider = "deepseek"
default_model = "deepseek/deepseek-chat"

[providers.deepseek]
name = "DeepSeek"
base_url = "https://api.deepseek.com/v1"
api_key = "${DEEPSEEK_API_KEY}"

[[providers.deepseek.models]]
id = "deepseek-chat"
name = "DeepSeek Chat"
context_window = 65536
max_output_tokens = 8192
supports_images = false
supports_tools = true
```

Create `~/.robit/.env`:

```txt
DEEPSEEK_API_KEY=your-actual-api-key-here
```

- [ ] **Step 2: Run the REPL**

Run: `cargo run -p robit-chat`
Expected output:

```txt
Robit Chat | provider: deepseek | model: deepseek-chat
输入消息开始对话，输入 exit 或 Ctrl+D 退出
```

- [ ] **Step 3: Test single-turn conversation**

Type: `你好，请用一句话介绍自己`
Expected: Streaming response displayed token by token

- [ ] **Step 4: Test multi-turn conversation**

Type: `请记住数字 42`
Then type: `我刚才让你记住的数字是多少？`
Expected: The model should recall "42" from conversation history

- [ ] **Step 5: Test exit**

Type: `exit` or press Ctrl+D
Expected: Program exits with `再见！`

- [ ] **Step 6: Final commit**

```bash
git add -A
git commit -m "chore: phase 1 complete - robit-ai + robit-chat verified"
```

---

## Notes

- **`async-openai` type conversions**: The `async-openai` crate uses `From` impls for converting between message types (e.g., `ChatCompletionRequestUserMessage` → `ChatCompletionRequestMessage`). The `.into()` calls in `main.rs` handle these conversions.
- **No retry in Phase 1**: Retry strategy is configured in `settings.toml` but not implemented in `LlmClient` yet. This is intentional — Phase 2 will add retry logic.
- **Stream type**: `chat_stream()` returns `ChatCompletionResponseStream` which is `futures::stream::BoxStream<'static, Result<CreateChatCompletionStreamResponse, OpenAIError>>`.
- **`anyhow` in `robit-chat`**: Used only in the binary crate for ergonomic error handling. The `robit-ai` library crate uses `thiserror` exclusively.
- **Spec note**: The spec mentions `examples/chat.rs` in the "验证方式" section but the actual implementation uses `examples/robit-chat/` as a separate binary crate per the updated crate structure.
