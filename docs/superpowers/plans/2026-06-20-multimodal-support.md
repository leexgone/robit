# 多模态支持（图片理解）实现计划

> **目标**：当用户上传图片或文件时，程序接收并存储在 `{cwd}/media/` 目录下，然后通知 LLM。对于支持多模态的模型，加载图片进行理解。
> **验证**：QQ Bot 接收用户图片 → 存储 → 支持视觉的模型能理解图片内容。

## 上下文

当前架构：
- QQ Bot 已经能接收 `MediaAttachment`（包含图片 URL、content_type、文件名等）
- 目前仅将附件转换为文本描述（如 `"[用户发送了图片: image.png (200KB)]"`）发送给 LLM
- `ModelConfig` 已有 `supports_images` 字段，但 `ResolvedModel` 缺少该字段
- async-openai 0.41.0 已完整支持多模态消息，但类型未从 `robit-ai` 导出

## 实现步骤（按依赖顺序）

### Step 1: robit-ai — 导出多模态类型并补充 ResolvedModel

**文件**：`crates/robit-ai/src/lib.rs`

补充导出 async-openai 的多模态类型：
```rust
pub use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionRequestAssistantMessage,
    ChatCompletionRequestUserMessage, ChatCompletionResponseStream, ChatCompletionTools,
    CompletionUsage, CreateChatCompletionResponse, CreateChatCompletionStreamResponse, Role,
    // 新增：多模态类型
    ChatCompletionRequestUserMessageContent, ChatCompletionRequestUserMessageContentPart,
    ChatCompletionRequestMessageContentPartText, ChatCompletionRequestMessageContentPartImage,
    ChatCompletionRequestMessageContentPartAudio, ChatCompletionRequestMessageContentPartFile,
};
pub use async_openai::types::image_url::{ImageDetail, ImageUrl};
```

**文件**：`crates/robit-ai/src/config.rs`

在 `ResolvedModel` 中补充 `supports_images` 和 `supports_tools`：
```rust
pub struct ResolvedModel {
    pub profile_name: String,
    pub model_id: String,
    pub base_url: String,
    pub api_key: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub context_window: Option<u64>,
    // 新增
    pub supports_images: bool,
    pub supports_tools: bool,
}
```

在 `resolve_profile()` 中填充这些字段：
```rust
Ok(ResolvedModel {
    profile_name: provider_key,
    model_id: model.id.clone(),
    base_url: provider.base_url.clone(),
    api_key: provider.api_key.clone(),
    max_tokens: model.max_tokens,
    temperature: model.temperature,
    context_window: model.context_window,
    supports_images: model.supports_images.unwrap_or(false),
    supports_tools: model.supports_tools.unwrap_or(false),
})
```

**文件**：`crates/robit-ai/src/client.rs`

在 `LlmClient` 中添加访问器方法：
```rust
impl LlmClient {
    // ... 现有方法

    pub fn supports_images(&self) -> bool {
        self.resolved.supports_images
    }

    pub fn supports_tools(&self) -> bool {
        self.resolved.supports_tools
    }
}
```

### Step 2: robit-agent — 定义通用 MediaAttachment 并扩展 FrontendMessage

**文件**：`crates/robit-agent/src/event.rs`

定义通用的 `MediaAttachment`：
```rust
#[derive(Debug, Clone)]
pub struct MediaAttachment {
    /// MIME type (e.g. "image/jpeg", "application/pdf")
    pub content_type: String,
    /// URL to access the media
    pub url: String,
    /// Original filename if available
    pub filename: Option<String>,
    /// File size in bytes if available
    pub size: Option<u64>,
    /// Image width in pixels if available
    pub width: Option<u32>,
    /// Image height in pixels if available
    pub height: Option<u32>,
}

impl MediaAttachment {
    pub fn is_image(&self) -> bool {
        self.content_type.starts_with("image/")
    }

    pub fn describe(&self) -> String {
        let filename = self.filename.as_deref().unwrap_or("unknown");
        let type_desc = if self.is_image() { "图片" } else { "文件" };
        let size_str = self.size.map(|s| format!(" ({:.1}KB)", s as f64 / 1024.0)).unwrap_or_default();
        format!("[用户发送了{type_desc}: {filename}{size_str}]")
    }
}
```

