# RFC：智能体计划任务（Scheduled Tasks）

- 状态：M1 已实现（核心 + Bot）；M2/M3 待启动
- 日期：2026-11-24
- 涉及 crate：`robit-agent`（核心）、`robit-chatbot`（首个接线前端）、`robit-gui` / `robit-tui` / `robit-web`（后续接线）

## 1. 背景与目标

当前 Agent 只能被动响应用户输入。希望增加**计划任务**能力：用户（或 Agent 受用户明确委托）登记一个「未来某时刻 / 周期性触发的提示词任务」，到点后系统把提示词作为一轮用户输入注入指定会话，Agent 正常执行并把结果送达前端。

典型场景：

- QQ/微信 Bot：每天早上 9 点给群里发日报；每小时巡检一次服务器并汇报；30 分钟后提醒我开会。
- GUI/TUI：定时跑测试并总结；每周一整理上周记忆文件。
- Web 宿主（如 zimu-server）：定时生成行情摘要推送给订阅会话。

**目标**

1. Agent 拥有自助工具面：`schedule_create` / `schedule_edit` / `schedule_list` / `schedule_view`，LLM 可在对话中登记与管理任务。
2. 任务持久化（进程重启不丢），到点可靠触发，触发内容以「计划任务」身份进入会话历史（可区分于真实用户消息）。
3. 前端无关：核心在 `robit-agent`，各前端通过统一的投递抽象接线；Bot 平台复用已落地的 `inject_message`（H1）。
4. 安全面不放松：注入回合走正常工具确认流程；任务只能由用户明确请求创建（提示词纪律 + 工具确认双保险）。

**非目标（本期不做）**

- 操作系统级 cron / 系统服务集成（进程内调度即可，Bot 常驻进程天然满足）。
- 完整 RFC 5545 RRULE（BYSETPOS、BYWEEKNO 等高级组合）——用「一次性 / 固定间隔 / 标准 cron」三种触发器覆盖绝大多数需求。
- 跨机器分布式调度、任务依赖编排（DAG）。
- `new_session` 目标的完整实现（本期只预留字段，见 §6.2 拍板点）。

## 2. 可复用的现有基础

| 基础 | 位置 | 对本设计的意义 |
| --- | --- | --- |
| 宿主注入 API `inject_message(chat_id, text, source)` | `robit-chatbot/src/manager.rs`（H1，已落地） | Bot 侧投递端子现成：绕过斜杠解析与确认关键词路由、死句柄自愈、落库带 source 标记 |
| `messages.source` 列（migration v6） | `robit-agent/src/storage.rs` | 注入回合档案标记 `"scheduler"`，与真实用户消息可区分 |
| 会话隔离键 `(chat_id, source, namespace)`（migration v7） | 同上 | 任务归属直接沿用三元组，天然解决跨平台 / 同平台多实例的任务串台 |
| 统一 SQLite schema + 版本化迁移链（当前 v7） | `storage.rs` | 任务表走 v8 迁移，所有前端共库 |
| 异步任务机制（`TaskRegistry` / `async_runner` / `query_task`） | `robit-agent/src/tool/` | 「共享句柄经 `ToolContext` 注入无状态工具」的模式照搬给 schedule 工具；但**调度器本体不复用** async_runner（它是回合内一次性后台任务，无持久化、无周期语义） |
| `FrontendMessage::UserInput` 消息循环 | `agent.rs` run() | 注入即一轮正常 turn，mpsc 排队天然串行化 |
| `WebBridge::send_message` | `robit-web/src/bridge.rs` | Web 宿主投递端子现成 |

**关键判断**：计划任务 = 「持久化的定时触发器」+「已存在的消息注入通道」。新增代码集中在触发器与存储，投递端各前端基本是现成端子的适配。

## 3. 总体架构

