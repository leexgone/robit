# Plan: robit-qq 图片/文件收发支持

> **Date:** 2026-06-20
> **Status:** Draft
> **Scope:** `robit-qq` + `robit-chatbot` 的媒体消息支持

---

## 1. 问题分析

### 当前状态

robit-qq 当前是**纯文本**的：
- `ChatMessage` 只有 `text: String` 字段
- QQ WebSocket 消息解析只提取 `content` 字段，忽略附件
- 发送消息只使用 `msg_type: 0`（文本）或 `msg_type: 2`（Markdown）
- Markdown 清洗器把 `![]()` 图片语法转换为 `[Image: alt]` 纯文本

### QQ Bot API 能力调研

**接收消息（WebSocket Dispatch）：**
QQ 的 `C2C_MESSAGE_CREATE` / `GROUP_AT_MESSAGE_CREATE` 事件的 `d` 字段除了 `content` 外，**还包含 `attachments` 数组**，每个 attachment 有：
- `url` — 媒体 URL（图片/文件的下载地址）
- `content_type` — 如 `image/jpeg`、`image/png` 等
- `filename` — 文件名（如果有）
- `size` — 文件大小
- `height` / `width` — 图片尺寸

**发送消息：**

QQ Bot 提供以下几种发送富媒体的方式：

1. **Markdown 消息 (msg_type=2)**：QQ 支持 Markdown 模板，但 `![image](url)` 语法需要图片 URL 是**已上传到 QQ 的 URL**，不能是任意外链。

2. **富媒体上传 API**：`POST /v2/groups/{group_openid}/files` 和 `POST /v2/users/{user_openid}/files`，用于上传图片/文件，返回可在消息中引用的 URL。

3. **Markdown 模板中的图片**：QQ Bot 支持在 Markdown 模板中使用 `![img#N](url)` 格式引用已上传的图片。

4. **Ark 消息 (msg_type=3)**：QQ 的富媒体卡片消息，可以包含图片、链接等结构化内容。模板需在 QQ 开放平台配置。

5. **Embed 消息 (msg_type=4)**：类似 Discord Embed，支持 title、description、image、thumbnail 等字段。最直接的富媒体发送方式。

6. **富媒体消息 (msg_type=7)**：直接发送图片/文件，需先上传获取 `file_info`。

**结论：最实用的方案是使用 `msg_type=7`（富媒体消息）+ `msg_type=4`（Embed 消息）组合。**

---

## 2. 设计方案

### 2.1 整体策略

采用分层改造，从底层类型到上层流程逐步添加媒体支持：

```
QQ 事件 → protocol.rs (解析 attachments) 
    → platform.rs (构建 ChatMessage with media) 
    → adapter.rs (ChatMessage 增加 media 字段)
    → manager.rs (透传)
    → agent (LLM 看到媒体描述 + URL)
    
LLM 输出 → frontend.rs (flush_buffer 时处理媒体引用)
    → adapter.rs (send_message 扩展为支持 media)
    → platform.rs (QQ 上传媒体 + 发送)
```

### 2.2 修改清单

#### Step 1: 扩展 `ChatMessage` 类型（`robit-chatbot/src/adapter.rs`）

```rust
/// 媒体附件
#[derive(Debug, Clone)]
pub struct MediaAttachment {
    /// MIME 类型: image/jpeg, image/png, application/pdf 等
    pub content_type: String,
    /// 媒体 URL（QQ 返回的下载地址）
    pub url: String,
    /// 文件名（可选）
    pub filename: Option<String>,
    /// 文件大小（字节，可选）
    pub size: Option<u64>,
    /// 图片宽度（可选）
    pub width: Option<u32>,
    /// 图片高度（可选）
    pub height: Option<u32>,
}

/// A parsed chat message from the platform.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub text: String,
    pub sender: SenderInfo,
    pub attachments: Vec<MediaAttachment>,  // NEW
}
```

#### Step 2: 扩展 `SendMessage` 相关类型（`robit-chatbot/src/adapter.rs`）

```rust
/// 发送消息时可以附带媒体
pub struct SendMessageParams {
    pub text: String,
    pub media_urls: Vec<String>,   // 已上传到平台的媒体 URL
    pub media_type: Option<String>, // "image" | "file"
}

/// 扩展 PlatformAdapter
pub trait PlatformAdapter: Send + Sync + 'static {
    // ... existing methods ...
    
    /// 上传媒体文件到平台，返回平台 URL
    async fn upload_media(&self, chat_id: &str, file_path: &str, media_type: &str) 
        -> Result<String>;
    
    /// 发送带媒体的消息
    async fn send_media_message(&self, chat_id: &str, params: &SendMessageParams) 
        -> Result<SendResult>;
}
```

但实际上，考虑到 QQ 的 API 特性，更好的方式是**扩展 `SendResult`**：

```rust
/// 扩展 PlatformAdapter 的 send_message 能力
pub trait PlatformAdapter: Send + Sync + 'static {
    // ... existing ...
    
    /// 上传文件到平台，返回可在消息中引用的标识
    async fn upload_file(&self, chat_id: &str, file_path: &str) 
        -> Result<UploadResult>;
}

#[derive(Debug, Clone)]
pub struct UploadResult {
    /// 平台返回的文件 URL 或 ID
    pub file_id: String,
    /// 可直接引用的 URL
    pub url: String,
}
```

#### Step 3: 扩展 Agent 事件类型（`robit-agent/src/event.rs`）

```rust
pub enum AgentEvent {
    // ... existing ...
    
    /// Agent 想要发送媒体（通过工具读取的图片等）
    MediaRequest {
        file_path: String,
        description: String,
    },
}
```