扩展 `FrontendMessage` 支持附件：
```rust
pub enum FrontendMessage {
    /// User typed a new message with optional media attachments
    UserInput {
        text: String,
        #[serde(default)]
        attachments: Vec<MediaAttachment>,
    },

    /// User wants to cancel the current operation
    Cancel,

    /// User responded to a tool confirmation request
    ConfirmationResponse {
        tool_call_id: String,
        approved: bool,
    },
}

// 保持向后兼容：从 String 转换
impl From<String> for FrontendMessage {
    fn from(text: String) -> Self {
        Self::UserInput { text, attachments: vec![] }
    }
}
```

### Step 3: robit-agent — 媒体下载工具

**文件**：`crates/robit-agent/src/media.rs` （新建）

提供媒体下载和 base64 编码功能：
```rust
use reqwest::Client;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid media content")]
    InvalidContent,
}

/// Download media from URL and save to the specified directory
pub async fn download_media(
    url: &str,
    filename: Option<&str>,
    save_dir: &PathBuf,
) -> Result<PathBuf, MediaError> {
    // Create directory if it doesn't exist
    tokio::fs::create_dir_all(save_dir).await?;

    // Determine filename
    let save_filename = filename.unwrap_or_else(|| {
        // Generate a unique filename from URL or UUID
        uuid::Uuid::new_v4().to_string()
    });
    let save_path = save_dir.join(save_filename);

    // Download
    let client = Client::new();
    let response = client.get(url).send().await?;
    let bytes = response.bytes().await?;

    if bytes.is_empty() {
        return Err(MediaError::InvalidContent);
    }

    tokio::fs::write(&save_path, &bytes).await?;

    Ok(save_path)
}

/// Download media from URL and encode as base64 data URL
pub async fn download_and_encode_base64(url: &str, content_type: &str) -> Result<String, MediaError> {
    use base64::{engine::general_purpose, Engine as _};

    let client = Client::new();
    let bytes = client.get(url).send().await?.bytes().await?;

    if bytes.is_empty() {
        return Err(MediaError::InvalidContent);
    }

    let base64 = general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:{};base64,{}", content_type, base64))
}
```

**文件**：`crates/robit-agent/src/lib.rs`

导出新模块：
```rust
pub mod media;
pub use media::{download_media, download_and_encode_base64, MediaError};
```

**文件**：`crates/robit-agent/Cargo.toml`

添加依赖：
```toml
reqwest = { version = "0.11", features = ["json", "stream"] }
base64 = "0.22"
```

### Step 4: robit-agent — Agent 支持多模态消息

**文件**：`crates/robit-agent/src/agent.rs`

首先更新 `Agent::new()` 保存 `llm_client` 的引用，然后修改 `run_turn()`：

```rust
impl Agent {
    // ... 现有代码

    async fn run_turn(&mut self, message: FrontendMessage, frontend: Arc<dyn Frontend>) {
        // ... 提取 text 和 attachments

        // Build user message
        let user_message = if self.llm_client.supports_images() && !attachments.is_empty() {
            self.build_multimodal_message(&text, &attachments).await
        } else {
            ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                content: text.clone().into(),
                name: None,
            })
        };

        // Add to history
        // ...
    }

    /// Build a multimodal message with text + images
    async fn build_multimodal_message(
        &self,
        text: &str,
        attachments: &[MediaAttachment],
    ) -> ChatCompletionRequestMessage {
        use robit_ai::{
            ChatCompletionRequestUserMessage, ChatCompletionRequestUserMessageContent,
            ChatCompletionRequestUserMessageContentPart,
            ChatCompletionRequestMessageContentPartText,
            ChatCompletionRequestMessageContentPartImage,
            ImageUrl,
        };

        let mut parts = vec![ChatCompletionRequestUserMessageContentPart::Text(
            ChatCompletionRequestMessageContentPartText {
                text: text.to_string(),
            },
        )];

        // Add images
        for attachment in attachments {
            if attachment.is_image() {
                // Download and encode as base64
                if let Ok(base64_url) = download_and_encode_base64(
                    &attachment.url,
                    &attachment.content_type,
                ).await {
                    parts.push(ChatCompletionRequestUserMessageContentPart::ImageUrl(
                        ChatCompletionRequestMessageContentPartImage {
                            image_url: ImageUrl {
                                url: base64_url,
                                detail: None,
                            }
                        }
                    ));
                }
            }
        }

        ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
            content: ChatCompletionRequestUserMessageContent::Array(parts),
            name: None,
        })
    }
}
```