```txt
┌────────────────────────── 宿主进程（每个前端一个）──────────────────────────┐
│                                                                            │
│  ChatbotManager / GUI App / TUI App / Web 宿主                              │
│    └── 拥有 Scheduler（tokio 任务，进程内单例）                              │
│          ├── ScheduleStore（SQLite，robit.db，migration v8）                │
│          │     scheduled_tasks 表 + task_runs 表                            │
│          ├── 触发循环：sleep 到最近 next_run_at，CRUD 经 Notify 唤醒         │
│          └── ScheduleDelivery（trait，前端实现）                            │
│                ├── Bot:  inject_message(chat_id, wrapped_prompt, "scheduler")│
│                ├── GUI/TUI: message_tx.send(ScheduledInput{..})             │
│                └── Web:  WebBridge::send_message(ScheduledInput{..})        │
│                                                                            │
│  Agent（每会话一个）                                                        │
│    ├── ToolContext.schedule: Option<ScheduleContext>                        │
│    │     （Store 句柄 + 唤醒 Notify + 本会话归属绑定 source/namespace/chat_id）│
│    ├── schedule_create / schedule_edit / schedule_list / schedule_view      │
│    └── run() 新增 FrontendMessage::ScheduledInput 分支                      │
│          → 包装提示词 → run_turn()（正常工具确认流程）                       │
└────────────────────────────────────────────────────────────────────────────┘
```

分层纪律（与现有架构一致）：

- `robit-agent::schedule` 只做**存储 + 触发 + 投递抽象**，不认识任何具体前端，不依赖 HTTP/IM SDK。
- **Scheduler 归宿主所有**，不归 Agent 所有：Agent 是每会话 spawn、会被空闲回收的（Bot cleanup_loop），而调度必须跨会话存活。Agent 只通过 `ToolContext` 拿到 CRUD 句柄。
- 一个宿主进程一个 Scheduler；任务归属 `(source, namespace)` 过滤保证共库的多个宿主各触发各的任务，不会重复触发（§6.4）。

## 4. 数据模型与持久化（migration v8）

沿用 `storage.rs` 迁移链，v7 → v8：

```sql
CREATE TABLE IF NOT EXISTS scheduled_tasks (
    id              TEXT PRIMARY KEY,           -- UUID v4
    title           TEXT NOT NULL,              -- 任务名（用户语言，≤120 字符）
    prompt          TEXT NOT NULL,              -- 到点注入的提示词原文
    schedule        TEXT NOT NULL,              -- 触发器 JSON（§5）
    timezone        TEXT NOT NULL DEFAULT '',   -- IANA 时区名；'' = 宿主本地时区
    target          TEXT NOT NULL,              -- 投递目标 JSON（§6）
    source          TEXT NOT NULL,              -- 归属前端："qq"/"wechat"/"gui"/"tui"/"web"
    namespace       TEXT NOT NULL DEFAULT '',   -- [app.bot] instance_id，同平台多实例隔离
    status          TEXT NOT NULL DEFAULT 'active',  -- active | paused | completed | deleted
    max_runs        INTEGER,                    -- NULL = 不限；达到后置 completed
    run_count       INTEGER NOT NULL DEFAULT 0,
    next_run_at     TEXT,                       -- UTC ISO8601；paused/completed 为 NULL
    last_run_at     TEXT,                       -- UTC ISO8601
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL               -- 乐观并发控制令牌（schedule_edit 校验）
);
CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_due
    ON scheduled_tasks(status, next_run_at);
CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_owner
    ON scheduled_tasks(source, namespace);

CREATE TABLE IF NOT EXISTS task_runs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id         TEXT NOT NULL REFERENCES scheduled_tasks(id),
    scheduled_at    TEXT NOT NULL,              -- 计划触发时刻（UTC）
    fired_at        TEXT NOT NULL,              -- 实际投递时刻（UTC）
    catch_up        INTEGER NOT NULL DEFAULT 0, -- 1 = 停机补触发
    status          TEXT NOT NULL,              -- claimed | delivered | failed | skipped
    error           TEXT                        -- 失败原因（投递错误等）
    -- claimed = 已认领、投递结果未落定；与认领同事务写入，供崩溃对账（§5.3）
);
CREATE INDEX IF NOT EXISTS idx_task_runs_task ON task_runs(task_id, fired_at);
```

约定：

- 时间戳一律 UTC ISO8601 字符串，与现有 `created_at/updated_at` 纪律一致（`datetime.rs` 注释）；展示层转本地时区。
- `status = 'deleted'` 为软删除（保留 runs 审计），列表默认过滤；`schedule_edit delete` 不物理删行。
- `task_runs` 每任务保留最近 100 条，触发时顺带清理（避免无限增长）。
- 迁移测试覆盖「v7 旧库直升 v8」与「全新库直建 v8」两条路径，沿用现有测试模式。

## 5. 触发器模型

**拍板点 A：结构化 JSON 三选一，而不是裸 RRULE 字符串。** 对 LLM 更友好（参数即语义），校验面小，够用。

