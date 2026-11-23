# robit 设计文档 · Bot 忙碌状态信号（typing）统一抽象与微信落地

- **日期**：2026-11-23（**v2**，替代同日 v1）
- **修订依据**：维护者指示——**以 W-D（上游 SDK 暴露 client 级 typing API）为最终目标设计**；必要时可做架构/API 层重构，允许适度破坏兼容性。v1 的"W-C 内置协议客户端起步"路线据此**废弃**，robit 内不保留任何 typing 协议重复代码。
- **版本基准**：robit **0.5.1**（`main` @ `d9ea180`）——文中行号均按此基线核实；协议 SDK 为 crates.io 依赖 `weixin-agent` **0.3.0**
- **影响面**：`weixin-agent`（上游 API 增补，0.4.0）、`robit-chatbot`（adapter.rs / frontend.rs / manager.rs，含 `PlatformCaps` 一次性破坏重构）、`robit-wechat`（platform.rs）、`robit-ai`（config.rs）；QQ / 飞书零行为变化
- **状态**：方向已确认（W-D 终态 + 接受 `PlatformCaps` 破坏重构，2026-11-23 维护者拍板）；**上游路径待定**——weixin-agent 为**第三方库**（`github.com/spensercai/weixin-agent-sdk-rs`，作者 spensercai，非 robit 维护者所有），路径 A 不适用，B/C 取舍待维护者判断（§7.1）
- **实施进度**（2026-11-23）：路径 B 已确认并落地中——①chatbot 层抽象已提交 main（`31ff5fa`，132 测试通过）；②上游 delta 已实现并本地验证（克隆 `E:\GitHub\weixin-agent-sdk-rs` 分支 `typing-client-api` @ `76e9b2c`，待用户 fork+PR，提交件见 `docs/upstream-pr-weixin-agent-typing.md`）；③微信实现位于 `feat/wechat-typing` 分支（`[patch.crates-io]` 指向本地克隆，合 main 门禁见 §7.3）

## 1. 背景与问题

### 1.1 现状

用户给 Bot 发消息后，在答案到达前看不到任何"对方在干活"的信号：

- 微信：`progress_hint` 默认 `off`（`adapter.rs:179`），且微信无原地编辑，提示是独立消息、占反刷配额（R14），不宜常开。
- QQ：靠"增长消息"（首条消息原地编辑成答案，`frontend.rs:266-299`）+ `HintPolicy::Always` 表达忙碌，但首条消息发出前（LLM 首 token 延迟、工具确认等待）仍是空窗。
- 模型思考过程（`ThinkingDelta`）Bot 端刻意不渲染（`frontend.rs:416-425`），只计数记日志。

### 1.2 平台能力差异（事实核查）

| 平台 | 原生瞬时"输入状态" | 协议证据 |
|---|---|---|
| 微信 iLink | **有**：`ilink/bot/sendtyping`（Typing/Cancel），不产生聊天消息 | SDK `messaging/inbound.rs:141-171`、`api/client.rs:265-275` |
| QQ 官方 Bot | **无**（"正在输入"是真人客户端间能力，不对 bot 开放） | QQ 开放平台 API 无对应端点 |
| 飞书（计划） | **无** typing API；将来可用流式卡片 | — |

另核实：iLink 的工具进度 item（类型 11/12）服务端接受但**微信客户端不渲染**（SDK `messaging/outbound_run.rs:28-42`，2026-08-13 生产实测五种线格式变体均无效）。微信面当前唯一可用的原生忙碌显示就是 typing。

### 1.3 关键约束：SDK 0.3.0 的 typing 公开面极窄（W-D 的动机）

| SDK 项 | 可见性 | 后果 |
|---|---|---|
| `MessageContext::send_typing()` / `cancel_typing()` | **pub**，但只在 `on_message(ctx)` 回调内可用 | 唯一公开 typing 路径 |
| `MessageContext` 本体 | 无 `Clone`；`sender` 字段 `pub(crate)`（`inbound.rs:91`） | 无法把句柄带出回调、无法外部构造 |
| `WeixinClient` | pub，但**无** typing 方法（`client.rs:20-119`） | 回调外无法经 client 调 typing |
| `HttpApiClient` / `ConfigCache` / `GetConfigRequest` | `pub(crate)`（`api/client.rs:85` 等） | 无法直接复用底层 API 客户端 |
| `MessageSender` 字段与方法 | 全 `pub(crate)`（`messaging/sender.rs`） | `OutboundRun` 也带不出 typing 能力 |

**结论**：轮次全生命周期（点亮/刷新/取消）驱动 typing，必须让 SDK 在 client 级暴露该能力。这就是 W-D——**协议代码始终留在 SDK 层**，robit 只做策略与生命周期。v1 曾以"robit 内置最小协议客户端（W-C）"绕过此约束，现按维护者指示废弃：不在 robit 内制造协议重复。

## 2. 语义模型与总体架构

### 2.1 抽象对象：忙碌状态，不是 typing 功能

