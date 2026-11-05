# robit 设计文档 · 思考过程输出（thinking output）落地 0.1.28

- **来源需求**：子木 StockAgent R7（`docs/robit-rfc-thinking-output.md`，评审结论见其影响分析）
- **日期**：2026-11-05
- **状态**：五个决策点已全部确认并按本文实施（分支 `feat/thinking-output`）；D1 经追加调查后定为**双路径并存**形态（见 §1，与初稿的差异已就地更新）
- **范围决策**：全链路打通 `reasoning_content`（robit-ai → robit-agent → robit-web），**渲染启用仅 robit-gui**；TUI / QQ Bot（robit-chatbot）本轮只补编译臂、不启用；同时满足子木 R-A / R-B / R-C 验收。
- **分支**：`feat/thinking-output`（沿 R2–R5 惯例，合并/tag/发布由 robit 拍板）

## 0. 已核验的关键事实（设计前提）

| 事实 | 证据 |
|---|---|
| async-openai 0.42.3 流式 delta 无 `reasoning_content`、无 flatten 兜底；`CreateChatCompletionRequest` 同样无 flatten（全文件唯一 flatten 在无关的 `ChatCompletionMessageListItem`） | 本地 registry `chat_.rs:1325` / `:1450` |
| 子木不直连 robit-ai `chat_stream`（其 `chat_stream` 是 axum 路由名），消费面 = robit-agent + robit-web | `StockAgent/backend/crates/zimu-server/src/main.rs` |
| `RetryConfig` 仅解析导出、未接线到任何 LLM 调用——旁路不使重试能力倒退 | grep 全仓无消费点 |
| 流内错误恢复的素材已按 **raw payload 字符串**分类（`extract_gateway_error` / `extract_provider_error` / moderation） | `crates/robit-ai/src/error.rs:70-110` |
| `eventsource-stream 0.2.3`、`reqwest 0.13.5` 已在 Cargo.lock（async-openai 传递依赖），转正为直接依赖零编译增量 | Cargo.lock:1306, :3970 |
| `AgentEvent` 穷举 match 全在 workspace 内：tui / gui / chatbot / examples / web 共 5 处 | `tui/app.rs:147`、`gui/frontend.rs:37`、`chatbot/frontend.rs:256`、`examples/robit-agent/main.rs:195`、`web/event.rs:61` |
| **上游最新 0.43.0 仍无 `reasoning_content`、chat 类型文件零 flatten**——缺口不是版本滞后，是上游只同步 OpenAI 官方 spec 的立场 | 0.43.0 crate 源码 `chat_.rs` 逐字段核对 |
| **上游政策性拒收**：PR #334/#335（2025-03）、PR #525（2026-02，`ChatThinking` 请求字段 + `reasoning_content` 响应字段，与本需求逐字对应）全部关闭未合并；维护者原话 "contributions for non-openai types … are out of scope" | github.com/64bit/async-openai PR #525 评论区 |
| crates.io 现成 fork `async-openai-thinking 0.41.5`（crow-cli）：delta 加了 `reasoning_content`，但无 `enable_thinking`、底座 0.41.3 低于本仓锁定的 0.42.3、76 次下载的个人维护面 | fork crate 源码逐字段核对 |
| 功能完备的替代库 genai 0.7.3（流式 reasoning + `with_extra_body` 透传）与 rig-core 0.46.0 均为**自有类型系统的框架级库**——替换等于重写 robit-ai 公开面并全仓迁移 `ChatCompletionRequestMessage` 引用（history/构造面/持久化全动），且子木侧构造 API breaking | 两库源码核对 + 本仓类型引用面盘点 |

## 1. 决策 D1（定案）：双路径并存——`auto` 走 async-openai 传统路径，`on`/`off` 走自建 SSE 旁路

初稿为「全量切自建旁路 + 上游 PR 并行提」；追加调查后定案为**按模型配置分流的双路径**，把新解析引擎的回归面从「所有流式流量」收缩到「显式 opt-in 的模型」。

**排除过程**（证据见 §0 事实表后四行）：