或者更简单的方式：让 Agent 通过 `read` 工具读取图片时，将图片路径信息通过现有的 `ToolCallResult` 传递，然后在 `ChatbotFrontend` 中检测并处理。

**更简单：不需要改 Agent 事件。** 在 `ChatbotFrontend::flush_buffer()` 中，如果 Markdown 包含 `[Image: xxx]` 这样的输出（当前 markdown.rs 的转换结果），尝试在本地查找对应文件并上传。

#### Step 4: 更新 QQ 协议解析（`robit-qq/src/protocol.rs`）

```rust
/// QQ 消息事件中的附件
#[derive(Debug, Clone, Deserialize)]
pub struct QqAttachment {
    pub url: String,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

/// 扩展 MessageEvent
#[derive(Debug, Clone, Deserialize)]
pub struct MessageEvent {
    pub id: String,
    #[serde(default)]
    pub content: String,
    pub author: Author,
    #[serde(default, rename = "group_openid")]
    pub group_openid: Option<String>,
    /// 附件列表
    #[serde(default)]
    pub attachments: Vec<QqAttachment>,
}
```

#### Step 5: 更新 QQ 平台适配器（`robit-qq/src/platform.rs`）

- `build_platform_event()` 提取 attachments
- `send_message()` 支持发送带媒体的消息
- 新增 `upload_file()` 实现

#### Step 6: 更新 Markdown 清洗器（`robit-chatbot/src/markdown.rs`）

保留现有的 `[Image: alt]` 转换，但增加一个模式：如果图片 URL 是本地文件路径（以 `file://` 或绝对路径开头），标记为"可上传"状态。

或者更简单的做法：在 `ChatbotFrontend::flush_buffer()` 中做后处理——检测 `[Image: xxx]` 标记，如果 `xxx` 对应本地文件路径，则上传并替换为 QQ 图片 URL。

#### Step 7: 更新 `ChatbotFrontend`（`robit-chatbot/src/frontend.rs`）

在 `flush_buffer()` 后，检测并上传图片：
1. 分析 Markdown 输出中的图片引用
2. 如果引用的是本地文件（Agent 通过 `read` 工具读取的），上传到 QQ
3. 用 QQ 的图片 URL 替换原始引用
4. 使用 `msg_type=7` 发送带图片的消息

#### Step 8: 更新 `PlatformCaps` 和 `MarkdownFeatures`

```rust
// QQ 平台能力
impl PlatformCaps {
    pub fn qq() -> Self {
        Self {
            // ... existing ...
            supports_images: true,       // NEW
            supports_files: true,        // NEW
            max_image_size: 20 * 1024 * 1024, // 20MB
        }
    }
}
```

---

## 3. 实现步骤

### Phase A: 底层类型扩展（不影响现有行为）

1. **`robit-chatbot/src/adapter.rs`**:
   - 添加 `MediaAttachment` 结构体
   - `ChatMessage` 增加 `attachments: Vec<MediaAttachment>`
   - 添加 `UploadResult` 结构体
   - `PlatformAdapter` trait 增加 `upload_file()` 方法（默认返回 unimplemented）

2. **`robit-qq/src/protocol.rs`**:
   - 添加 `QqAttachment` 结构体
   - `MessageEvent` 增加 `attachments` 字段

### Phase B: 接收图片/文件

3. **`robit-qq/src/platform.rs`**:
   - `build_platform_event()` 提取 attachments
   - 更新测试

4. **`robit-agent/src/event.rs`**:
   - 在 `FrontendMessage::UserInput` 中传递附件信息
   
   或者更简单：在 `ChatMessage` 的 `text` 中附加附件描述（如 `[用户发送了图片: image_001.jpg]`），这样不需要改 Agent 协议。

### Phase C: 发送图片/文件

5. **`robit-qq/src/platform.rs`**:
   - 实现 `upload_file()`（调用 QQ 文件上传 API）
   - 扩展 `send_message()` 支持 `msg_type=7`（富媒体消息）

6. **`robit-chatbot/src/frontend.rs`**:
   - `flush_buffer()` 后处理图片引用
   - 上传本地图片文件到 QQ 平台
   - 使用富媒体消息发送

### Phase D: 配置和清理

7. **`robit-qq/src/platform.rs`** / **`robit-chatbot/src/adapter.rs`**:
   - 更新 `PlatformCaps::qq()` 增加媒体相关配置
   - 增加 `max_image_size` 等限制

8. **更新测试**

---

## 4. 关键设计决策

| # | 决策 | 理由 |
|---|------|------|
| 1 | 附件信息附加到 `ChatMessage.text` 而非修改 Agent 协议 | 避免改 `FrontendMessage` 枚举，Agent 天然理解文本描述 |
| 2 | 图片上传发生在 `ChatbotFrontend::flush_buffer()` | 与 Markdown 清洗在同一层，保持平台适配器职责单一 |
| 3 | 使用 QQ `msg_type=7`（富媒体消息）发图片 | 最直接的方式，比 Markdown 模板更简单可靠 |
| 4 | 不在 Agent 事件层添加媒体类型 | Agent 看到的始终是文本；媒体路径通过 Markdown 中的特殊标记传递 |
| 5 | `upload_file()` 添加到 `PlatformAdapter` trait | 每个平台的文件上传 API 不同，需要各自实现 |

---

## 5. 风险和限制

- **QQ 文件上传 API 限制**：需要确认具体 API 端点和限制（大小、格式）
- **图片下载**：接收到的 QQ 图片 URL 有时效性（通常几小时），需要及时下载
- **LLM 视觉能力**：要让 LLM "看懂"图片，需要模型支持 vision（如图片 URL 传入），或使用 OCR 等替代方案
- **大文件**：文件上传可能很慢，需要异步处理并给用户反馈
