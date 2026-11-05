# robit 需求文档 · 把模型「思考过程」带上 wire（thinking output）

- **来源**：子木 StockAgent · `docs/07-backlog.md` R7（R 系列第六项；上批 R2–R5 已随 robit 0.1.27 落地）
- **日期**：2026-11-05
- **读者**：robit 维护者（本文档随取随用，可直接搬进 robit 仓库评审）
- **影响面**：`robit-ai`（流式解析）→ `robit-agent`（事件枚举）→ `robit-web`（wire schema）；宿主前端（子木 ChatPanel 等）随后消费
- **性质**：框架侧能力补齐，不含任何宿主领域逻辑（协作原则见子木 ADR-0005：框架级缺口先改 robit，宿主不写绕过补丁）

## 1. 问题

思考型模型流式输出时，**思考文本与正文本同帧并行下发**；子木生产链路（zimu-server → robit-web → 前端）目前没有任何通道把它送到界面——用户只看到一段静默等待后正文突然出现。断点不在最上层，而在最底层的依赖库：

| 层 | 现状 | 思考文本的归宿 |
|---|---|---|
| provider SSE wire | OpenAI 兼容面惯例：思考增量放 `delta.reasoning_content`（正文仍在 `delta.content`）——Qwen（DashScope 兼容模式）、DeepSeek-R1、Kimi 等皆此形态 | ✅ 在线上 |
| async-openai 0.42（robit-ai 依赖，子木 Cargo.lock 锁 0.42.2） | 流式 delta 结构 `ChatCompletionStreamResponseDelta`（`src/types/chat/chat_.rs:1325`）只有 `content / function_call / tool_calls / role / refusal` 五个字段——**没有 `reasoning_content`，也没有 `#[serde(flatten)]` 未知字段兜底** | ❌ **serde 对未知字段默认静默忽略＝第一丢失点**（发生在进入 robit 自有代码之前） |
| robit-agent | `AgentEvent` 枚举（`crates/robit-agent/src/event.rs:50`）无 thinking 变体；事件流循环只读 `choice.delta.content`（`agent.rs:677`） | ❌ 从未被看见 |
| robit-web | `WebEvent` 八个变体（`crates/robit-web/src/event.rs:16`）无 thinking；`from_agent`（同文件 `:61`）无投影 | ❌ wire schema 没有 |
| 宿主前端 | 子木 ChatPanel `SseEvent`（`frontend/src/components/ChatPanel.tsx:14`）无此类 | ❌ 无米下锅 |

**旁证（一个已被实测捕捉的怪象）**：子木 R6（2026-11-02）观察到 qwen profile 会发来「空白 `text_delta` 帧」——那正是**携带思考文本的那批帧**：DashScope 兼容模式在思考阶段发 `"content": ""` + `reasoning_content: "…"` 同帧；思考文本被 async-openai 丢弃后，空 `content` 外壳继续上行成空白帧。换言之：**思考内容已经走到半程，缺的只是中间那个字段**。顺带说明请求侧现状——该 profile 在未显式设置的情况下就下发了思考增量（或以默认开启），实施时以实际网关验证为准。

## 2. 需求（按依赖顺序）

### R-A `robit-ai`：捕获 reasoning 增量

目标：让 `chat_stream` 的消费方拿到**类型安全的逐帧思考增量**（目前它们到不了调用方）。

设计选项（由 robit 拍板，验收只认行为）：

1. **等/推上游**：给 async-openai 提 PR（纯 additive 字段）。进度不可控，不建议单独押注，但值得先提。
2. **pin git fork（最小 diff）**：fork/patch `ChatCompletionStreamResponseDelta` 加 `#[serde(default)] pub reasoning_content: Option<String>`；上游合入后切回 registry。与「升级前先想两边」的版本实践兼容。
3. **raw SSE 旁路**：`chat_stream`（`crates/robit-ai/src/client.rs:318`）改为自读字节流 + SSE 行分帧，用 `serde_json::Value` 摘 `delta.reasoning_content`，其余字段仍委托 async-openai 类型反序列化。改动面最大、不依赖上游、且天然为将来更多非标准字段留了口子。

**请求侧配套**：需要一个控制思考开启的配置面——DashScope 兼容面为 body 字段 `enable_thinking: true`（各 provider 拼写有差异，建议开放 extra body 字段透传，或 `providers.<profile>.thinking = auto | on | off`，默认 `auto` ＝不发额外字段、维持现状）。注意 enable 字段同样受「typed request 装不下未知字段」约束——随本条选项一并解决。