- **等/推上游（RFC 方案 1）死刑**：上游最新 0.43.0 仍无字段，且维护者对非官方 spec 字段政策性拒收（PR #525 与本需求逐字对应，2026-05 被以 out of scope 关闭）。「上游合入后旁路退役」的预期收益为零——旁路是 robit-ai 的**永久兼容层资产**，与其「适配 DeepSeek/QWen 等 OpenAI 兼容面」的 crate 定位职责归位。上游 PR 降为可选善意动作，不占排期。
- **pin git fork（RFC 方案 2）与 registry 发布互斥**：`cargo publish` 拒绝 git dependency；子木按惯例切 registry 版本消费（ADR-0005），fork 路线要么长期双维护独立 crate，要么让子木配 `[patch.crates-io]`（违背「宿主不写绕过补丁」协作原则）。
- **现成第三方 fork（`async-openai-thinking`）不采用**：能力半条腿（无 `enable_thinking`）、底座降级（0.41.3 < 本仓 0.42.3）、个人维护面过 API key 流量的供应链成本，对发布型框架不成立。
- **迁移 genai / rig-core 不采用**：字段能力完备但等于换类型地基——robit-ai 公开面重写 + 全仓 `ChatCompletionRequestMessage` 引用迁移（history/构造面/GUI 持久化全动）+ 子木构造 API breaking，为一个字段付整个 wire 层的迁移税。

**双路径语义**：`thinking = "auto"`（默认）→ async-openai `create_stream` 原样直通（请求与解析行为与旧版本字节级一致，reasoning 照旧被丢弃）；`thinking = "on"/"off"` → 自建 SSE 旁路（请求体注入 `enable_thinking`，响应侧捕获 `reasoning_content`）。两路返回统一的 `RobitChatStream`，agent 及以下各层无感知。

**已知边界**：「默认下发思考增量 + 拒绝未知 body 字段」的 provider（如 DeepSeek-R1 若属此类）在 `auto` 下捕获不到、在 `on/off` 下有 400 风险——将来出现真实 case 再加「捕获但不注入」的第四态（YAGNI）。

### 1.1 新模块 `crates/robit-ai/src/stream.rs`

```rust
/// 一个 SSE 帧的解析结果：标准字段委托 async-openai typed，非标准字段旁路摘取。
#[derive(Debug, Clone)]
pub struct StreamChunk {
    /// async-openai 的原生帧类型（其余字段语义与此前完全一致）。
    pub response: async_openai::types::chat::CreateChatCompletionStreamResponse,
    /// 首个 choice 的 `delta.reasoning_content`（如有）。
    pub reasoning: Option<String>,
}

// 兼容垫：使 chunk.usage / chunk.choices 等既有访问路径经 Deref 继续编译，
// 收窄对 robit-ai 外部 typed 消费者的破坏面。
impl std::ops::Deref for StreamChunk {
    type Target = CreateChatCompletionStreamResponse; ...
}

pub type RobitChatStream =
    std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<StreamChunk, LlmError>> + Send>>;
```

**逐帧解析策略（双轨委托）**：每帧先 `serde_json::from_value::<CreateChatCompletionStreamResponse>(v)`（标准字段行为等价于原 typed 直通，**回归面收敛在「我们不再信任 async-openai 的失败判定」上**），再从 `Value` 摘 `choices[0].delta.reasoning_content`。多 choice 场景与现状对齐（只消费 first）。

**帧管线**：`reqwest` 直接 POST `{base_url}/chat/completions`（`Authorization: Bearer`、`stream: true`），`bytes_stream()` → `eventsource-stream` 分帧（原生处理 `data:` 行、`event:` 行、`: keep-alive` 注释心跳、`[DONE]` 哨兵）。

**错误路径**（保持现有友好度不回退）：

| 场景 | 处理 |
|---|---|
| HTTP 非 2xx | 读 body → 新增 `LlmError::from_sse_payload(status, raw)`：复用 `extract_gateway_error` / `extract_provider_error` / `is_content_moderation_code` / `moderation_error`；未识别时落 `ServerError { status, message }`；401/403 → `AuthenticationError` |
| 流内 `{"error":{...}}` 帧（无 `choices`） | `from_sse_payload` 同一入口（现状即 DashScope 内容审核的来路，agent.rs:658-666 的恢复行为必须保留） |
| 帧 JSON 解析失败 | `LlmError::ParseError(raw 截断)`，不再依赖 async-openai 的 `JSONDeserialize` 包装 |