统一抽象的是**语义状态**："本聊天有一轮 agent 工作正在进行"。typing 只是该状态在微信的原生渲染；QQ 的增长消息、无原生能力平台的文字 hint 是同一语义的其他渲染。因此：

- **生命周期归 chatbot 层**：只有 manager/frontend 看得见轮次边界（注入成功、TurnComplete、Error、确认等待）。
- **渲染归平台 adapter**：微信委托 SDK client 级 typing（刷新节奏等策略仍在 robit）；QQ/飞书默认 no-op，**不承诺人人能显示 typing**。
- **best-effort 契约**：typing 是可选信号，任何失败只记日志，绝不影响答案投递；adapter 实现必须**立即返回**（内部 spawn），不得让 HTTP/节流排队阻塞 `on_event`。

### 2.2 架构分层（终态）

```
ChatbotManager                     ChatbotFrontend (per chat)
  注入成功 ──notify_turn_started──▶  typing_on 期望态去重
                                      │ on_event: busy 置位臂 re-assert(true)
                                      │ TurnComplete / Error ──▶ (false)
                                      │ request_tool_confirmation:
                                      │   手动模式等待前 (false) / 决议后 (true)
                                      ▼
                              PlatformSender::set_typing(chat_id, active)
                                      │ PlatformSenderBridge 显式委托
                                      ▼
                              PlatformAdapter::set_typing   ← 默认 no-op
                                      │
                    ┌─────────────────┼──────────────────┐
                 QQ/飞书            微信 adapter
                 (no-op)        per-chat 状态格 + 刷新任务（策略）
                                      │ TypingTransport 缝（可测试）
                                      ▼
                              WeixinClient::send_typing / cancel_typing   ← W-D 新增
                                      │ SDK 内部：ConfigCache(ticket) + sendtyping
                                      ▼
                              ilink/bot/sendtyping（协议始终在 SDK）
```

与现有设计哲学同构：caps 声明能力（`supports_edit` 先例）、trait 可选方法带默认实现（`edit_message` / `upload_file` 先例）、per-channel 策略覆盖（`hint_policy()` 先例）、frontend 拥有轮次状态（`busy` / `progress_hint_sent` 先例）。

## 3. chatbot 层 API 设计

### 3.1 能力声明与 PlatformCaps 重构（adapter.rs）

`PlatformCaps`（:33-51）增加字段，并**借本次破坏窗口做一次性加固**（维护者已允许适度破坏兼容）：

```rust
#[derive(Debug, Clone)]
#[non_exhaustive]                       // 新增：此后增补能力字段不再破坏下游
pub struct PlatformCaps {
    // ...既有字段不变...
    /// Native ephemeral "busy/typing" status (e.g. WeChat iLink sendtyping).
    /// Descriptive capability; the per-channel policy is `typing_enabled()`.
    pub supports_typing: bool,
}

impl Default for PlatformCaps {
    /// 保守基线：纯文本、无编辑、无媒体、提示关闭。
    fn default() -> Self {
        Self {
            supports_edit: false,
            returns_msg_id: false,
            supports_markdown: false,
            markdown_features: MarkdownFeatures::default(),
            max_message_length: 2000,
            supports_images: false,
            supports_files: false,
            max_upload_size: 0,
            progress_hints: HintPolicy::Off,
            supports_typing: false,
        }
    }
}
```

- 取值：`qq()`（:133-147）/`feishu()`（:150-162）`supports_typing: false`；`wechat()`（:167-181）`true`。三个构造器改写为 `Self { ..., ..Default::default() }` 结构更新语法。
- **破坏面（已逐点核实）**：workspace 内字面量构造仅 7 处测试代码（`frontend.rs:573/819/867`、`confirmer.rs:262`、`outbound.rs:273/287/303`），同 PR 改为 `Default` + 字段覆写；`robit-qq`/`robit-wechat` 均经构造器（`qq()`/`wechat()`），不受影响。**下游自定义 adapter** 若字面量构造 `PlatformCaps` 会一次性编译失败，迁移见 §8。
- 收益：caps 是持续生长的能力面，`#[non_exhaustive]` + `Default` 让**未来每次能力增补都不再是破坏变更**——本次是最后的破坏窗口。

### 3.2 平台钩子（adapter.rs，PlatformAdapter trait :280-391）

```rust
/// Signal the chat's busy state. Idempotent; best-effort.
///
/// Contract:
/// - Must return promptly: spawn internal work, never block on HTTP or
///   send-budget queues (callers are on the turn's critical path).
/// - `active=true`: show the native busy indicator (adapter owns refresh).
///   `active=false`: cancel it. Repeated same-value calls are no-ops.
/// - Errors are logged by the adapter, never propagated as turn failures.
///
/// Default: no-op — platforms without a native indicator keep their
/// existing behavior exactly (QQ's growing message + hints are untouched).
async fn set_typing(&self, _chat_id: &str, _active: bool) -> Result<()> {
    Ok(())
}

/// Per-channel typing policy. Defaults to the capability declaration;
/// adapters with a config override (WeChat) override this method.
/// Mirrors the `hint_policy()` pattern (:345-347).
fn typing_enabled(&self) -> bool {
    Self::capabilities().supports_typing
}
```