**明确不在本轮范围**：OpenAI 官方 Responses-API 形态的 `reasoning` item、Anthropic 风格 thinking block / signature——robit-ai 当前消费面是 chat completions，将来碰再另提。

### R-B `robit-agent`：事件枚举加变体

- 新增 `AgentEvent::ThinkingDelta(String)`：与 `TextDelta` 同形态；在摘到 `delta.reasoning_content` 处发射（`agent.rs` 流循环内与读 `content` 同一处，`:677` 旁）。
- **顺序契约**：同一 assistant 段内，`ThinkingDelta` 可能先于、也可能与 `TextDelta` 交错到达；不强制另设结束标记——消费方在 `TurnComplete` 收口未闭思考段即可（若 CLI/GUI 参考实现需要更精确的段闭合，可讨论加 `ThinkingComplete`，可选项而非硬需求）。
- **历史策略**：思考文本**不写回** `session.history`（OpenAI 兼容面的多轮回放不需要；思考是展示性单向输出）。
- 现有 `Frontend` 实现（CliFrontend / GUI）：对 `ThinkingDelta` 忽略或渲染成弱样式皆可（robit 自定），保证枚举变更的编译兼容由 robit 在自家 workspace 内消化。

### R-C `robit-web`：wire schema

- 新增 `WebEvent::ThinkingDelta { text: String }`，序列化形态 `{"type":"thinking_delta","text":"…"}`；`from_agent` 加一条投影臂。
- 兼容性声明：对穷举 match 的宿主是**编译期**可见的 additive 变更（子木已预备：ChatPanel 加一个 case 即可消费）；对只认已知 type 的 JSON 消费方是向后兼容。
- **顺带收口 R6**：空白 `text_delta` 帧「wire 层过滤 or 文档化」与本次是同一处投影代码——建议随本改动一并拍板，一次了断。

## 3. 验收标准

1. `robit-ai`：mock provider（wiremock 级）注入两阶段帧——
   `data: {"choices":[{"index":0,"delta":{"content":"","reasoning_content":"思考甲"}}]}` →
   `data: {"choices":[{"index":0,"delta":{"content":"正文乙","reasoning_content":null}}]}`；
   断言消费方获得无损、按序的 reasoning 增量，且 `content` 解析行为不受影响（回归）。
2. `robit-agent`：注入 FakeLlm 走一轮，断言事件序列形如 `[ThinkingDelta…, TextDelta…, TurnComplete]`（构造 thinking-only 帧与正文-only 帧混合、含交错各一）。
3. `robit-web`：`from_agent` 投影单测 + `thinking_delta` 序列化 golden 值，写入其 schema 文档。
4. 人工端到端（子木配合回归）：robit 发版后，子木 zimu-agent-cli / zimu-server + 前端消费真实 qwen profile 思考增量；子木前端展示形态（默认折叠灰字 vs 展开流式区）另按子木产品拍板，不影响本验收。

## 4. 发布与消费流程（沿 R2–R5 惯例）

- robit 侧改动走 `feat/thinking-output` 基线分支；合并 / tag / crates.io 发布由 robit 拍板。
- 子木按惯例切 registry 版本消费，回归面：`cargo build` + backend 全量测试 + 前端 `tsc/build` + `npm test`。

## 5. 排查锚点（证据索引）

| 位置 | 内容 |
|---|---|
| 子木 `backend/Cargo.lock` | async-openai 0.42.2（source registry）；robit-ai/agent/web 0.1.27 |
| async-openai 0.42.3 `src/types/chat/chat_.rs:1325` | `ChatCompletionStreamResponseDelta` 字段清单（无 reasoning 字段、无 flatten） |
| robit `crates/robit-ai/src/client.rs:318` | `chat_stream`＝`create_stream` typed 直通 |
| robit `crates/robit-agent/src/event.rs:50` | `AgentEvent` 枚举全貌（无 thinking 变体） |
| robit `crates/robit-agent/src/agent.rs:677` | 流循环只读 `choice.delta.content` |
| robit `crates/robit-web/src/event.rs:16,61` | `WebEvent` 变体与 `from_agent` 投影 |
| 子木 `frontend/src/components/ChatPanel.tsx:14,348` | `SseEvent` 无 thinking；空白帧守卫注释（R6 现象） |
| 子木 `docs/07-backlog.md` R6 / R7 | 空白帧观察记录；本需求的 backlog 条目 |