**request 侧**：`serde_json::to_value(CreateChatCompletionRequest)` 得基础 body（与今天 async-openai 发出的字节级一致），按 thinking 配置插入额外字段后发送。`stream_options.include_usage` 维持现状——usage 尾帧行为不变，agent.rs:671 的捕获逻辑不动。

### 1.2 配置面：`providers.<p>.models[].thinking`

```toml
[[providers.qwen.models]]
id = "qwen3-max"
thinking = "on"        # auto | on | off，默认 auto
```

- `config.rs`：`ModelConfig` 加 `thinking: Option<ThinkingMode>`（serde 小写枚举，缺省 `Auto`），`ResolvedModel` 加 `pub thinking: ThinkingMode`——与 `supports_images` / `supports_tools` 完全同构。
- 语义：`Auto` = **不发** `enable_thinking` 且走传统 typed 路径（维持现状，兼容 DeepSeek 等对未知字段报 400 的网关）；`On`/`Off` = 走 SSE 旁路并注入 `enable_thinking: true/false`（DashScope 兼容面拼写）。各 provider 拼写差异本轮只落这一种，后续有 case 再扩映射表。
- `client.rs` 的 `chat_stream` 签名不变（`(Vec<ChatCompletionRequestMessage>, Option<Vec<ChatCompletionTools>>)`），返回类型 `ChatCompletionResponseStream` → `RobitChatStream`；lib.rs:20 的 re-export 相应替换。旧 async-openai 类型仍经 re-export 可得（`StreamChunk.response` 的 Deref 目标）。

### 1.3 显式排除（本期不做，文档声明）

- 非流式 `chat()`（摘要压缩 / read_image 路径）同样丢 reasoning——不修，展示无关。
- OpenAI Responses-API `reasoning` item、Anthropic thinking block / signature——robit-ai 消费面是 chat completions，另议。
- `ChatCompletionMessage` 历史回放携带 reasoning——不需要（见 D2 历史策略）。

## 2. 决策 D2：robit-agent 事件与发射点

### 2.1 枚举与发射

`event.rs` 新增：

```rust
/// Streaming reasoning/thinking delta from LLM response (display-only,
/// never written back to session history).
ThinkingDelta(String),
```

`agent.rs` 流循环（`:675` 附近）改为消费 `StreamChunk`：

```rust
// 思考增量先发（同帧惯例：思考阶段 content 为空串）
if let Some(thinking) = &chunk.reasoning {
    if !thinking.is_empty() {
        let _ = self.frontend
            .on_event(AgentEvent::ThinkingDelta(thinking.clone())).await;
    }
}
if let Some(content) = &choice.delta.content {
    if !content.is_empty() {          // ← R6 收口：空 content 不再发空白 TextDelta
        full_text.push_str(content);
        let _ = self.frontend.on_event(AgentEvent::TextDelta(content.clone())).await;
    }
}
```

- **R6 空白帧在发射端收口**（而非 RFC 建议的 web 投影层）：一处修改，TUI/GUI/chatbot/web 全前端受益；`full_text` 累积语义不变（`push_str("")` 本就无害）。子木前端已有的空白帧守卫保持兼容（守卫变为惰性）。
- **顺序契约**：沿 RFC——不设 `ThinkingComplete`，消费方在 `TurnComplete` 收口；GUI 渲染层同时以「首个 TextDelta / ToolCallRequested」作段闭合信号（见 D4）。
- **历史策略**：thinking 不写 `session.history`、不计入 `full_text`、不进 token 本地估算（provider usage 校准已覆盖实际开销）。
- Err 分支简化：流 Item 已是 `Result<_, LlmError>`，agent.rs:663 的 `from_openai_error` 包装移除。

### 2.2 测试缝（与子木验收 2 的对齐结果）

`Agent` 依赖具体类型 `Arc<LlmClient>`（agent.rs:114），无 trait 抽象；本期**不**为测试引入 `Llm` trait（改动超出需求）。替代方案：把 chunk→(thinking, text) 的投影抽成纯函数 `chunk_deltas(chunk: &StreamChunk) -> (Option<&str>, Option<&str>)`，事件序列断言改为「纯函数单测（含交错帧矩阵）+ robit-ai wiremock 全链路 + robit-web golden」三层覆盖。**验收标准等价、实现形态变化，随 PR 说明知会子木。**