带默认实现，对现有 trait 实现者**非破坏**。

### 3.3 发送桥（frontend.rs PlatformSender :51-74；manager.rs Bridge :61-105）

`PlatformSender` 增加同签名默认 no-op 方法 `set_typing`。**Bridge 必须显式委托**（同 `send_best_effort` 的注释先例 :71-79——默认实现会吞掉平台实现）：

```rust
async fn set_typing(&self, chat_id: &str, active: bool) -> Result<()> {
    self.platform.set_typing(chat_id, active).await
}
```

### 3.4 前端状态机（frontend.rs ChatbotFrontend）

新增字段与构造：

```rust
/// Resolved typing policy (manager wires `platform.typing_enabled()`).
pub typing_enabled: bool,                 // new() 默认 false
/// Desired typing state, dedupes hook calls (events re-assert constantly).
typing_on: Mutex<bool>,                   // tokio Mutex，跨 send 持锁防并发乱序

pub fn with_typing_enabled(mut self, enabled: bool) -> Self { ... }
```

核心 helper（唯一调用钩子的地方）：

```rust
/// Transition the desired typing state; calls the platform hook only on
/// change. Never fails the turn: hook errors are logged and swallowed.
async fn set_typing_state(&self, on: bool) {
    if !self.typing_enabled { return; }
    let mut cur = self.typing_on.lock().await;
    if *cur == on { return; }
    *cur = on;
    if let Err(e) = self.platform_sender.set_typing(&self.chat_id, on).await {
        tracing::warn!("[chatbot] set_typing({}) failed: chat_id='{}', error={}",
            on, self.chat_id, e);
    }
}

/// Manager calls this right after a user turn was successfully injected
/// (earliest correct point — covers LLM first-token latency).
pub async fn notify_turn_started(&self) {
    self.set_typing_state(true).await;
}
```

调用点（全部在既有挂点上追加，不改变原有逻辑）：

| # | 位置 | 动作 | 说明 |
|---|---|---|---|
| F1 | `on_event` TextDelta / ThinkingDelta / ToolCallRequested 臂（:413/:417/:427，`busy=true` 处） | `set_typing_state(true)` | re-assert：确认暂停后首个事件自动复亮；去重闸保证每轮只真正调一次 |
| F2 | `on_event` TurnComplete 臂（:476，`busy=false` 处） | `set_typing_state(false)` | 与 flush 同簇；flush 若被节流排队，typing 保持点亮（答案仍在路上，语义正确） |
| F3 | `on_event` Error 臂（:485，`busy=false` 处） | `set_typing_state(false)` | 出错必灭 |
| F4 | `request_tool_confirmation`（:517-521） | 手动模式：`confirmer.request` 前 `(false)`、返回后 `(true)`；auto_approve 不暂停 | 等用户的 60s 里 agent 实际闲着，不应显示"正在输入"；决议后无论批准/拒绝/超时都复亮，随后 F2/F3 收口 |

F4 包裹后：

```rust
async fn request_tool_confirmation(&self, info: &ToolCallInfo) -> Result<bool> {
    if !self.auto_approve {
        self.set_typing_state(false).await;
        let decision = self.confirmer.request(&self.chat_id, info, self.auto_approve).await;
        self.set_typing_state(true).await;
        decision
    } else {
        self.confirmer.request(&self.chat_id, info, self.auto_approve).await
    }
}
```

**Confirmer 本体零改动**——暂停/复亮完全由 frontend 包裹实现。

### 3.5 manager 接线（manager.rs）

| # | 位置 | 动作 |
|---|---|---|
| M1 | `spawn_session_agent`（:808-816，`with_hint_policy` 旁） | `.with_typing_enabled(self.platform.typing_enabled())` |
| M2 | `route_user_input` 正常注入成功（:427-429，`tx.send` ok 后、save 前） | `frontend.notify_turn_started().await` |
| M3 | `route_user_input` 死句柄重生注入成功（:440-443） | 同上 |
| M4 | `inject_message` 两分支汇合后（:496，save 前） | 同上（宿主注入轮同等待遇） |

`/stop` **不需要独立挂点**：`FrontendMessage::Cancel` 只取消异步后台任务（`robit-agent/src/agent.rs:421-424`），同步轮次仍跑到自然收口并发 TurnComplete → F2 灭灯。typing 跟随**真实**轮次结束，比"停止信号已发送"更诚实。

## 4. 生命周期状态机与边界情形

```
                 注入成功 (M2/M3/M4)
      Idle ─────────────────────────────▶ Busy [typing on]
       ▲                                    │
       │ TurnComplete / Error (F2/F3)       │ 手动确认请求 (F4)
       ├────────────────────────────────────┤
       │                                    ▼
       │                             AwaitingConfirm [typing off]
       │                                    │ 决议返回（批准/拒绝/超时）(F4)
       │                                    ▼
       └──────────────────────────── Busy [typing on]（F1 亦会 re-assert）
```