### Step 5: robit-chatbot — 传递附件到 Agent

**文件**：`crates/robit-chatbot/src/manager.rs`

修改 `handle_message()`：
```rust
async fn handle_message(&self, msg: ChatMessage) {
    // ...

    // Convert chatbot MediaAttachment to agent MediaAttachment
    let attachments: Vec<robit_agent::event::MediaAttachment> =
        msg.attachments.into_iter().map(|a| a.into()).collect();

    // Send to agent
    if let Err(e) = tx.send(FrontendMessage::UserInput {
        text: msg.text,
        attachments,
    }).await {
        // ...
    }
}
```

**文件**：`crates/robit-chatbot/src/adapter.rs`

实现转换 trait：
```rust
impl From<MediaAttachment> for robit_agent::event::MediaAttachment {
    fn from(att: MediaAttachment) -> Self {
        Self {
            content_type: att.content_type,
            url: att.url,
            filename: att.filename,
            size: att.size,
            width: att.width,
            height: att.height,
        }
    }
}
```

### Step 6: robit-chatbot — 下载并存储媒体文件

**文件**：`crates/robit-chatbot/src/manager.rs`

在 `handle_message()` 中添加下载逻辑：
```rust
async fn handle_message(&self, msg: ChatMessage) {
    let chat_id = msg.sender.chat_id.clone();

    // Download and store media files
    let media_dir = self.working_dir.join("media");
    for attachment in &msg.attachments {
        if let Err(e) = robit_agent::media::download_media(
            &attachment.url,
            attachment.filename.as_deref(),
            &media_dir,
        ).await {
            tracing::warn!("Failed to download media: {}", e);
        }
    }

    // ... rest of existing code
}
```

### Step 7: 配置示例

更新 `config.toml` 示例，添加支持视觉的模型配置：
```toml
[[providers.qwen.models]]
id = "qwen-vl-max"
name = "Qwen Vision Max"
context_window = 32768
temperature = 0.7
supports_images = true
supports_tools = true
```

## 降级策略（Graceful Degradation）

为保持稳定性：
1. **模型不支持图片**：自动降级为仅发送文本描述（当前行为）
2. **图片下载失败**：记录 `warn!` 日志，继续发送文本描述
3. **图片过大**：配置大小限制（如 10MB），超过时仅发送描述

## 涉及文件清单

| 文件 | 变更类型 |
|------|---------|
| `crates/robit-ai/src/lib.rs` | 补充导出多模态类型 |
| `crates/robit-ai/src/config.rs` | `ResolvedModel` 新增 `supports_images/toools` |
| `crates/robit-ai/src/client.rs` | 添加 `supports_images/toools` 访问器 |
| `crates/robit-agent/src/event.rs` | 新增通用 `MediaAttachment` + 扩展 `FrontendMessage` |
| `crates/robit-agent/src/media.rs` | 新建媒体下载/编码工具 |
| `crates/robit-agent/src/agent.rs` | 支持构建多模态消息 |
| `crates/robit-agent/src/lib.rs` | 导出 `media` 模块 |
| `crates/robit-agent/Cargo.toml` | 添加 `reqwest`、`base64` 依赖 |
| `crates/robit-chatbot/src/manager.rs` | 传递附件 + 下载存储 |
| `crates/robit-chatbot/src/adapter.rs` | 实现 `MediaAttachment` 类型转换 |

## 验证场景

1. **使用支持视觉的模型**：
   - 用户发送图片给 QQ Bot
   - 图片下载到 `{cwd}/media/` 目录
   - LLM 能看到并描述图片内容

2. **使用不支持视觉的模型**：
   - 用户发送图片给 QQ Bot
   - 图片下载到 `{cwd}/media/` 目录（可选配置）
   - LLM 收到文本描述：`"[用户发送了图片: image.png (200KB)]"`

## 配置项（可选）

在 `[app.bot]` 下添加可选配置：
```toml
[app.bot.media]
store_media = true       # 是否保存媒体到本地
media_dir = "media"      # 存储目录
max_image_size = 10485760 # 最大图片大小（字节，默认 10MB）
```