### 2.3 其余 match 臂补齐（全部不启用渲染）

| 位置 | 处理 |
|---|---|
| `robit-tui/src/app.rs:147` | `AgentEvent::ThinkingDelta(_) => {}` + 注释「暂不启用，后续以弱样式渲染」 |
| `robit-chatbot/src/frontend.rs:256` | 显式忽略 + `tracing::trace`（QQ 消息长度限制，思考文本会撑爆分片转发） |
| `examples/robit-agent/main.rs:195` | 忽略臂 |
| `robit-gui/src/frontend.rs:37` | **启用**：映射 `UiEvent::ThinkingDelta`（见 D4） |

## 3. 决策 D3：robit-web wire schema

`WebEvent` 新增变体（放 `TextDelta` 之后保持可读顺序）：

```rust
/// Streaming reasoning/thinking delta (display-only additive frame).
ThinkingDelta { text: String },
```

- `from_agent` 投影臂；序列化形态 `{"type":"thinking_delta","text":"…"}`。
- 兼容性声明沿 RFC：穷举 match 宿主编译期可见；只认已知 type 的 JSON 消费方向后兼容。
- 测试：`every_agent_event_variant_projects` 增例 + `thinking_delta` golden 值。
- 空白 `text_delta`：发射端已收口，投影层**不加**二次过滤（wire 层保持直通语义的纯洁性）。
- `docs/protocol.md` 增补 `thinking_delta` 帧定义。

## 4. 决策 D4：robit-gui 完整渲染（唯一启用面）

### 4.1 Rust 侧

- `events.rs`：`UiEvent::ThinkingDelta { session_id: String, delta: String }`。
- `frontend.rs`：映射臂。
- `state.rs` 中继循环：与 TextDelta 同策略独立缓冲批处理（50ms，`thinking_buffer` 与 `delta_buffer` 各自 flush；两者跨类型顺序罕见交错，批间乱序可接受——思考段通常先于正文段整体到达）；`ToolCallRequested` 前同时 flush 两个 buffer。
- **持久化决策**：thinking **不写 SQLite**。MessageData 无新列、DB schema 无迁移；思考文本是瞬态展示态，会话重开即消失（与「不写回 history」的框架级语义一致，UI 侧同样单向）。

### 4.2 React 侧

| 文件 | 改动 |
|---|---|
| `lib/types.ts` | UiEvent 联合加 `ThinkingDelta` |
| `lib/store.ts` | `thinkingBuffer: Record<string,string>`；`appendThinking` / `clearThinking`；`commitStreaming` 只提交正文（thinking 不入 messages）；`TurnComplete` / `Error` 时保留 buffer 供回看直至下次输入（见组件状态机） |
| `App.tsx` | `case "ThinkingDelta": appendThinking(sid, payload.delta)` |
| `components/MessageList.tsx` | 流式区改造（见下） |
| `components/ThinkingPanel.tsx`（新增） | 折叠容器（见下） |

**ThinkingPanel 状态机**（挂在流式 assistant 气泡上方）：

- `thinking`：仅 thinkingBuffer 增长、无正文 → 展开流式渲染灰字思考文本，替代现有 `ThinkingIndicator` 空转圈（showThinkingIndicator 条件加「无 thinking」）；
- `writing`：首个 TextDelta 到达 → thinking 自动折叠为头行「▸ 思考过程 · 进行中」，正文照常流式；
- `done`：TurnComplete → 头行定稿「▸ 思考过程」，点击展开/折叠可回看本轮 thinking（直到下一条用户输入清空）。

渲染细节：`text-muted-foreground`、等宽感 `text-xs`、`whitespace-pre-wrap`、`max-h-64 overflow-y-auto` 自动滚到尾部；markdown 不做渲染（思考文本按纯文本处理，避免半截语法高亮抖动）。memo 比较加 `thinkingContent` prop。

## 5. 测试设计（对应 RFC §3 验收逐条替换）