| 情形 | 行为 | 依据 |
|---|---|---|
| LLM 首 token 延迟 | 注入成功即点亮，不等首个事件 | M2-M4 早于 F1 |
| 多工具长轮次 | 持续点亮；adapter 内部刷新 | §6.3 |
| 手动确认等待 | 灭灯；决议后复亮 | F4 |
| auto_approve | 不暂停（confirmer 立即返回，避免 cancel/send 抖动） | F4 守卫 |
| 中途出错 | Error 臂灭灯 + 错误通知照发 | F3 |
| `/stop`（同步轮次进行中） | 轮次自然收口后灭灯 | §3.5 说明 |
| `/stop`（空闲） | typing 本就 off，无动作 | 状态机 |
| 死句柄重生 | 新 frontend 期望态全新；adapter per-chat 状态格幂等，重复 true 不重起任务 | §6.3 |
| 会话空闲驱逐 | busy 会话不被驱逐（`manager.rs:1032`）；驱逐必发生在灭灯后 | 既有守卫 |
| 进程关停 | adapter shutdown token 级联取消全部刷新任务；drain 宽限期内收口的轮次正常走 F2 | §6.3 |
| 宿主直推（绕过 manager 直调 `adapter.send_message`） | 无 typing（瞬时发送，非 agent 轮次） | 非目标 §13 |
| 空答案轮次（buffer 为空，flush 不发） | F2 仍执行灭灯——不依赖"有回复才清 typing" | F2 与 flush 解耦 |

## 5. 上游 SDK API 设计（W-D 目标 · weixin-agent 0.4.0）

本节是可直接执行的上游变更规格。**设计原则：SDK 保持纯协议层**（其 crate 文档自述）——只暴露既有能力的 client 级入口，不引入刷新/轮次等集成策略；ticket 管理沿用内部 `ConfigCache`。

### 5.1 公开 API 增补

```rust
impl WeixinClient {
    /// Send a typing indicator to a user.
    ///
    /// The typing ticket is fetched/cached internally (per-user ConfigCache,
    /// 24h TTL + jitter + backoff) — callers never manage tickets.
    /// Best-effort semantics are the caller's policy; this method returns
    /// the raw API result.
    pub async fn send_typing(&self, to: &str, context_token: Option<&str>) -> Result<()>;

    /// Cancel the typing indicator for a user.
    pub async fn cancel_typing(&self, to: &str, context_token: Option<&str>) -> Result<()>;
}
```

签名与 `MessageContext::send_typing/cancel_typing` 的语义对齐（`to` = 对方 ilink_user_id，`context_token` 可选关联），调用方从回调内句柄换成 client 即可，心智模型不变。

### 5.2 内部实现：共享 helper，消除双份

现状 `MessageContext::send_typing/cancel_typing`（`inbound.rs:141-171`）各自内联构造请求。增补时顺手收敛为单一实现：

```rust
// messaging/sender.rs
impl MessageSender {
    pub(crate) async fn set_typing(
        &self, to: &str, context_token: Option<&str>, status: TypingStatus,
    ) -> Result<()> {
        let ticket = self.config_cache.get_typing_ticket(to, context_token).await;
        let req = SendTypingRequest {
            ilink_user_id: to.to_owned(),
            typing_ticket: ticket,
            status,
            base_info: self.api.base_info(),
        };
        self.api.send_typing(&req).await
    }
}

// MessageContext::send_typing  → self.sender.set_typing(&self.from, ctx_token, TypingStatus::Typing)
// MessageContext::cancel_typing → ... TypingStatus::Cancel
// WeixinClient::send_typing     → self.sender.set_typing(to, context_token, TypingStatus::Typing)
// WeixinClient::cancel_typing   → ... TypingStatus::Cancel
```

净变更约 40–60 行（含文档注释），**零新协议代码**——全部委托既有 `ConfigCache` + `HttpApiClient::send_typing` 路径。`OutboundRun` 不加 typing（无需求，保持最小公开面）。

### 5.3 上游测试与示例

- 单测：`set_typing` 请求构造（status 线格式 `typing`/`cancel`、ticket 注入、base_info 一致）；`WeixinClient` 两方法委托断言（mock API 层）。
- e2e 示例：`examples/e2e_test.rs` 的 `typing` 命令（:184-188）增补 client 级调用变体，供 robit 真机探针（§11）复用。
- CHANGELOG：明记新增 API 与动机（回调外生命周期驱动）。

### 5.4 版本与依赖

- 上游：纯增补 → **0.4.0**（0.x 阶段 minor 即功能位）。
- robit：workspace `Cargo.toml:46` `weixin-agent = "0.3"` → `"0.4"`；`Cargo.lock` 同步。对 robit 既有源码**零破坏**（上游只增不改）。
- 发布顺序约束见 §7.3（crates.io 门禁）。

## 6. 微信侧终态实现（robit-wechat）