```jsonc
// 一次性（提醒类）
{ "kind": "once", "at": "2026-11-25T09:00:00+08:00" }

// 固定间隔（巡检类）；anchor 可选，缺省 = 创建时刻
{ "kind": "interval", "every_secs": 3600, "anchor": "2026-11-24T09:00:00+08:00" }

// 标准 5 段 cron（日报类）：分 时 日 月 周
{ "kind": "cron", "expr": "0 9 * * 1-5" }
```

规则：

- `once.at` 必须带时区偏移或为 UTC；过去时刻创建即报错（不静默补触发）。
- `interval.every_secs ≥ min_interval_secs`（配置，默认 60），防 LLM 手滑造出秒级轰炸。
- `cron` 用 [`cron`](https://crates.io/crates/cron) crate 解析（内部 7 段：秒 分 时 日 月 周 年；适配层把 5 段前置 `"0 "`、年补 `*`）。拒绝 `* * * * *`（每分钟）低于最小间隔的表达式。
- 时区：任务级 `timezone`（IANA 名，如 `Asia/Shanghai`），缺省取宿主本地。cron 在任务时区里求值后转 UTC 存 `next_run_at`，DST 切换由 `chrono-tz` 处理（不存在的本地时刻跳过，重复时刻取较早瞬间）。
- 每次触发后重算 `next_run_at`；`once` 触发后置 `completed`；`max_runs` 达到置 `completed`。

**依赖引入**（workspace 登记）：`cron`、`chrono`、`chrono-tz`。仓库现用 `time` crate 做格式化，两者并存无冲突；schedule 模块内部统一 chrono（cron crate 的生态绑定），边界处转字符串，不向外扩散 chrono 类型。
备选：`tokio-cron-scheduler`（自带调度循环）被否——它绑定自己的任务模型与持久化选项，和「宿主拥有调度、共库迁移链、归属隔离」的既有纪律冲突，自研 tick 循环约 150 行，更可控。

### 5.1 触发循环

```txt
loop {
    now = Utc::now()
    due = store.claim_due(owner=(source,namespace), now)   // 认领 + claimed run 行同事务，见 §6.4
    for task in due {
        （task_runs 行已在认领事务中写入，status='claimed'）
        delivery.deliver(task).await
          Ok  → claimed run 行置 delivered；run_count++, last_run_at,
                重算 next_run_at（或 completed）
          Err → task_runs.status=failed + error；按重试策略退避重试（最多 3 次，
                间隔 5s/30s/120s），仍失败则保持 active、next_run_at 照常推进，
                并 tracing::error!（不静默丢任务）
    }
    sleep_until( min(所有 active 任务的 next_run_at) ) 或 wake_notify.notified()
}
```

- CRUD 工具写库后 `notify_one()` 唤醒循环重算 sleep 目标——新任务无需等旧 tick。
- **启动与空载行为**：`enabled = true` 时宿主启动即无条件 spawn 调度循环——先做一次索引查询（due 扫描 + claimed 对账，§5.3），再计算 sleep 目标；**零任务时直接 park 在 Notify 上，不注册定时器、零轮询、零空转**，常驻成本仅一个挂起的 tokio 任务。不做惰性启动（首个 create 才 spawn）：工具侧需持有 spawn 句柄并与宿主关停接线竞态，收益只是省掉一个近乎免费的挂起任务。运行中创建的首个任务经 `notify_one()` 立即生效，无需重启进程。
- 时钟回拨/休眠唤醒：sleep 醒来后一律以 `Utc::now()` 重新比对 due，不信任 sleep 时长。

### 5.2 停机补触发（missed policy）

进程停机期间错过的触发，启动时按任务处理：

| 策略 | 行为 |
| --- | --- |
| `latest`（默认） | 只补最近一次错过的那次（`catch_up=1`），随后推进到下一个未来时刻 |
| `skip` | 全部跳过，直接推进到下一个未来时刻 |

配置项 `[app.schedule] missed_policy`。不做 `all`（全量补发）——对 IM 前端是消息轰炸，收益低。`once` 任务错过即按策略补一次或置 completed（skip 时标注 missed）。

### 5.3 关停与崩溃恢复

**总原则：状态全在库里，触发循环只是闹钟。** 内存中不持久化任何「睡眠进度」，关停无需保存现场；优雅退出与强杀都不丢任务，需要处理的只有「关停瞬间正在飞的那一次触发」。

**优雅关停（Ctrl+C / SIGTERM / GUI 退出）**

1. 复用宿主既有的 `CancellationToken`（Bot 即 `ChatbotManager::run` 的 shutdown，与 cleanup_loop 同源）；调度循环 `select!` 加 `_ = shutdown.cancelled()` 臂。level-triggered，信号落在投递中途也不会丢（与 manager 事件循环同款纪律）。
2. 收到信号后**不再认领新任务**；给进行中的投递一个短宽限窗（建议 2s——投递本质是 channel send + 落库，毫秒级）。窗口内未完成则保持 `claimed` 状态退出，交给启动对账。
3. **顺序**：先停 Scheduler（掐断新回合注入来源），再走宿主既有的 Agent 排水（Bot 为 `drain_agents(SHUTDOWN_GRACE=15s)`）。反过来会在排水期间继续注入新回合。
4. 已注入、仍在执行的 LLM 回合**不归调度器等待**——那是 Agent 生命周期，由既有排水宽限 + Agent 自身 drain phase（取消异步任务）处理；二次 Ctrl+C 立即退出，长回合不能劫持进程。

**崩溃 / 断电（无优雅钩子）**

SQLite 事务原子性保证无半状态。唯一风险窗口：**认领成功（`next_run_at` 已推进）但投递未完成**——该次触发「已消耗未送达」，且下次启动时 `next_run_at` 已在未来，missed policy 看不见它。对策：

- 认领（推进 `next_run_at`）与写入 `task_runs(status='claimed')` **同一事务**（§6.4）。
- 启动时对账：扫描本归属下残留的 `claimed` run 行（上次进程没来得及落定结果），按 `missed_policy` 处理——`latest` 补投递一次（`catch_up=1`）或 `skip` 置 `skipped`，随后正常推进。崩溃窗口从「静默丢失」收敛为「至多一次补触发」。

**语义澄清：`delivered` = 已移交会话通道，≠ 回合执行成功。** 投递成功后进程退出、回合没跑完的情形：档案里有 scheduler 用户行、无 assistant 回复，任务**不重触发**（投递责任已结束），会话下次启动从库复活。回合执行结果是 Agent 的事，审计经 `task_runs.scheduled_at / fired_at` 与档案对照。

**边界**：进程必须活着才会触发（OS 级常驻是非目标，§1）；停机时段由 missed_policy 覆盖。调度器不补偿时钟漂移（§13）。

## 6. 投递语义

### 6.1 目标模型

```jsonc
// target JSON
{ "kind": "this_session", "session_id": "…", "chat_id": "group:123" }
```

- `this_session`：注入创建任务时所在的会话。Bot 存 `chat_id`（会话可被空闲回收，注入时经 `get_or_create_session` 从库复活——inject_message 已内置）；GUI/TUI/Web 存 `session_id`。
- `new_session`（**本期预留，不实现**）：每次触发开新会话执行。难点在「结果送达哪」——Bot 需要新会话仍绑定原 chat 的输出通道，GUI 需要宿主 spawn 新 Agent 并路由事件。字段与枚举先留好，M3 再拍。

### 6.2 忙碌与串行

注入走各前端现成的消息通道（mpsc），Agent 循环逐条消费：**上一回合没跑完，计划回合自然排队**，无需额外锁。两个推论：

- 长回合可能让触发延迟数分钟——`task_runs.fired_at` 记录的是投递时刻，`scheduled_at` 记录计划时刻，审计可查。
- 同一任务上一次注入还在排队/执行时又到点（interval 过短）：默认照常排队（queue 语义）；`min_interval_secs` 已挡住绝大多数病态配置。不做 skip-if-busy（需要跨层 busy 信号，复杂度不值）。

### 6.3 注入回合的身份

新增 `FrontendMessage` 变体（向后兼容，`From<String>` 等不动）：

```rust
/// A scheduled task fired; deliver `prompt` as a user turn marked as scheduler-origin.
ScheduledInput { task_id: String, title: String, prompt: String },
```

`Agent::run()` 新分支行为（对齐 inject_message 的纪律）：

1. **不过**斜杠命令 / 技能触发解析（prompt 里写 `/help` 就是字面文本进模型）；
2. 包装后走 `run_turn`：

   ```txt
   [计划任务「{title}」(id={task_id}) 于 {本地时间} 触发]
   {prompt}
   ```

3. 档案落库 `source = "scheduler"`（复用 v6 列；GUI/TUI 的落库路径需补 with_source 变体，Bot 侧 inject_message 已支持）；
4. 发 `AgentEvent::ScheduledTaskFired { task_id, title }` 供前端渲染系统提示条（新事件变体，旧前端 match 加一臂忽略即可）；
5. 工具确认、上下文压缩、记忆等一切照旧——**安全面不放松**。

Bot 路径可以不经过该变体：Scheduler 的 Bot 投递实现直接调 `inject_message(chat_id, 包装后的文本, "scheduler")`，包装在投递层做。变体主要服务 GUI/TUI/Web 直连 `message_tx` 的场景。

### 6.4 归属与多进程安全

- 任务行带 `(source, namespace)`；每个宿主的 Scheduler **只认领自己归属的任务**（`claim_due` 的 WHERE 条件）。GUI 与 QQ Bot 共库（global_storage）时互不触发对方任务；两个 QQ 实例靠 `instance_id` 区分——与 v7 会话隔离同一套键，无新概念。
- 认领用单条条件 UPDATE（`SET next_run_at = <推进值> WHERE id = ? AND next_run_at = <旧值>`，changes()=1 才算认领成功），**与写入 `task_runs(status='claimed')` 同一事务**（崩溃对账依据，§5.3）；同归属双进程误配时也不会双触发。
- 同 `(source, namespace)` 双开本就是会话层的已文档化误配，此处不额外兜底。

## 7. Agent 工具面

四个工具，注册纪律对齐记忆工具：**受 `[app.schedule] enabled` 门控（默认 false），启用后作为系统工具注册，不受 `enabled_tools` 白名单影响**。

共享句柄模式照搬 `TaskRegistry`：`ToolContext` 加

```rust
pub schedule: Option<ScheduleContext>,   // None = 功能未启用，工具不注册
pub struct ScheduleContext {
    pub store: ScheduleStore,            // Arc 内核，clone 廉价
    pub wake: Arc<tokio::sync::Notify>,
    pub binding: ScheduleBinding,        // 本会话归属：source/namespace/session_id/chat_id
}
```

`ScheduleBinding` 由宿主在构造 Agent 时填入（Bot = chat_id + platform_name + instance_id；GUI/TUI = session_id + "gui"/"tui"）。工具是无状态单例，绑定经 ToolContext 走——与 TaskRegistry 注释里阐述的理由完全一致。

### 7.1 schedule_create（需用户确认）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `title` | string | 任务名，用户语言，≤120 字符，不含时间/状态/id |
| `prompt` | string | 到点注入的完整指令（自包含：触发时没有对话上下文可依赖，描述里写明） |
| `schedule` | object | §5 触发器 JSON |
| `timezone` | string? | IANA 时区，缺省宿主本地 |
| `max_runs` | int? | 最大触发次数 |

返回：`id`、下次触发时刻（本地 + UTC 双格式）。校验失败（过去时刻、间隔过小、cron 非法、超配额）返回结构化错误让 LLM 修正。

**提示词纪律**（写进工具 description + 系统提示词 schedule 段）：*仅在用户明确要求定时/提醒/周期执行时创建；不得把「用户随口提到将来要做的事」自作主张登记为任务。* 工具确认弹窗是第二道闸。

### 7.2 schedule_edit（需用户确认）

`{ id, action: "update" | "pause" | "resume" | "delete", expected_updated_at, ...fields }`

- `expected_updated_at` 乐观并发：不匹配返回冲突错误（任务可能已被别的会话/前端改过），LLM 重新 view 后再试。
- `update` 可改 title/prompt/schedule/timezone/max_runs；改 schedule 即重算 next_run_at。
- `pause` 置 paused、next_run_at=NULL；`resume` 重算（错过部分按 missed_policy）；`delete` 软删除。
- 只能操作**本绑定归属**的任务（WHERE 带 source/namespace；Bot 再限 chat_id——A 群不能改 B 群的任务）。跨归属返回「不存在」。

### 7.3 schedule_list / schedule_view（免确认）

- `list(status?, limit=20, offset?)`：本归属任务摘要（id/title/状态/下次触发本地时间/run_count）。
- `view(id)`：全字段 + 最近 10 条 runs。

### 7.4 用户直接管理面（不经过 LLM）

- TUI：~~`/schedule` 斜杠命令~~（取消，见 §11 决策）。
- Bot：`/schedule` 走现有斜杠解析（Manager 层处理，不进模型）。
- GUI：M3 加管理面板（任务列表 + 运行历史 + 启停删）。
- 这些入口与工具共用 ScheduleStore API，确认纪律各自前端自理。

## 8. 配置

```toml
[app.schedule]
enabled = false            # 总开关；false 时不注册工具、宿主不启动 Scheduler
missed_policy = "latest"   # latest | skip（§5.2）
min_interval_secs = 60     # interval 下限 & cron 最小有效间隔
max_active_tasks = 50      # 每归属（每 chat / 每会话）active+paused 任务上限
delivery_retries = 3       # 投递失败重试次数（退避 5s/30s/120s）
```

- 加载纪律同现有 config：项目本地覆盖全局；`enabled` 缺省 false，**不配置行为逐字节不变**。
- Bot 无人值守场景的工具确认：沿用现有 `[app.bot] auto_approve` / `confirm_timeout_secs`，**不新增 schedule 专属豁免**。默认确认超时=拒绝，意味着无人值守时写类工具会失败——这是有意的安全默认；需要全自动的宿主自行开 `app.bot.auto_approve`（既有旋钮，风险自负）。文档明示此交互。

## 9. 各前端接线

| 前端 | Scheduler 宿主 | Delivery 实现 | 里程碑 |
| --- | --- | --- | --- |
| `robit-chatbot`（QQ/微信） | `ChatbotManager::run` 旁 spawn（与 cleanup_loop 并列，共享 shutdown） | `inject_message(chat_id, wrapped, "scheduler")` | **M1（首个消费者）** |
| `robit-gui` | App 级单例（多会话共享） | 按 session_id 路由到对应 Agent 的 message_tx，发 `ScheduledInput`；会话已关闭则从库复活或记 failed | M2 |
| `robit-tui` | — | **不做**（2026-11-24 用户拍板：短命交互会话，价值场景在常驻 Bot） | 取消 |
| `robit-web` | 宿主自便（桥基座不内置 HTTP/调度生命周期） | `WebBridge::send_message(ScheduledInput)`；robit-web 只导出适配 helper | M2 |

Bot 选为首个消费者的理由：常驻进程（Ubuntu 部署）+ 注入 API 现成 + 提醒/日报是最强需求场景；且 M1 对 agent 核心的侵入最小（ToolContext 一个字段 + 四个工具 + schedule 模块，`FrontendMessage` 变体可延后到 M2）。

## 10. 安全与边界

1. **创建闸**：提示词纪律（仅用户明确要求）+ 工具确认（默认需批准）双保险。
2. **执行闸**：注入回合不豁免任何工具确认；`auto_approve` 语义不变。
3. **归属闸**：CRUD 与触发都带 `(source, namespace[, chat_id])` 过滤，跨群/跨平台/跨实例不可见、不可改、不触发。
4. **配额闸**：min_interval_secs / max_active_tasks / prompt 长度上限（建议 8KB）/ title 长度上限，防资源滥用与上下文污染。
5. **注入内容即历史**：与 H1 一致，注入回合参与后续上下文（模型下回合可见），也参与压缩——计划任务不是旁路系统消息。
6. **prompt 注入面**：任务 prompt 由创建时的对话产生，若对话本身被第三方内容污染（如群消息），确认弹窗是最后防线；Bot 场景文档建议对含写操作的任务保持人工确认。

## 11. 里程碑

### M1：核心 + Bot（可独立发版）

- `robit-agent::schedule`：ScheduleStore（v8 迁移）+ 触发器求值 + Scheduler 循环 + ScheduleDelivery trait。
- 四个工具 + ToolContext.schedule + 配置门控。
- ChatbotManager 接线：启动/关停 Scheduler、BotDelivery、`/schedule` 斜杠命令。
- 测试：迁移链（v7→v8、旧库直升）、触发器求值（cron/interval/once、DST、时区）、missed policy、归属隔离、乐观并发、配额校验、inject 端到端（复用 manager 现有测试基建）、**关停与崩溃对账**（优雅关停不再认领、claimed 残留行启动补触发/跳过、认领事务原子性）。
- 验收：QQ 群里「每天 9 点发日报」→ 重启进程 → 次日准时收到，档案行 source="scheduler"；停机一晚 → 只补最近一次。

### M2：GUI / Web 接线（TUI 已评估不做）

> **决策（2026-11-24，用户拍板）**：TUI 支持计划任务意义不大——TUI 是短命交互式会话，调度只在窗口开着时运行，用户本就坐在终端前，提醒/周期任务的价值场景在常驻 Bot。**TUI 接线取消**；M2 收窄为 GUI / Web 按需接线。

- `FrontendMessage::ScheduledInput` + `AgentEvent::ScheduledTaskFired` + run() 分支 + GUI 落库 with_source（届时各前端 exhaustive match 同步加臂）。
- GUI 宿主 Scheduler + 会话路由投递；GUI 渲染触发提示条。
- robit-web 导出投递 helper + 协议文档补 `scheduled_input` 帧说明。

### M3：管理体验与 new_session

- GUI 管理面板（列表/历史/启停删）。
- `new_session` 目标落地（先拍「结果送达哪」）。
- 运行历史查询工具化（可选 `schedule_runs`）。

## 12. 拍板点汇总

| # | 问题 | 推荐 | 备选 |
| --- | --- | --- | --- |
| A | 触发器表达 | 结构化 JSON：once / interval / cron(5 段) | 裸 RRULE 子集字符串（表达力强但 LLM 易错、校验面大） |
| B | 调度器归属 | 宿主进程单例，Agent 只拿 CRUD 句柄 | Agent 内置（会被空闲回收杀掉，否） |
| C | 首个接线前端 | Bot（inject_message 现成 + 常驻进程） | GUI 先行（需求弱、接线多） |
| D | 停机补触发 | latest（只补最近一次） | skip / all（轰炸风险） |
| E | 工具确认 | create/edit 全部需确认，list/view 免确认 | 仅 create/delete 确认（pause/resume 也改变持久行为，一并确认更稳） |
| F | 默认开关 | `[app.schedule] enabled = false` | 默认开（改变现有部署行为面，否） |
| G | cron 依赖 | `cron` + `chrono`/`chrono-tz`，自研 tick | `tokio-cron-scheduler`（任务模型/持久化绑死，否） |
| H | 忙碌重叠 | queue（mpsc 天然串行） | skip-if-busy（需跨层 busy 信号，M1 不做） |

## 13. 风险与开放问题

- **时钟依赖**：容器/服务器时钟漂移会平移所有触发；文档建议宿主开 NTP。不做单调时钟补偿（跨重启无意义）。
- **Bot 会话复活的人格一致性**：inject_message 复活会话走 get_or_create_session，人格提示词（H6）已随 Manager 配置注入，无额外风险。
- **GUI 会话路由**：GUI 多会话的 session_id → message_tx 路由表目前散在 commands 层，M2 需要收敛一个可查询的会话注册表（小重构，提前知会）。
- **微信配额**：定时任务在微信侧受出站限流（`[channels.wechat_bot.rate_limit]`）约束，投递失败重试与限流冷却叠加时可能延迟——task_runs 如实记录，不另做豁免。

## 14. M1 实施注记（与草案的差异）

1. **cron 星期映射适配**：`cron` 0.15 采用 1=Sunday…7=Saturday（数字与名称皆然），与标准 cron 的 0/7=Sunday 不同。`trigger.rs::translate_dow` 做适配：数字 `n → (n%7)+1`，名称先归一为标准序数再转换；回绕区间（`5-1`）拆成两段，回绕+步长≠1 拒绝并提示改用列表。
2. **时间戳毫秒精度 + 单调 updated_at**：schedule 各表时间戳统一毫秒精度 RFC3339（定长可字典序比较）；`save_expected` 保证 `updated_at` 逐行严格单调（同毫秒内连写强制 +1ms），乐观并发令牌不再碰撞。
3. **run_count 在认领时自增**（含投递失败的尝试），与草案「Ok 后自增」略有差异——崩溃一致性更好，语义为「触发尝试次数」。
4. **Bot 投递走 mpsc 回路**：Scheduler 的 BotDelivery 把触发事件发回 `ChatbotManager::run` 循环，由循环调 `inject_message`——避免 `Arc<Self>` 重构，且注入与平台消息天然串行。关停顺序：先 cancel 调度器 token（2s 宽限 await）再 `drain_agents`。
5. **提示词纪律 M1 仅落在工具 description**（系统提示词 schedule 段留待 M2，避免 Agent 构造期签名改动）；工具确认弹窗为第二道闸，不变。
6. **`/schedule` 命令**：list / pause / resume / delete，id 前缀匹配（多匹配拒绝），与工具共用 Store 与归属隔离。