| 层 | 内容 | 工具 |
|---|---|---|
| robit-ai | ① RFC 验收 1 原样落地：mock 两阶段帧，断言 `reasoning` 无损按序 + `content` 回归；② `reasoning_content: null` / 缺失 / 空串 → `None`/不过发；③ `[DONE]`、usage 尾帧、`: keep-alive` 心跳、`event:` 行；④ 流内 `{"error":{...}}` → ContentModeration 映射（对齐 error.rs 现有测试语料）；⑤ HTTP 400/413-HTML → ProviderError/GatewayError；⑥ `thinking = auto/on/off` 三态下 body 是否含 `enable_thinking`；⑦ Deref 兼容：既有 typed 字段访问形态编译通过 | dev-dep `wiremock`（workspace 新增，dev-only）+ `tokio::test` |
| robit-agent | `chunk_deltas` 纯函数矩阵：thinking-only / 正文-only / 同帧混合 / 交错各一 → 断言发射序 `[ThinkingDelta…, TextDelta…, TurnComplete]`；空 `content` 不发 TextDelta（R6 回归） | 现有 `#[cfg(test)]` 体系 |
| robit-web | `from_agent` 投影 + `thinking_delta` golden | 扩展 event.rs tests |
| robit-gui | `cargo check` + React `tsc/build` + 手动冒烟（qwen profile 实网关开 thinking=on） | — |
| 全仓 | `cargo check --workspace` + 各 crate 测试；TUI/chatbot 验证「忽略臂不破坏现有渲染」冒烟 | CI |

## 6. 风险与回归面

| 风险 | 缓解 |
|---|---|
| **旁路回归面**（双路径定案后收缩为 opt-in 模型）：`thinking = on/off` 的模型流式路径换解析引擎；`auto` 默认路径字节级不变 | typed 委托保证字段语义等价；wiremock 用例矩阵覆盖非标帧形态 + `auto` 传统路径不变性断言；发版前 TUI + GUI 对 deepseek（auto）/qwen（on）双 profile 手动冒烟 |
| 网关对 `enable_thinking` 报 400（DeepSeek 系） | 默认 `Auto` 不发字段；按 model 粒度 opt-in；llm-config.md 标注生效面「DashScope 兼容面实测」 |
| 子木 qwen profile 默认是否下发思考增量 | RFC §1 已注「以实际网关验证为准」——验收 4 人工回归阶段确认，若默认已开则 `Auto` 即可吃到，`on` 只是显式保险 |
| token 估算不含 thinking → 本地估算偏低 | usage 校准机制（`last_known_prompt_tokens`）已覆盖；估算仅 fallback，风险低 |
| 直连 robit-ai `chat_stream` 的理论外部用户（已确认子木不在内） | Deref 垫兼容；返回类型变化在 CHANGELOG 显式标注 breaking surface |
| GUI 快速多轮会话 thinking 串台 | buffer 全部按 sessionId 隔离，沿用现有模式 |

## 7. 实施顺序（PR 内提交切分）

1. **robit-ai**：`stream.rs` + config/`ThinkingMode` + `from_sse_payload` + wiremock 测试；examples/robit-chat 适配。
2. **robit-agent**：`AgentEvent::ThinkingDelta` + 发射/收口 + `chunk_deltas` 测试 + tui/chatbot/examples 忽略臂。
3. **robit-web**：`WebEvent::ThinkingDelta` + 投影 + golden。
4. **robit-gui**：UiEvent 映射 + state 批处理 + React 四件套 + ThinkingPanel。
5. **文档/发版**：`docs/protocol.md`、`docs/llm-config.md`、`docs/architecture.md`（事件表 + 旁路说明）、CLAUDE.md（config 示例、robit-ai 依赖行）、CHANGELOG `[Unreleased]` → `0.1.28`；三处版本号 + `cargo check` 同步 lock。

## 8. 交付给子木的消费信息（随 release notes）

- wire 新帧：`{"type":"thinking_delta","text":"…"}`——ChatPanel 加一个 case 即消费（R-B/R-C 验收达成）。
- 空白 `text_delta` 帧自 0.1.28 起不再下发（发射端收口），前端守卫可移除但保留亦无害（R6 了断）。
- 验收 2 的实现形态按 §5 等价替换（纯函数 + wiremock + golden），行为断言不变。
- 配置：模型级 `thinking = "on"` 显式开启 DashScope 兼容面思考增量；`auto` 下 qwen 若默认已下发亦可直接消费。
- GUI（robit 自家 Tauri 应用）为参考实现：默认折叠灰字 + 流式展开，可对照子木 ChatPanel 的产品形态拍板。