**终态没有 typing.rs 协议模块**——v1 的 W-C 设计整体删除。adapter 只做三件事：配置解析、per-chat 状态格与刷新策略、委托 SDK。

### 6.1 配置面（robit-ai config.rs WechatBotConfig :319-337）

```rust
/// Native typing indicator switch. Default false until real-device
/// verification (§11) completes; flip the default in a later release.
#[serde(default)]
pub typing: Option<bool>,
/// Typing refresh interval in seconds (default 5; 0 = no refresh).
/// Tune per §11 probe results.
#[serde(default)]
pub typing_refresh_secs: Option<u64>,
```

`WechatConfig`（platform.rs:54-67）增 `typing: bool`（解析默认 `false`）与 `typing_refresh: Duration`（默认 5s）。adapter 覆写 `typing_enabled()` 返回 `self.typing`（同 `hint_policy()` 覆写先例 :669-672）。caps 静态声明 `supports_typing=true` 与配置开关分离：**能力是事实，开关是策略**。新增 serde 可选字段，非破坏。

### 6.2 委托缝：TypingTransport（可测试性）

`WeixinClient` 是具体类型，为让状态格逻辑离线可测，adapter 内部引入最小传输缝：

```rust
#[async_trait]
trait TypingTransport: Send + Sync {
    async fn typing(&self, peer: &str, context_token: Option<&str>) -> weixin_agent::Result<()>;
    async fn cancel(&self, peer: &str, context_token: Option<&str>) -> weixin_agent::Result<()>;
}

/// 生产实现：纯委托，无协议代码。
struct SdkTyping(Arc<WeixinClient>);
#[async_trait]
impl TypingTransport for SdkTyping {
    async fn typing(&self, peer: &str, token: Option<&str>) -> weixin_agent::Result<()> {
        self.0.send_typing(peer, token).await
    }
    async fn cancel(&self, peer: &str, token: Option<&str>) -> weixin_agent::Result<()> {
        self.0.cancel_typing(peer, token).await
    }
}
```

adapter 持 `Arc<dyn TypingTransport>`（`typing=false` 时不构建）；测试注入记录型 mock。此缝是 robit 内部私有 trait，不进公开 API。

### 6.3 状态格与刷新任务（platform.rs）

```rust
struct TypingCell {
    desired: bool,                  // 期望态（幂等闸）
    task: Option<JoinHandle<()>>,   // 点亮期间的刷新循环
    cancel: CancellationToken,      // shutdown 的子 token
}
// WechatPlatformAdapter 新增：
typing_cells: Mutex<HashMap<String, TypingCell>>,  // key = chat_id
typing: Option<Arc<dyn TypingTransport>>,
typing_refresh: Duration,
```

`set_typing(chat_id, active)` 实现（**立即返回**，HTTP 全在任务内）：

- `true`：`desired=true`；已有存活任务 → no-op（幂等，覆盖死句柄重生）；否则 spawn 子 token 任务：
  ```text
  loop {
      transport.typing(peer, outbound_context_token(peer) 现值)
      连续失败计数：≥3 → warn 一次并退出（熔断降级为无 typing，下次 true 重置）
      sleep(refresh_interval)；refresh=0 时发一次即驻留等待取消（不刷新）
      被 cancel → 退出
  }
  ```
- `false`：`desired=false`；取消任务；spawn 一次性 `transport.cancel`（best-effort，失败仅日志）。inactive 时重复 false → no-op（不刷 Cancel）。
- 关停：adapter `shutdown` token 级联取消全部子任务；**不**在关停路径阻塞发 Cancel（进程将退出，客户端状态自行过期；drain 宽限期内自然收口的轮次走正常 false 分支）。
- `typing=false`（配置关）：`set_typing` 直接 no-op（防御纵深，frontend 侧已有闸）。

每次发送取**当时**的 `outbound_context_token(peer)`（platform.rs:279 既有 helper），与回复的 token 关联策略一致；token 缺失照发（SDK 同行为）。ticket 获取/缓存/续期**全部在 SDK 内部**，robit 零感知。

### 6.4 与 progress_hint / SendLimiter 的关系

- **hint 不变**：微信 `progress_hint` 保持默认 `off`。typing 开启后 hint 依旧不建议开（独立消息占配额，R14）。两套机制语义重叠但互不干扰，**本次不合并 `HintPolicy`**（§13）。
- **绕过消息节流**：sendtyping 是独立端点、不产生聊天消息，`SendLimiter` 管的是 sendMessage 配额，typing 调用**不入**该预算。自带节奏约束：刷新间隔 ≥ 配置值（默认 5s）+ 失败退避 + 熔断。
- **待验证**：sendtyping 是否与 sendMessage 共享账号级 -2 反刷惩罚（§11 探针 P4）。若共享：typing 循环观测到 -2 时自行长退避，并可在消息冷却期内暂停点亮（按探针结果定，v1 先只做自身退避）。

## 7. 过渡机制与发布门禁

W-D 依赖上游发版。按维护者与 weixin-agent 的关系分三条路径——**终态架构完全相同，仅过渡机制不同**：

### 7.1 路径决策（已确认：B，2026-11-23 维护者拍板）

weixin-agent 是第三方 crate（作者 spensercai），路径 A 不适用。**采用路径 B**（PR + fork 桥接）：delta 极小且纯增补，合并概率高；门禁（§7.3）把等待隔离在 feature 分支。若上游长期无响应则落入 C，每个发版窗口重议。chatbot 层抽象与真机探针不依赖上游，先行实施。

**上游现状核查（2026-11-23，GitHub API + crates.io 实测）：**

- 仓库 [SpenserCai/weixin-agent-sdk-rs](https://github.com/SpenserCai/weixin-agent-sdk-rs)：MIT、允许 fork、PR 政策 `all`（任何人可提）；11 star / 4 fork，个人项目，代码约 4.3k 行。
- 发版节奏（crates.io）：0.1.0（2026-03-29）→ 0.2.0（2026-05-19）→ 0.3.0（2026-08-13），约 2–3 个月一版；**0.3.0 至今已 3 个月无新版**，仓库最后 push 2026-08-13。
- PR 历史仅 2 条，均来自同一外部贡献者（zensh / ldclabs）：
  - [PR #1](https://github.com/SpenserCai/weixin-agent-sdk-rs/pull/1) `with_cancel_token`（纯暴露内部能力的小增补）：04-12 提出 → **05-19 随 0.2.0 合并**（约 5 周）。**robit-wechat 已在用这个外部贡献的功能**（platform.rs:230）——同类增补被接受的直接先例。
  - [PR #2](https://github.com/SpenserCai/weixin-agent-sdk-rs/pull/2) 错误响应体校验（含 typing 端点 ret 检查）：06-06 提出 → **至今 open（约 5.5 个月无合并动作）**。与本设计 delta 互补不冲突（其改 api/client.rs 响应校验，本 delta 在 sender/inbound/client 暴露层）。
- **结论**：上游接受小型纯增补 PR 有先例，但节奏是"间歇性、数周到数月"，无响应保证。设计必须把等待隔离在 feature 分支（§7.3），且 robit 侧零返工。

| 路径 | 上游关系 | 过渡机制 | robit 侧动作 |
|---|---|---|---|
| ~~A · 上游优先~~ | ~~自己可控~~ | — | **不适用**（上游非本维护者所有） |
| **B · PR + fork 桥接**（第三方可提 PR） | 可提交、排期不可控 | 受控 fork（如 `github.com/<org>/weixin-agent`，分支 `typing-client-api`，仅含 §5 delta）+ feature 分支上 `[patch.crates-io]` | 立即提 PR；robit typing 在 **feature 分支**对着 fork 开发（代码从第一天就是终态写法）；上游 0.4.0 上架 → bump 依赖、删 patch → 合 main |
| **C · 长期 fork**（上游不可控且 PR 无望） | 不可控 | 同 B 但 fork 长期存续 | 同 B；每个发版窗口决策：继续等上游 / 功能滞留分支。**不推荐**——优先把 C 转化为 B（任何人都可开 PR） |

```toml
# 路径 B/C：仅 feature 分支的根 Cargo.toml，合 main 前删除
[patch.crates-io]
weixin-agent = { git = "https://github.com/<org>/weixin-agent", branch = "typing-client-api" }
```

fork 纪律：delta 严格限于 §5（约 60 行 + 测试），定期 rebase 上游；**不**在 fork 里加 robit 专属逻辑——它就是要原样合入上游的 PR 内容。

### 7.2 为什么不用 W-C 过渡

v1 的 W-C（robit 内置协议客户端）虽不依赖上游排期，但把协议代码搬进 robit、终态还要再删一次，与维护者"协议归 SDK、W-D 为终态"的指示相悖。fork 桥接把同样的等待期成本放在**正确的层**（SDK 层），robit 代码零返工。W-C 仅在上游彻底无望且发版期限不可延的极端情形下作为最后备选重议（需新决策，本文档不展开）。

### 7.3 发布门禁（硬约束）

**crates.io 不接受 git 依赖**，而 robit 发版流程（`release.yml`）会把全部 crate 发布到 crates.io。因此：

- `[patch.crates-io]` 只能活在 feature 分支；**main 必须始终依赖 registry 上的 weixin-agent ≥ 0.4.0**。
- typing 功能合入 main 的前置条件 = 上游 0.4.0 已在 crates.io 可用。路径 A 天然满足；路径 B/C 的合入时点由上游发版触发。
- 门禁期间 main 照常可发（无 typing，行为同 0.5.1）；typing 在 feature 分支持续集成（CI 用 fork）。
- 上游 0.4.0 上架后的合入 PR：bump `weixin-agent = "0.4"` + 删 patch + `Cargo.lock` 同步，robit 源码**零改动**（从第一天就按终态 API 写）。

## 8. 兼容性破坏清单与迁移（需确认）

维护者已允许适度破坏。本次**唯一**的公开 API 破坏是 `PlatformCaps`，其余均为增补：

| 变更 | 性质 | 波及与迁移 |
|---|---|---|
| `PlatformCaps` 增 `supports_typing` + `#[non_exhaustive]` + `Default` | **破坏**（外部字面量构造失效） | workspace 内 7 处测试字面量同 PR 改写（§3.1 已列点）；下游自定义 adapter 迁移：`let mut caps = PlatformCaps::default(); caps.supports_edit = true; ...`（字段仍 pub，可读写；`non_exhaustive` 只禁外部字面量构造）。**此后增补能力字段永久非破坏** |
| `PlatformAdapter` / `PlatformSender` 增 `set_typing`、`typing_enabled` | 非破坏 | 带默认实现（no-op / caps 声明），既有实现者零改动 |
| `ChatbotFrontend` 增 `typing_enabled` pub 字段 + 私有 `typing_on` | 事实破坏（外部字面量构造失效） | 受支持构造路径是 `new()` + builder，文档明示；不另加 `non_exhaustive`（控制破坏面） |
| `WechatBotConfig` 增 `typing` / `typing_refresh_secs` | 非破坏 | serde 可选字段，缺省行为同 0.5.1 |
| `weixin-agent` 0.3 → 0.4 | 非破坏（上游纯增补） | 依赖号 bump；robit 既有调用不受影响 |
| QQ / 飞书行为 | 零变化 | caps `supports_typing=false` + 默认 no-op；回归钉守住（§10） |

**版本策略**：破坏项随下个 **0.6.0** 发布（0.x 阶段 minor 即不兼容位，与 cargo 语义一致）；CHANGELOG 明列 `PlatformCaps` 破坏与迁移示例。发版时按 CLAUDE.md 三处版本号流程执行。

## 9. 可观测性

| 事件 | 级别 | 内容 |
|---|---|---|
| 期望态迁移 | debug | `[chatbot] typing on/off: chat_id=...`（去重后每轮至多两条） |
| 钩子调用失败 | warn | chat_id + 错误（frontend 吞错不失败轮次） |
| sendtyping/getconfig 失败 | warn | SDK 侧类别化传输日志（既有）；robit 侧熔断 warn 每聊天一次 |
| 观测到 -2（若探针证实共享惩罚） | warn | 明示"服务端反刷、分钟级自愈、勿密集重试"（对齐 R14 W4） |

## 10. 测试计划

**上游（weixin-agent PR 内）：**

1. `MessageSender::set_typing` 请求构造：status 线格式、ticket 注入、base_info 一致。
2. `WeixinClient` / `MessageContext` 两入口委托同一 helper（mock API 断言）。
3. e2e `typing` 命令 client 级变体（真机探针复用）。

**chatbot 层（MockSender 记录 `set_typing` 调用序列，离线）：**

4. 注入成功 → 恰好一次 `(chat, true)`；TurnComplete → 恰好一次 `(chat, false)`。
5. Error 收口 → false；空答案轮次（buffer 空）→ false 仍发出。
6. 一轮内 N 个 TextDelta/工具事件 → true 只调一次（去重闸）。
7. 手动确认：request 前 false、决议后 true；批准/拒绝/超时三路径均复亮后由收口事件灭灯。
8. auto_approve：确认路径零 typing 调用（不暂停）。
9. `typing_enabled=false`：全程零调用。
10. 钩子返回 Err：轮次正常完成（吞错断言）。
11. `PlatformCaps::default()` 基线快照 + 三构造器取值断言（重构回归钉）。

**wechat 层（TypingTransport mock，离线）：**

12. 状态格：true/true 幂等、false/false 幂等、true→false→true 任务重建、shutdown 级联取消。
13. 熔断：连续 3 次失败退出循环，下次 true 重置。
14. refresh=0：发一次后驻留，不重复调用。
15. 委托正确性：SdkTyping 把 peer/token 原样传给 client（mock client 层断言）。

**回归钉：**

16. QQ caps 逐位不变（除新增 `supports_typing=false`）+ 现有 hint/编辑合并测试全绿。
17. 微信 `typing` 缺省（未配置）→ 行为与 0.5.1 逐位一致（零 typing 调用）。

## 11. 真机验证清单（Stage 0，先于默认值翻转）

用 SDK e2e `typing` 命令（含 §5.3 新增 client 级变体）扩展探针，逐项记录：

| # | 探针 | 决定的设计参数 |
|---|---|---|
| P1 | 点亮后不取消、不回复，客户端显示持续多久？是否自动过期？ | 刷新间隔默认值（现设 5s 保守值）；若永久持续可设 `typing_refresh_secs=0` 省调用 |
| P2 | 发出回复后 typing 是否自动清除？任意出站消息（如确认提示）是否清除？ | F2 灭灯与回复的时序观感 |
| P3 | 长轮次（>60s）期间按 5s 刷新，显示是否稳定连续？ | 刷新间隔与熔断阈值 |
| P4 | 高频 sendtyping（如 1s 间隔连发）是否触发 -2？是否波及 sendMessage？ | §6.4 冷却联动是否实现 |
| P5 | ticket 有效期是否确为 ~24h？空 ticket 发送的服务端响应？ | SDK 缓存 TTL 验证（上游既有语义，robit 零感知） |
| P6 | 确认等待场景：灭灯后客户端是否立即消失？ | F4 观感验收 |

探针结论回填本文档 §6 参数，再开默认值翻转 PR。

## 12. 风险

| 风险 | 缓解 |
|---|---|
| 上游排期不可控（路径 B/C） | §7.3 门禁把等待隔离在 feature 分支，main 随时可发；fork delta 极小（~60 行）易合并；优先转化为路径 A/B |
| fork 与上游漂移 | delta 严格限 §5、定期 rebase、不含 robit 专属逻辑；上游合入即删 fork |
| 客户端 typing 显示语义未实测（P1/P2） | 默认关 + Stage 0 探针门禁；钩子 best-effort，最坏退回"无 typing"现状 |
| sendtyping 共享反刷惩罚（P4） | 保守刷新间隔 + 熔断 + 自身退避；探针证实后加冷却联动 |
| 进程崩溃时 typing 滞留 | 客户端侧过期兜底（P1 实测）；优雅关停路径已覆盖 |
| 钩子阻塞关键路径 | trait 契约"立即返回"+ adapter 全 spawn 实现；代码评审检查项 |
| `PlatformCaps` 破坏波及下游自定义 adapter | §8 迁移示例 + CHANGELOG 明示；`non_exhaustive`+`Default` 保证此后不再破坏 |

## 13. 非目标与后续

**非目标（本次不做）：**

- 渲染思考内容（ThinkingDelta 维持静默计数）。
- 合并 `HintPolicy` 为 native/hint/off 大一统——两套机制先并存跑稳，合并是后续可选整理。
- 工具进度 item（类型 11/12）——客户端不渲染（§1.2），纯协议储备，不接入。
- QQ typing——平台无 API，no-op 即诚实表达。
- 宿主直推（绕过 manager）的 typing——瞬时发送非轮次，无语义。
- robit 内任何 typing 协议代码（W-C 废弃，协议始终在 SDK）。

**后续：**

1. 飞书流式卡片：实现同一 `set_typing` 钩子（渲染为卡片更新），生命周期零重写。
2. 探针数据支持时：`typing` 默认值翻转、刷新间隔调优、冷却联动。
3. 上游若接受更大重构：`MessageContext` typing 与 client typing 的文档互链、`TypingStatus` 便利构造等（不阻塞本次）。

## 14. 变更清单与实施顺序

| 仓库/文件 | 变更 | 量级 |
|---|---|---|
| **weixin-agent**（上游或 fork）`messaging/sender.rs` / `messaging/inbound.rs` / `client.rs` | §5 共享 helper + client 级 API + 测试 + e2e 变体 | ~60 行 + 测试 |
| `crates/robit-chatbot/src/adapter.rs` | caps 字段 + `non_exhaustive`/`Default` 重构 + 三构造器改写 + `set_typing`/`typing_enabled` | 小 |
| `crates/robit-chatbot/src/frontend.rs` | PlatformSender 钩子 + 字段/builder + `set_typing_state`/`notify_turn_started` + F1-F4 + 测试字面量改写 | 小 |
| `crates/robit-chatbot/src/manager.rs` | Bridge 显式委托 + M1-M4 | 小 |
| `crates/robit-chatbot/src/{confirmer,outbound}.rs` | 测试 caps 字面量改写（§3.1 列点） | 小 |
| `crates/robit-ai/src/config.rs` | WechatBotConfig 两字段 | 小 |
| `crates/robit-wechat/src/platform.rs` | WechatConfig 解析 + TypingTransport/SdkTyping + TypingCell 状态格 + `set_typing`/`typing_enabled` + 关停级联 | 中 |
| 根 `Cargo.toml` / `Cargo.lock` | `weixin-agent = "0.4"`（合 main 时）；feature 分支期间 `[patch.crates-io]` | 小 |
| 测试 | §10 4-17 | 中 |
| CLAUDE.md / CHANGELOG.md | 配置文档 + 破坏项与迁移 + 变更登记 | 小 |

**实施顺序：**

1. **上游 delta**（§5）：路径 A 直接实现并发 0.4.0；路径 B/C 提 PR + 建 fork 分支。
2. **robit-chatbot 抽象 + caps 重构**（§3，含测试与 QQ 回归钉）——不依赖 SDK 新 API，可先行合 main（默认 no-op，行为不变）。
3. **robit-wechat 实现**（§6）：feature 分支对着 fork/0.4.0 开发（路径 A 直接在 main 线）。
4. **真机探针**（§11）→ 参数回填。
5. **合入与发布**：上游 0.4.0 上架 → bump 依赖、删 patch → 合 main → 随 0.6.0 发布（默认关）。
6. **默认值翻转**（另 PR，探针门禁通过后）。
