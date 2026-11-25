# Changelog

本项目所有重要变更记录于此文件。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

> 历史版本（v0.1.16 及更早）未回溯记录，自 v0.1.17 起维护。发版流程见 `CLAUDE.md` 的「发版流程」一节。

## [Unreleased]

### Added

- **计划任务 M3：GUI 管理面板**。StatusBar 新增闹钟按钮，点击打开右侧抽屉面板：任务列表（标题/触发器/状态/下次触发）、展开查看详情（提示词/运行历史/时区/ID）、暂停/恢复/删除操作、刷新按钮。Tauri commands：`list_scheduled_tasks` / `get_scheduled_task` / `pause_scheduled_task` / `resume_scheduled_task` / `delete_scheduled_task`。创建仍走 LLM 对话（`schedule_create` 工具）。不配置 `[app.schedule] enabled = true` 时面板显示"未启用"提示。

### Fixed

- **LLM 请求挂起导致会话永久无响应（2026-11-25 robit-qq 线上事故）**。此前 LLM HTTP/SSE 请求在任何一层都没有超时：服务商静默丢弃 keep-alive 连接或 SSE 中途断流时，`chat_stream` / 流收集循环无限等待，该会话的 Agent 循环被永久卡死，后续消息只入库不处理（机器人在线但完全不回复，只能重启）。三层修复：
  - **robit-ai / 传输层超时**：`LlmClient` 的 reqwest 客户端（async-openai typed 路径经 `with_http_client` 注入 + SSE 旁路共用）加 `connect_timeout=10s`、`read_timeout=90s`（按读空闲计时，不影响长生成的 SSE）、`pool_idle_timeout=15s`、`tcp_keepalive=30s`（防连接池复用被中间设备静默丢弃的死连接）。
  - **robit-agent / 调用层超时**：`run_one_step` 中流创建包 60s 首包超时、每个 SSE chunk 包 90s 空闲超时（`RetrySettings` 可配），超时转为新增的 `LlmError::Timeout` 走既有 `AgentEvent::Error` 路径通知用户并释放循环。
  - **robit-agent / `[app.retry]` 真正生效**：此前该配置节只被解析从未使用。新增 `robit_agent::retry` 模块（`RetrySettings` / `retry_backoff`），瞬时错误（超时/连接失败/5xx/408/429/网关 502-504，见 `LlmError::is_retryable`）按指数退避重试（默认 3 次，1s→30s 封顶）；**已有内容流式输出后不再重试**（避免用户侧重复文本）；重试耗尽后报错结束本轮，排队消息得以继续处理。四个宿主（chatbot/TUI/GUI/example）经新增 builder `Agent::with_retry_settings` 接线配置，未配置时用文档默认值。
  - 错误分类增强：`LlmError::from_reqwest_error`（超时/连接区分）、typed 路径 mid-stream `Transport error:` 字符串还原分类、SSE 旁路 mid-stream 超时归类为可重试。

## [0.7.0] - 2026-11-24

计划任务（Scheduled Tasks）M1：核心调度 + Bot 接线。设计见 `docs/robit-rfc-scheduled-tasks.md`（含 §14 实施注记与 TUI 取消决策）。**不配置 `[app.schedule]` 时行为与 0.6.0 逐字节一致**；`Agent`/`ChatbotManager` 公共 API 零签名变更（`with_schedule` 为新增 builder 方法）。

### Added

- **robit-agent / `schedule` 模块**：SQLite 持久化（schema v8：`scheduled_tasks` + `task_runs` 审计，含 v7→v8 迁移）；三种触发器 `once` / `interval` / 标准 5 段 `cron`（任务级 IANA 时区，cron 星期映射适配层）；宿主级事件驱动 Scheduler（零任务零空转、停机补触发 `latest`/`skip`、投递失败退避重试 5s/30s/120s、认领事务 + 崩溃对账、shutdown 先停调度再排水）。
- **robit-agent / 系统工具 `schedule_create` / `schedule_edit` / `schedule_list` / `schedule_view`**：受 `[app.schedule] enabled` 门控（默认关闭），不受 `enabled_tools` 白名单影响；create/edit 需用户确认；经 `ToolContext.schedule` + `Agent::with_schedule` 注入（零签名变更）。乐观并发令牌 `expected_updated_at`（毫秒精度 + 逐行单调）。
- **robit-ai / `[app.schedule]` 配置节**：`enabled` / `missed_policy` / `min_interval_secs` / `max_active_tasks` / `delivery_retries`，全部可选；任何前端（含 TUI）加载含该节的配置均正常启动（配置兼容回归测试）。
- **robit-chatbot / Bot 接线**：Scheduler 随 `ChatbotManager::run` 启停；触发经 mpsc 回路复用 `inject_message`（档案 `source="scheduler"`，绕过斜杠解析，正常工具确认流程）；`/schedule [list|pause|resume|delete <id前缀>]` 直管命令 + `/help` 更新。任务归属沿用 `(source, namespace[, chat_id])` 会话隔离键，共库多前端/多实例互不串台、不重复触发。
- **robit-tui / 配置兼容提示**：TUI 不做调度（短命交互会话，价值场景在常驻 Bot，用户拍板）；检测到 `[app.schedule] enabled = true` 时启动显示系统提示，任务不会在 TUI 触发。

### 依赖

- 新增 `cron` 0.15、`chrono` 0.4、`chrono-tz` 0.10（workspace 登记，robit-agent 使用；robit-chatbot 引 chrono）。

### 后续

- GUI/Web 接线（M2）与管理面板、`new_session` 目标（M3）待后续版本；TUI 接线已取消。

## [0.6.0] - 2026-11-23

Bot 忙碌状态信号（typing）统一抽象（chatbot 层）+ `PlatformCaps` 一次性破坏加固。设计见 `docs/robit-rfc-bot-typing-design.md`（W-D 终态、路径 B）。微信 typing 实际实现依赖上游 weixin-agent 0.4.0（[PR #4](https://github.com/SpenserCai/weixin-agent-sdk-rs/pull/4)），在 `feat/wechat-typing` 分支等待上游发版后合并，**不含在本版本**。

> **升级提示（0.5.x → 0.6.0）**：唯一破坏点是 `PlatformCaps` 加 `#[non_exhaustive]`——crate 外**结构体字面量构造（含 `..Default::default()` 更新语法）编译失败**，迁移为 `let mut caps = PlatformCaps::default(); caps.xxx = ...;` 逐字段赋值。读字段、调 `qq()/wechat()/feishu()` 构造器、实现 `PlatformAdapter`/`PlatformSender`（新方法带默认实现）、`ChatbotFrontend::new()` 与 `ChatbotManager` API 均不受影响。**运行时行为零变化**：typing 钩子全为默认 no-op，无平台启用。0.x semver 以 minor 为不兼容边界，`"0.5"` 依赖不会自动升级到本版。

### Added

- **robit-chatbot / 忙碌状态信号（typing）统一抽象**：新增 `PlatformCaps::supports_typing` 能力声明（微信 true，QQ/飞书 false）与 `PlatformAdapter::set_typing(chat_id, active)` / `typing_enabled()` 钩子（默认 no-op / 读能力声明，既有实现者零改动）；`ChatbotFrontend` 持有期望态去重闸与轮次生命周期——注入成功即点亮（`notify_turn_started`，覆盖 LLM 首 token 延迟）、busy 事件 re-assert、TurnComplete/Error 灭灯、手动确认等待期间暂停/决议后复亮（auto_approve 不暂停）；best-effort 契约（钩子失败仅记日志绝不影响答案投递，adapter 实现须立即返回）。设计见 `docs/robit-rfc-bot-typing-design.md`（W-D 终态：协议始终在 SDK；微信实现随上游 weixin-agent 0.4.0 在 feature 分支落地，见 `docs/upstream-pr-weixin-agent-typing.md`）。本条为 chatbot 层抽象，尚无平台启用 typing，行为与 0.5.1 一致。

### Changed

- **robit-chatbot / `PlatformCaps` 破坏性重构**：加 `#[non_exhaustive]` + `Default`（保守基线：纯文本/无编辑/无媒体/提示关/无 typing），三平台构造器改结构更新语法。**下游自定义 adapter 若以结构体字面量构造 `PlatformCaps` 会一次性编译失败**，迁移为 `PlatformCaps::default()` + 字段覆写（字段仍 pub 可读写）；此后增补能力字段永久非破坏。破坏项自 0.6.0 起生效（0.x 阶段 minor 即不兼容位）。

## [0.5.1] - 2026-11-22

维护版本：依赖修复升级与 CI 现代化，无 API 与行为变化。

### Changed

- **依赖 / rusqlite 0.42 → 0.43**：修复上游 3 处 use-after-free（`ChangesetIter::start_strm` / `Connection::column_metadata` / `Filter::in_values`）与 `time::OffsetDateTime` 短 TEXT panic；上游收紧空/纯注释 SQL 拒绝行为（本仓库均为静态 schema SQL 与动态查询，不受影响）。`libsqlite3-sys` 维持 0.40（bundled SQLite 版本不变），全 workspace 测试与 clippy 基线回归通过。下游若直接引用 rusqlite 并与 robit 公共 API 交换 `Connection`/`Error` 类型，需同处 0.43 线（0.x semver 以 minor 为兼容边界）。
- **CI / 发版 workflow actions 升级 Node 24 运行时**：`actions/checkout` v4 → v7、`actions/setup-node` v4 → v7（GUI 构建 `node-version` 维持 22）、`softprops/action-gh-release` v2 → v3（纯运行时迁移，输入不变）；`Swatinem/rust-cache@v2` 浮动标签已指向 2.9.3（node24）无需变更。消除 GitHub Node.js 20 弃用警告。

## [0.5.0] - 2026-11-22

IM 出站 Markdown 表格投影（子木第七轮移交 R17）。需求移交件见 `docs/robit-rfc-im-table-projection.md`，评估与实现设计见 `docs/robit-rfc-im-table-projection-design.md`（决策点 D1–D6 全部按推荐采纳）。

> **升级提示（0.4.x → 0.5.0）**：双平台可见行为变化——微信会话回复与宿主直调推送现在过纯文本投影（原始管道表格/栅栏/加粗标记不再直达用户）；QQ 含标题/引用/删除线/链接/表格的回复按 features 诚实降级（字面 `**`/`> `/`~~` 去除、链接保留 URL、表格线性化）。**纯文本回复（无 Markdown 语法）双平台逐字节不变**。出站文本统一按 Markdown 语义处理：宿主直推含成对 `*`/`_` 等显著字符的纯文本会被按强调消化（trait 契约已明示）。

### Added

- **robit-chatbot / 表格线性化投影（T1）**：`render_table`（空格补位伪表格，比例字体下原理性错位）替换为 `linearize_table`——两列表逐行 `键: 值`；N 列表逐记录 `首列 — 表头: 值 · …`（记录间空行，>5 列宽表记录内拆多行、续行不缩进）；空单元格跳过对应键值对；表头语义并入键值对不再单独输出。输出无管道符/分隔行残留，二次投影幂等（黄金字符串单测逐字节钉死，含 CJK 持仓真实场景）。`features.tables=true` 平台直通原始表格语法（源偏移切片，保留 `\|` 转义与行内语法），激活死标志。
- **robit-chatbot / 纯文本快速通道**：不含 Markdown 触发字符、行首尾无空白的文本逐字节直通投影（不做换行归一化）——系统通知（确认提示/命令回执/进度提示/错误通知）与已投影回复二次过出站守卫时逐位稳定，幂等由构造保证；既有长度守卫精确字节测试全部原样通过。
- **robit-chatbot / adapter 直调路径透明内容投影（T3）**：`prepare_outbound` 扩展为「内容投影 + 长度守卫」统一出站准备（先投影后分段——投影改变长度，顺序不可反）。宿主直调 `send_message` / `send_message_best_effort` 不调清洗器也自动获得平台可读投影，与 0.4.1 长度守卫透明封装完全同构；QQ/微信 adapter 零改动（发送咽喉已就位）。长表格直调 → 线性化 + 分段组合有单测覆盖。

### Changed

- **robit-chatbot / 纯文本平台也过投影（T2）**：`flush_buffer` 删除 `supports_markdown=false` 的整体跳过分支——清洗器定位是「平台可读性投影」而非「Markdown 渲染器」，all-false features = 纯文本投影，本就该跑。微信输出与 caps 宣称一致（caps 说纯文本，输出就是纯文本）；SDK `filter_markdown` 变成无害二次过滤（它「保留」的表格/栅栏/加粗已被投影掉）。微信纯文本回复逐字节不变（快速通道）；含 Markdown 语法的回复按预期改善：加粗/栅栏/标题标记去除、引用去 `> `、表格线性化、图片语法变 `[Image: alt]` 文本（SDK 此前将图片语法整体删除，现在信息反而得以保留）。
- **robit-chatbot / 降级尊重 features（QQ 改善性变化）**：`headings=false` 时标题降级不再恒输出字面 `**`——`bold=false` 输出纯文本 + 空行（`bold=true` 维持 `**` 兜底）；`blockquotes=false` 去除字面 `> `；`links=false` 由「丢弃 URL」改为 `text (url)`（信息保全，文本与 URL 相同或为空时去重）；`strikethrough=false` 剥除字面 `~~`（解析选项恒开、发射按 flag，与 bold/italic 同构）。QQ 纯文本回复逐字节不变，含上述语法的回复按描述变化（caps 诚实优先，既有单测全部回归通过）。列表标记 `- `/`N. ` 恒保留（纯文本自然约定，flag 为描述性）。
- **robit-chatbot / `PlatformAdapter::send_message` trait 契约**：「长度守卫契约」扩展为「出站准备契约（内容投影 + 长度守卫）」，明示出站文本按 Markdown 语义处理、纯文本快速通道与幂等保证，调用方无需自行预投影/预截/预分段。`supports_markdown` 字段语义改为描述性声明（投影恒跑，不再作为门控）。更正 adapter.rs 两处与 SDK 实际行为不符的注释（「SDK strips Markdown」——SDK 实际保留表格/栅栏/加粗）。
- **robit-chatbot / HardBreak 归一化**：行尾双空格硬换行统一输出 `\n`（行尾双空格是 Markdown 语法残留，IM 客户端不可见）。
- **全仓库 / 构建警告清理**：移除与 `repository` 冗余的 `homepage` 元数据（8 个 crate + workspace）；`[workspace.dependencies]` 不再登记无依赖方的 `robit-web` / `robit-wechat`（消除 cargo `unused_workspace_dependencies` 警告，bump 清单同步更新为 3 个，见 CLAUDE.md）；robit-gui 的 brotli 传递依赖 pin 经 `[lints.cargo]` 豁免 `unused_dependencies`（pin 本身保留）。clippy 机器可适用修复批量落地（冗余 `.into()`、`assert_eq!(x, true)`、`push('\n')`、Default derive 等，语义保持，全 workspace 测试回归通过）。涉及公共 API 稳定与有意设计的警告（`too_many_arguments`、lock.rs read-before-truncate、类型擦除 `new()` 等）按决策保留。

### Fixed

- **robit-chatbot / `blockquotes=true` 时引用标记丢失**：`in_blockquote` 置位后无消费点，支持引用的平台（未来飞书）也会丢失 `> ` 标记；现按行发射前缀（嵌套引用 `> > `），含幂等单测。
- **robit-chatbot / 表格单元格捕获泄漏边界**：单元格文本捕获由「从 output 剥回末段文本」hack 重构为事件路由（行内事件直接写单元格缓冲），修复单元格内 HardBreak 等残留泄漏到主输出的问题；单元格内行内代码/加粗/链接按 features 正确投影（含降级链接 `text (url)` 形态）。

## [0.4.1] - 2026-11-22

### Added

- **robit-chatbot / 出站长度守卫公开模块 `outbound`**：新增 `prepare_outbound` 函数及 `OutboundResult`、`MAX_OUTBOUND_SEGMENTS`、`TRUNCATION_MARKER` 等公开类型/常量，将原 `frontend.rs` 私有的分段/截断逻辑提取为框架级共享能力。非编辑平台自动分段（≤3 段、边界感知、代码栅栏保护），编辑平台截断 + 省略号，≤cap 文本幂等直通。
- **robit-wechat / adapter 直调路径透明长度守卫**：`send_text_inner` 在节流前调用 `prepare_outbound`，超长文本自动分段逐段发送，每段独立过 `SendLimiter`；部分失败时放弃余段并返回已发段的最后 msg_id。宿主直调 `send_message` / `send_message_best_effort` 现在与 chatbot 回复路径行为一致。
- **robit-qq / adapter 直调路径透明长度守卫**：`send_message` 在构造请求体前调用 `prepare_outbound`，QQ 作为编辑平台走截断路径，超长文本不再原样透传导致服务端拒绝或静默截断。

### Changed

- **robit-chatbot / `PlatformAdapter::send_message` trait 文档**：明确写入长度守卫契约——实现方必须按 caps 强制长度，调用方无需也不应再自截。
- **robit-chatbot / `frontend.rs` 分段逻辑去重**：`flush_buffer` 改为调用 `outbound::prepare_outbound`，删除原私有 `split_for_platform` / `truncate_to_max` 及辅助函数，规则单源化。既有单元测试迁移至新 API，行为逐位不变。
- **CI / 发版自动化**：`release.yml` 新增 `publish-crates` job，打 tag 后自动按依赖顺序发布 crates.io；删除独立的 `publish.ps1` / `publish.sh` 脚本。

## [0.4.0] - 2026-11-20

### Added

- **robit-chatbot / 通用出站节流模块 `throttle`**：`SendLimiter` 提供账号级发送预算（最小间隔 + 滑动窗口）、平台限流后的冷却（`note_throttled`）与可持久化快照（`snapshot` / `restore`），等待全程可被关停令牌取消；模块内不含任何平台错误码——何时限流、冷却多久、快照存哪、配额按什么粒度划分，全部由 adapter 决定，因此飞书（HTTP 429 + `Retry-After`）、Telegram（`429 retry_after`）等后续渠道可复用同一实现。微信侧 `[channels.wechat_bot.rate_limit]` 缺省即启用：最小间隔 2s、窗口 10 条/180s、反刷冷却 120s、必达消息排队上限 180s，参数全部可调，`enabled = false` 可完全退回旧行为。
- **robit-agent / `AgentError::RateLimited`**：新增瞬态平台限流错误变体（带 `retry_after_secs` 提示）与 `is_rate_limited()` 便捷判断，宿主终于能区分「分钟级自愈的服务端反刷/限流」与永久失败（token 失效、参数错误）。`AgentError` 同时标注 `#[non_exhaustive]`，后续新增变体不再构成破坏性变更。
- **robit-chatbot / `HintPolicy` 进度提示策略与「尽力而为」发送通道**：`PlatformCaps.progress_hints` 声明平台缺省（QQ / 飞书 `always`，微信 `off`）；新增 `PlatformAdapter::send_message_best_effort` 与 `PlatformSender::send_best_effort`（缺省实现等价于普通发送，现有平台零改动），配额紧张时提示类消息直接跳过，不再排队挤占必达消息的位置。

### Changed

- **robit-wechat / 出站反刷防护**：文本与媒体两路发送共用同一账号级预算（含宿主直调 adapter 的路径），`-2`（`errmsg="prepare failed"`）被识别为服务端反刷——返回 `AgentError::RateLimited`、启动全局冷却，并按「每次事故一条」输出包含「分钟级自愈 / 勿密集重试 / 重启无效」语义的 warn（而非每条失败刷一行）。发送预算快照写入 `wechat-state.json` 的 `outbound` 段，重启后冷却与窗口延续，避免「重启后第一波推送再次撞墙」。微信进度提示缺省改为 `off`（无原地编辑，每条提示都是独立消息且占用配额），可用 `[channels.wechat_bot].progress_hint = "best_effort"` 或 `"always"` 恢复。

### Fixed

- **robit-chatbot / 无编辑能力平台的长回复不再静默截断**：`flush_buffer` 此前对所有平台统一调用 `truncate_to_max`，微信这类不支持原地编辑的平台会直接丢掉超过 2000 字的部分（只在结尾补一个 `…`，用户无从知晓）。现在无编辑平台按「段落 → 行 → 句末 → 空白 → 硬切」的优先级把回复切成最多 3 段（每段不超过平台上限，且优先不切断代码块围栏），逐段顺序发送并受出站预算节流；仅当超过 3 段上限时才截断，末尾带显式的「……（内容过长，已截断）」标记。支持原地编辑的平台（QQ / 飞书）仍保持「单条消息编辑合并」的原有行为，并有回归单测钉死。前端模块文档同时补上了完整的出站清单（提示、进度分段、最终答案、确认链路、命令回执、媒体），便于核对哪些路径走「必达」、哪些走「尽力而为」。

## [0.3.1] - 2026-11-20

### Fixed

- **robit-wechat / `login` 子命令支持 Ctrl+C 取消**：此前 `robit-wechat login` 未注册信号处理，扫码等待期间按 Ctrl+C 只能走系统默认的信号终止，既无提示也不会报告取消原因。现在登录流程启动即安装 Ctrl+C 监听（首个 `await` 之前完成注册，启动阶段按下也不会漏），取二维码、轮询、刷新、重定向各步骤均可被打断，等待配对码输入时同样可中断（stdin 读取改为 `tokio::io::stdin` 异步读 + 取消选择，不再阻塞事件循环）；取消后打印「已取消微信登录（收到 Ctrl+C），未保存任何登录状态」并以非零状态退出，保证不会写出半成品状态文件。为此 `robit-wechat` 的 tokio 依赖补充 `io-std` / `io-util` feature。
- **robit-chatbot / robit-wechat / robit-qq / Ctrl+C 优雅退出**：三个问题一并修复。(1) 关闭信号由 `Arc<Notify>` 改为 `tokio_util::sync::CancellationToken`：`notify_waiters()` 是边沿触发、无人等待时信号直接丢失，而取消令牌是电平触发，取消发生在消息处理中也不会丢，微信长轮询 supervisor / QQ WS 任务同样受益。(2) `robit-wechat`、`robit-qq` 的 `main` 不再在 `select!` 命中 Ctrl+C 时丢弃 `manager.run()` 的 future（那样会中断正在处理的事件），改为 pin 住该 future、取消令牌后继续等待它返回；第二次 Ctrl+C 立即强制退出。(3) `ChatbotManager::run` 返回前关闭所有会话输入通道，并最多等待 15 秒让正在生成的回复自然收尾——此前每个会话的 Agent turn 是独立 `tokio::spawn` 任务，进程退出会在流式回复中途将其截断；超时后记录告警并放弃剩余 turn。注意 `ChatbotManager::run` 的 `shutdown` 参数类型随之从 `Arc<Notify>` 变为 `CancellationToken`（对外 API 变更，直接调用者需适配）。

## [0.3.0] - 2026-11-17

微信官方 iLink Bot 渠道接入。渠道选型调研（官方渠道 vs 商业协议代理）、认证模型与架构决策见 `docs/robit-plan-wechat-bot.md`。

### Added

- **robit-wechat / 微信 Bot 前端（新 crate）**：实现 `PlatformAdapter` trait，接入微信官方 iLink Bot 渠道（`ilinkai.weixin.qq.com` 长轮询收消息 + HTTP 发消息 + CDN 媒体 AES-128-ECB 解密），二进制 `robit-wechat`。协议层依赖社区纯协议 SDK `weixin-agent` v0.3（官方 npm 包 `@tencent-weixin/openclaw-weixin` v2.4.6 的等价移植，不耦合 OpenClaw）。认证为一次性扫码换长期 `bot_token`（`robit-wechat login`，支持配对码验证与 IDC 重定向，二维码编码 `qrcode_img_content` 而非轮询 token）；凭证、长轮询游标（`sync_buf`）与会话出站 context token 持久化于 `{working_dir}/.robit/wechat-state.json`，重启续收不丢消息。长轮询为出站连接，**个人电脑/内网可直接部署，无需公网回调**；渠道仅 1:1 私聊（`chat_id = "private:{ilink_user_id}"`）。token 失效（errcode -14）发 `Disconnected` 令进程退出并提示重新登录。
- **robit-chatbot / 附件契约支持 adapter 预填 `local_path`**：`MediaAttachment.local_path` 已由 adapter 填写时，manager 跳过通用 URL 下载、保留本地副本（微信 CDN 媒体为 AES 密文，必须经协议 SDK 解密落盘，通用下载会拿到密文）。QQ 路径行为不变（adapter 恒置 None）。
- **robit-ai / `[channels.wechat_bot]` 配置段**：`bot_token`（可选，支持 `${ENV_VAR}`，缺省读 `wechat-state.json`）、`base_url` / `cdn_base_url`（可选覆盖，一般无需配置）。
- **robit-chatbot / `PlatformCaps::wechat()` 与 `MarkdownFeatures::wechat()`**：纯文本渲染（SDK 出站自动过滤 Markdown）、不支持原地编辑、2000 字上限、媒体 20MB。

### Fixed

- **robit-chatbot / 会话标题 fallback 去平台硬编码**：`generate_title` 空消息兜底由硬编码 `"QQ 会话"` 改为 `"{platform} 会话"`（取自 `PlatformAdapter::platform_name()`），多平台档案标题不再张冠李戴。

### Changed

- **发版与分发接线**：`release.yml` 新增 robit-wechat 三平台产物（`robit-wechat-{linux,windows}-x86_64` / `robit-wechat-macos-aarch64`）；workspace 版本 bump 清单扩为 5 个内部 crate（`robit-wechat` 登记进 `[workspace.dependencies]`）；crates.io 发布脚本（publish.ps1/sh）纳入 robit-wechat。

## [0.2.1] - 2026-11-14

### Fixed

- **robit-agent / robit-chatbot / 附件标注行携带本地保存路径（M1 收尾）**：0.2.0 已把 `local_path` 回填进附件，但 `describe()` 不含路径——非图片附件（csv/xlsx 等数据文件）对模型「能发现、够不着」：只见 `[用户发送了文件: xxx.csv (50.0KB)]`，无从获知落盘位置，宿主领域工具（R2 拍板下替代系统 read 的消费路径）拿不到入口，且 M4/M5 的消毒与冲突后缀使落盘名可能不同于原始文件名、模型无法猜测。新增 `MediaAttachment::describe_with_local(base)`：有本地副本时标注追加 `, 本地路径: <path>`（相对 `working_dir` 显示，不在其下时给绝对路径）；agent 模型侧三处标注分支与 chatbot 档案/标题标注统一改用，标注仍恰一份。图片不受影响（vision 本地优先编码成功时不拼标注）。GUI/TUI 无本地副本，输出与 0.2.0 逐字节一致；`describe()` 无路径形态保留。至此 RFC M1+M2 验收（「发 csv → 模型能复述文件首行」）闭环：robit-qq 默认装配含系统工具 `read`（始终注册、免确认）与 `bash`（需确认），模型可凭标注中的本地路径直接 `read` csv 内容，xlsx 等二进制经 `bash` 解析；子木等受限装配宿主（R2 拍板排除 read/bash）则经领域工具按路径消费（ADR-0005 分工）。部署提示：`read` 免确认且白名单管不住系统工具，robit-qq 工作目录勿放密钥文件（`.env` 建议置于 `~/.robit/`）。

## [0.2.0] - 2026-11-14

来自子木 StockAgent 第四轮消费反馈（R10 附件批 M1–M7 + R8 宿主形态六件套 H1–H6）。评审与实现方案见 `docs/robit-plan-qq-media-and-host-feedback.md`，移交件见 `docs/robit-rfc-qq-media-and-host-feedback.md`。

> **升级提示（0.1.x → 0.2.0）**：含不兼容 Rust API 变更（`Agent::new`/`with_history`、`MediaAttachment`、`ToolContext`、storage 会话/消息函数、`download_and_encode_base64`、`PlatformAdapter` 等，详见下方 Changed 节）——直接调用者按「传 `None`/`""` 即旧行为」适配；经 `ChatbotManager`/`bootstrap` 接线的宿主无需改代码。配置文件与旧数据库完全兼容（schema 自动迁移 v5→v7）。

### Added

- **robit-agent / robit-chatbot / 附件本地路径回填（M1）**：`MediaAttachment`（agent 与 chatbot 两形状 + `From` 转换）新增 `local_path: Option<PathBuf>`。Bot manager 落盘成功后把保存路径回填进附件，Agent 与宿主领域工具从此可消费附件内容（此前 `download_media` 返回值被丢弃，非图片文件对模型只有一行描述）。
- **robit-agent / 本地优先编码（M2）**：多模态消息构建改为三级兜底——`local_path` 本地编码（新增 `encode_file_base64_with_mime`，优先用平台 `content_type` 而非扩展名推 MIME）→ url 下载编码 → 描述行降级。QQ CDN url 过期/断网时只要本地副本在盘仍可识图；同一 url 不再无条件二次下载。
- **robit-agent / 下载守卫（M6）**：`[app.context] max_media_bytes`（缺省 20MB，0 = 不限）统一约束落盘下载与 vision url 兜底两条路径——Content-Length 预检 + 流式累加，超限即拒（新错误 `MediaError::TooLarge`），不落盘、内存峰值不超上限。新增 `download_media_with_opts` / `MediaDownloadOptions`；`download_media` 保留为缺省守卫的便捷封装（`generate_image` 落盘同受保护）。
- **robit-chatbot / 宿主注入 API（H1）**：`ChatbotManager::inject_message(chat_id, text, source)`——按 chat_id 向活跃会话投递回合并留档，不过 Confirmer 关键词路由、不过斜杠命令解析（注入 `"/help"` 即字面文本进模型）；档案行以 `messages.source` 列标记来源（如 `"scheduler"`），与真实用户消息可区分；注入内容正常参与后续会话历史。死句柄自动重建重试一次，失败返回错误由宿主处置。schema v6：`messages` 加可空 `source` 列（旧行 NULL = 真实消息）。
- **robit-chatbot / 人格提示词注入（H6）**：`[app.bot] system_prompt`（字符串）/ `system_prompt_file`（路径，相对 working_dir 或绝对）——优先级 字符串 > 文件 > 既有 `agent.md` 查找。`PromptBuilder::with_custom_prompt` 与 `Agent::new`/`with_history` 新增 `custom_prompt: Option<String>` 参数；不注入时系统提示词与旧版逐字节一致。宿主人格资产不必再挤进 `<working_dir>/.robit/prompts/`。
- **robit-chatbot / media 保留策略（M7）**：`[app.bot.media] retention_days` / `max_files`（缺省均 0 = 关闭，维持现行为），由 cleanup 循环按 mtime 执行超龄删除与数量裁剪，长跑服务 media 目录有界。
- **robit-ai / 会话实例命名空间（H4）**：`[app.bot] instance_id`（缺省 `""`）。schema v7：`sessions` 加 `namespace` 列，唯一索引重建为 `(chat_id, source, namespace)`；`find_session_by_chat_id` / `list_all_sessions_by_chat_id` / `activate_session` / `insert_session` 全部加 source/namespace 维度。同 working_dir 起两个平台或同平台两实例不再互相劫持会话，`/switch`、`/list` 不串台；单实例缺省行为不变。

### Fixed

- **robit-chatbot / 附件标注重复（M3）**：平台侧（robit-qq）不再把 describe 拼进消息文本，标注职责单一化——模型侧由 Agent 统一标注恰一份，档案/标题由 manager 用同一 `describe` 实现补标注（纯附件消息不再空档案、标题不退化）。删除 chatbot 侧重复的 describe 模板，缺省名统一为 `image`/`file`（原 agent 版 `unknown` 不再使用；无名附件的档案文本随之微变）。
- **robit-agent / 落盘文件名消毒（M4+M5）**：`download_media` 落盘名取 `file_name()` 单分量、滤控制字符与 `/` `\` `:`，杜绝路径穿越；同名冲突追加 `-1`/`-2` 后缀不再静默覆盖；无名/无扩展名附件按 `content_type` 补扩展名（uuid 名不再丢失 MIME 推断）。
- **robit-chatbot / cleanup 实装与死句柄自愈（H3）**：`cleanup_loop` 不再是 no-op——每 tick 剔除死句柄（`/exit` 后残留）、回收空闲会话（`session_timeout_minutes`，DB 记录保留可复活）；`AgentHandle` 与前端共享 `busy` 标志，长回合/确认等待中的会话绝不被误杀。用户消息发送失败时自动剔除死句柄并从 DB 重建会话重发一次，再失败给用户明确回执；`save_user_message` 移到发送成功之后，消除「档案里有、模型没看见」。
- **robit-chatbot / 落库 source 参数化（H2）**：`PlatformAdapter` 新增 `platform_name()`，`insert_session` 不再硬编码 `"qq"`，第二平台接入时档案来源真实。
- **robit-agent / memory 工具跟随 global_storage（H5）**：`ToolContext` 新增 `memory_dir`（Agent 构造时从 `MemorySettings.dir` 解析，已尊重 `global_storage`），memorize/recall/forget/list_memories 四处硬编码 `resolve_db_path(&ctx.working_dir, false)` 改为 `ctx.memory_dir.join("robit.db")`——宿主开全局存储时记忆工具与会话库同库；缺省路径逐字节不变。

### Changed

- **签名演进（仓内调用点已同步）**：`Agent::new`/`with_history` 末尾追加 `custom_prompt: Option<String>`；`insert_message` 追加 `source: Option<&str>`；`insert_session` 追加 `namespace: &str`；`find_session_by_chat_id`/`list_all_sessions_by_chat_id`/`activate_session` 追加 `source`/`namespace`；`download_and_encode_base64` 追加 `max_bytes`。经 `ChatbotManager`/`bootstrap` 接线的宿主不受影响；直接调用上述 API 的消费者按新参数适配（传 `None`/`""` 即旧行为）。
- **schema 迁移**：v5 → v6（messages.source）→ v7（sessions.namespace + 索引重建），顺接既有迁移链，旧库自动升级；各配迁移测试。

## [0.1.29] - 2026-11-08

### Fixed

- **robit-chatbot / ThinkingDelta 日志刷屏（子木 StockAgent 消费反馈）**：Bot 前端此前对每个思考增量打一条 trace 日志（`ThinkingDelta received (not rendered): … len=N`），思考型模型的长思考流会灌满日志文件。改为事件臂内仅原子计数（`deltas` / `bytes`），回合收口（`TurnComplete` / `Error`）时汇总输出一条 trace：`thinking deltas received (not rendered): chat_id='…', deltas=N, bytes=M`。级别与其它事件臂保持一致（trace）；无思考增量的回合不输出。

## [0.1.28] - 2026-11-06

来自子木 StockAgent 消费反馈的思考过程输出能力（R7，含 R6 空白帧了断）。设计记录见 `docs/robit-design-thinking-output.md`。

### Added

- **robit-ai / 流式双路径与思考增量捕获**：模型级新配置 `thinking = "auto" | "on" | "off"`（默认 `auto`）。`auto` 走 async-openai 传统 typed 流式路径，请求与解析行为与旧版本字节级一致；`on`/`off` 走自建 SSE 旁路（reqwest + eventsource-stream，标准字段仍委托 async-openai 类型反序列化）：请求体注入 `enable_thinking`（DashScope 兼容面拼写，typed 请求结构装不下该厂商扩展字段），响应侧逐帧捕获 `delta.reasoning_content`（async-openai 的流式 delta 类型无此字段也无未知字段兜底，serde 在其内部反序列化时静默丢弃；上游对该类字段政策性拒收，PR #525 等三次尝试均未合并）。新增公开类型 `StreamChunk`（`response` + `reasoning`，`Deref` 到 async-openai 帧类型保持既有访问路径）与 `RobitChatStream`；`LlmError::from_sse_payload` 复用既有语料分类器（内容审核/网关 HTML/提供商错误），流内 `{"error":…}` 帧与 HTTP 错误的友好提示行为不变。wiremock 集成测试 6 项 + 帧解析单测 9 项。
- **robit-agent / `AgentEvent::ThinkingDelta(String)`**：思考增量事件，在流循环中与 `TextDelta` 同点发射（思考帧先发）。展示性单向输出：不写回 `session.history`、不计入响应文本与 token 估算。无显式结束标记，消费方在 `TurnComplete` 收口。投影逻辑抽为纯函数 `chunk_deltas` 并配交错矩阵单测（子木验收 2 的等价形态）。
- **robit-web / `thinking_delta` wire 帧**：`WebEvent::ThinkingDelta { text }` → `{"type":"thinking_delta","text":"…"}`，additive 变更（穷举 match 的宿主编译期可见；只认已知 type 的 JSON 消费方向后兼容）。golden 测试与 `docs/protocol.md` 同步。
- **robit-gui / 思考过程渲染（唯一渲染启用面）**：`UiEvent::ThinkingDelta` 经事件桥独立 50ms 批处理送达 React；新增 `ThinkingPanel` 组件——思考阶段自动展开流式灰字（替代空转的 thinking indicator），正文/工具事件到达自动折叠，回合结束定稿可点击回看；下一条用户消息发送时清空。思考文本不写 SQLite（无 schema 迁移），与框架「展示性单向」语义一致。

### Changed

- **空白 `text_delta` 帧收口（子木 R6）**：`content` 为空串的增量不再发射 `TextDelta`（此前 provider 思考阶段的 `content:""` 外壳帧会原样上行成空白帧）。收口在 agent 发射端，TUI/GUI/Bot/Web 全前端一次性生效；Web 消费方已有的空白帧守卫变为惰性，可移除亦可保留。
- **robit-ai `chat_stream` 返回类型**（对直接调用 robit-ai 流式 API 的消费者为 breaking surface；经 robit-agent/robit-web 消费的宿主不受影响）：`ChatCompletionResponseStream` → `RobitChatStream`，Item 为 `Result<StreamChunk, LlmError>`（错误已在源头分类，不再需要调用方套 `from_openai_error`）；`StreamChunk` 经 `Deref` 保持 `chunk.choices` / `chunk.usage` 等既有访问形态。
- TUI / robit-chatbot（QQ 等 Bot 平台）对 `ThinkingDelta` 显式忽略（本版不渲染思考过程；Bot 面因平台消息长度限制暂不启用）。

## [0.1.27] - 2026-10-31

来自子木 StockAgent 消费反馈的宿主形态适配（R2–R5，R1 已随 robit-web 0.1.26 闭环）。均为通用形态能力，不涉及领域逻辑；现有 `bootstrap()` / `init_logging()` / `load_skills()` API 保持兼容。

### Added

- **robit-agent / bootstrap 组合形态（R2）**：新增 `bootstrap_with(config, working_dir, base_tool_names, options)` 与 `BootstrapOptions`——追加式装配一步到位，宿主不再需要在「全部标准工具」与「完全手搓 `ToolRegistry`」之间二选一，也不必复刻 bootstrap 编排。`BootstrapOptions` 三项能力：`extra_tools`（领域工具追加注册，注册在标准工具之后、可按名覆盖；工具名自动并入 `SkillRegistry` 校验 `tools_required` 的可用工具清单）、`standard_tools`（工具白名单，覆盖 `app.enabled_tools`——server 宿主可组合出「系统工具（read/load_skill 等，不含 bash）+ 领域工具」的装配）、`extra_skill_dirs`（见 R4）。配套新增 `create_tools_from_config_with`（白名单的程序化入口）与 `ToolRegistry::register_boxed`（注册 `Box<dyn Tool>`）。`BootstrapOptions::new()` 时 `bootstrap_with` 与 `bootstrap` 行为完全一致。白名单语义明确为两类工具：**系统工具**（`read`/`load_skill`/`search_history`/`query_task` 与记忆工具）始终注册、不受白名单控制（记忆工具仍由 `memory_mode = "tools"` 决定），白名单只控制**通用工具**（`bash`/`write`/`edit`/`ls`/`find`/`grep`/`generate_image`/`read_image`）——与 `enabled_tools` 配置的既有行为一致。
- **robit-agent / 额外技能目录（R4）**：新增 `load_all_skills_with(working_dir, extra_skill_dirs)` 与 `load_skills_from_dirs(dirs)`（通用形式，后者覆盖前者同名技能）。技能作为产品资产随仓库版本化（如仓库根 `skills/`）的宿主有了官方入口，不再需要在宿主侧抄一遍 loader 的目录逻辑；`bootstrap_with` 经 `extra_skill_dirs` 直通。坏文件仍为非致命逐条上报。
- **robit-ai / 日志可组合原语（R3）**：新增三个公开 API，宿主可单一初始化路径拿到「自己的 fmt 格式 + robit 文件日志 + 全 workspace crate 可见」，不再需要按「有无 robit 配置」二选一分支、手工拼 `"{crate}={level}"` directive：`build_log_filter(app_config, host_targets, additional_directives)`（`EnvFilter`——`host_targets` 声明宿主 workspace crate 清单，robit 三 crate 始终同级别放行，`RUST_LOG` 仍整体优先）；`log_file_layer(app_config, working_dir, filter)`（robit 文件日志层：本地日期轮转、无 ANSI、保留期清理；未启用或打开失败返回 `None`，`Option<L>` 自身实现 `Layer` 可直接 `.with(...)`）；`install_panic_hook()`（panic 经 tracing 落日志后链回原钩子，须在 `.init()` 后调用）。`init_logging` / `init_logging_silent` 保留为便捷封装，行为不变。
- **robit-agent / anyhow 互操作（R5）**：`AgentError` 新增 `External(#[from] anyhow::Error)` 变体、`ToolResult` 新增 `From<anyhow::Error>`（含 `{:#}` 整条 cause 链）。宿主工具的 `execute` 里可直接 `?` 返回 anyhow 错误——robit 拥有这两个类型故 impl 合法，宿主不再需要自写 `store_try!` 之类转换宏。推荐模式（见 `docs/architecture.md`）：错误信息对模型有转发价值时 `?` 直接传播；细节不该喂给模型时（如 SQL 原文）宿主自行记日志并返回脱敏的 `ToolResult::error(…)`。

## [0.1.26] - 2026-10-27

### Added

- **robit-web**：新增 Web 前端桥 crate（来自子木 StockAgent P2-T3 SSE 桥的通用化沉淀，应两工程共同演进需求 R1）。`WebEvent` 为 `AgentEvent` 提供官方 JSON 序列化 schema（内部 `type` 标签、snake_case 变体，如 `{"type":"text_delta","text":"…"}`），作为 Web 客户端（JS/TS）与 robit 之间的稳定 wire 契约；`WebFrontend` 是通用 `Frontend` 实现——事件经 `tokio::sync::broadcast` 扇出（多客户端可同时观看，慢消费者 `Lagged` 由宿主透出而非断流），工具确认改为推送/回填握手：`request_tool_confirmation` 挂起在单槽位 oneshot 上并推送 `confirm_request` 事件，宿主调用 `WebBridge::resolve_confirm(tool_call_id, approved)` 后 Agent 继续；responder 被 drop（新请求顶替、`reject_pending_confirms`、桥销毁）一律按拒绝处理。`WebBridge` 承载宿主接线：消息注入（`send_user_text` / `send_message`，Agent 循环退出后返回 `FrontendGone`）、`subscribe()` 订阅事件流、`take_message_rx()` 交出接收端给 `Agent::run`。SSE/WS 编码、keep-alive 与传输层关注点刻意留在宿主，crate 零 HTTP 框架依赖；`connected` 等宿主元数据帧由宿主自行序列化后并入流。单会话一桥，多会话宿主按会话各建一桥。单元测试 9 项（事件投影 ×3、确认握手 ×4、注入 ×1、扇出 ×1）。

## [0.1.25] - 2026-10-14

### Added

- **robit-agent / robit-ai**：新增 `read_image` 识图工具。主对话模型不支持图片输入（`supports_images = false`，如 deepseek-v4-pro）时，`read` 读图片只能拿到文件名/大小的占位文本；配置顶层 `default_vision_model = "provider/model"`（指向普通 `providers` 段中 `supports_images = true` 的多模态模型，如 `qwen/qwen-vl-max`）后，`read_image` 工具自动注册——Agent 传入图片路径与可选识别指令 `prompt`（如"提取图中文字"，缺省为详细描述），工具内部将图片（经 `max_image_dimension` 压缩）发送给该多模态模型，把文本描述返回给主模型。主模型支持图片时该工具不注册（`default_vision_model` 被忽略并打 info 日志）。配套变更：`resolve_profile` 的 provider/model 查找逻辑提取为共享的 `resolve_provider_model`；新增 `resolve_vision_model`（支持精确到 "provider/具体model_id"，不受"显式 provider 名只取第一个 model"的限制）与 `LlmClient::from_resolved` 构造入口。

### Fixed

- **构建**：发版 CI 的 GUI 三平台构建失败——`tauri-plugin-shell`（Rust crate 2.4.0）与 CI 全新安装的 npm `@tauri-apps/plugin-shell`（2.5.0）major/minor 不一致被 Tauri CLI 拒绝。修复分三层：`cargo update` 对齐 Rust 侧至 2.5.0（tauri 2.12.3 等依赖同步更新）；`package-lock.json` 重新生成（旧 lock 混有内网 Nexus 源 URL，CI 不可达，这也是此前 workflow 删 lock 全新安装的原因），全部 resolved 指向公共 npm 源，`@tauri-apps/plugin-shell` 在 package.json 中钉为精确版本 `2.5.0`；release.yml 的 GUI 依赖安装改用 `npm ci` 严格按 lock 安装，杜绝"未改代码却因依赖漂移挂发版"的静默失败。

## [0.1.24] - 2026-09-30

### Added

- **robit-agent**：`generate_image` 工具新增 4 个可选参数：`size`（输出分辨率，`宽*高` 格式，如 `1696*960`）、`negative_prompt`（反向提示词）、`prompt_extend`（提示词智能改写开关，遇 `IPInfringementSuspect` / `DataInspectionFailed` 审核报错时可设 `false` 重试）、`seed`（随机种子，范围 [0, 2147483647]，越界直接返回参数错误）。参数经 `extra_params` 管道透传：DashScope 协议合并进 `parameters`，OpenAI 兼容协议合并进请求体顶层；全部省略时请求体与之前完全一致，不影响现有行为。

### Changed

- **robit-agent / robit-ai**：记忆机制重构。新增 `[app] memory_mode` 配置（`tools` / `file` / `off`），**默认 `file`**——未配置的用户升级后 SQLite 记忆工具（`memorize` / `recall` / `forget` / `list_memories`）不再注册，自动切换为文件记忆机制：记忆目录（`.robit/memory/`，启用 `global_storage` 时为 `~/.robit/memory/`）下由 Agent 通过 `write` / `edit` 工具维护主记忆文件 `memory.md` 与每日记忆文件 `memory-YYYY-MM-DD.md`，机制说明与主记忆内容（上限 16KB，超限截断）随系统提示词注入上下文；`memory.md` 内容中的字面占位符文本不受模板替换影响。需要原 SQLite 记忆工具的用户请显式配置 `memory_mode = "tools"`；已有 `memories` 表数据不迁移、不删除。Bot 平台各聊天共享同一份 `memory.md`。
- **robit-agent**：记忆跟进修复。每日记忆文件名与系统提示词 `{date}` 改用**本地时区**日期（复用日志轮转的 `local_utc_offset`，无法确定时回退 UTC），UTC+8 用户在本地 0-8 点不再落到"昨天"的文件；DB 时间戳（created_at/updated_at）保持 UTC 不变。`memory_mode = "file"` 但 `enabled_tools` 未包含 `write` / `edit` 时，启动输出告警提示记忆将无法持久化（补齐工具或改设 `memory_mode = "off"`）。

## [0.1.23] - 2026-09-29

### Added

- **robit-agent**：图片进入上下文前自动压缩。`read` 图片文件与用户图片附件在 base64 编码前，若最长边超过 `[app.context].max_image_dimension`（默认 1024，0 禁用）则等比缩放（Lanczos3）并重编码为 JPEG（质量 85），透明通道合成白底；2K PNG 单图体积可降低一个数量级，配合 0.1.23 的"图片用后即弃"进一步保障长会话请求体不超网关限制。GIF（动图）直通不压缩；重编码后反而变大（已高度优化的图）时保留原字节。工具结果文本会标注压缩信息（如 `2048×2048 → 1024×1024 JPEG`），模型可据此判断可用细节量。解码前施加防护上限（单边最大 16384px、解码总分配 256MB），超高分辨率"解码炸弹"图片直接报错而非耗尽内存。注意：附件为未启用解码器的格式（BMP/TIFF/SVG 等）时降级为文本描述；压缩会忽略 EXIF 方向信息。

### Fixed

- **robit-agent**：修复会话历史中 base64 图片随每次 LLM 调用全量重发，长会话累积后请求体超出服务商网关限制（HTTP 413 Request Entity Too Large，openresty 拒绝）。`read` 图片等返回图片的工具结果、用户图片附件此前以多模态 user 消息注入历史且永不清理，多张 2K PNG 的 base64 累积达几十 MB；而服务商对每张图片仅计约 1k token，token 估算与截断对此完全无感。现在图片在成功发送给模型一次后即降级为文本占位符（`[历史图片已省略：N 张图片已发送给模型...]`），每张图片只随紧随其后的一次调用发送，后续调用不再重复携带图片数据。图片消息本就不持久化到 DB，重启后历史为纯文本，存量会话无需迁移即可正常加载。
- **robit-ai**：服务商反向代理（openresty 等）对超限请求返回 HTML 错误页，async-openai 只能报出裸的 JSON 反序列化失败（`failed to deserialize api response: error:expected value at line 1 column 1 content:<html>...`），真实原因被掩埋。现在检测 HTML 载荷并提取 `<title>`，映射为友好提示：413 会说明常见原因是上下文中图片过多或过大并建议清空上下文/新建会话，其他网关错误（如 502）给出稍后重试的通用建议。

## [0.1.22] - 2026-09-21

### Added

- **robit-ai**：服务商错误友好提示。LLM 请求与流式响应中的提供商错误现在映射为清晰的中文提示：已知内容审核错误码（DashScope `data_inspection_failed`、OpenAI 系 `content_filter`/`content_policy_violation`）区分输入侧/输出侧拦截——输入侧（对话上下文被判定不适宜，该会话后续请求会持续失败）提示清空上下文或新建会话，输出侧提示调整提问或稍后重试；其余错误码显示 `服务商返回错误（code）：message`，载荷无法解析时保持原始错误，服务商原始消息始终保留供排查。此前流中段推送的 `{"error": ...}` 事件（无 `choices` 字段）只报出裸的 JSON 反序列化失败，HTTP 400 拒绝只显示 `400 Bad Request ...`，真实原因埋在载荷里。

### Fixed

- **robit-agent**：修复 QQ Bot 执行 `/new` 后报 `UNIQUE constraint failed: sessions.chat_id`，且该聊天后续所有消息均无法开启会话。`sessions` 表唯一索引 `idx_sessions_chat_id` 此前只排除 `chat_id IS NULL`、未排除 `is_active = 0` 的归档行，等于限制同一 `chat_id` 终身只能有一条记录，与"归档旧会话 + 新建会话"的设计（`/list`、`/switch`）冲突。索引放宽为仅约束活跃会话（`WHERE chat_id IS NOT NULL AND is_active = 1`，每个聊天同时最多一个活跃会话，归档记录不限量）；schema v4 → v5 自动迁移重建索引，存量受影响的库启动时自动修复，历史会话与消息完整保留，无需删库。

## [0.1.21] - 2026-09-20

### Fixed

- **robit-agent**：修复并行图片工具调用触发 400 `insufficient tool messages following tool_calls message`（DeepSeek 等严格校验的提供商）。`read` 图片文件等返回图片的工具结果此前在工具执行循环内逐个注入多模态 user 消息，并行调用批次中该消息被插在 tool 响应中间，违反 "assistant(tool_calls) 后必须紧跟每个 tool_call_id 的 tool 响应" 的协议约束。现在图片 user 消息在整批工具结果之后统一注入，保证序列为 `assistant(tool_calls) → tool×N → user(图片)`。
- **robit-ai**：新增 `repair_interleaved_tool_responses` 历史修复，在 `chat_stream`/`chat` 发送前（`repair_tool_pairing` 之前）执行：把夹在 `tool_calls` 与 tool 响应之间的非 tool 消息移到批次末尾，保持相对顺序。DB 加载的历史、旧版本写出的交错序列在每次 API 调用前自动被修复，无需迁移存量数据。

## [0.1.20] - 2026-08-23

### Changed

- **robit-agent**：系统提示词不再注入工具清单（移除 `system.md` 的 `Available Tools` 段与 `PromptBuilder` 的 `tools` 参数）。工具定义完全通过 OpenAI function calling 的 `tools` 请求参数下发（name/description/参数 JSON Schema），消除与 `tools` 参数重复的文本描述，节省每轮上下文 Token。

### Fixed

- **robit-agent**：`tools` 请求参数现在仅在模型支持 function calling 时下发（`supports_tools` 配置项生效，此前该标志定义了但从未被使用），不支持该参数的 OpenAI 兼容端点不再收到无法处理的请求。注意：`supports_tools` 默认为 `true`（保持既有行为不变），不支持工具调用的模型（如 DeepSeek reasoner）需在配置中显式设置 `supports_tools = false`。

## [0.1.19] - 2026-08-17

### Added

- **robit-agent**：Token 校准计数。每次 API 响应后用 `usage.prompt_tokens` 作为精确基线回填（`last_known_prompt_tokens` + `snapshot_message_count`），之后仅对新增消息做增量估算；历史被截断/压缩时自动失效回退启发式估算。零新依赖，显著降低上下文 Token 估算误差，避免过早触发截断。
- **robit-ai**：流式请求携带 `stream_options.include_usage = true`，使流式响应也能返回 `usage` 统计（OpenAI/DeepSeek 均支持），支撑上述 Token 校准。
- **robit-gui**：`AgentEvent::Error` 现在以系统消息形式展示在会话中（此前仅打印到浏览器 console，前端无任何错误反馈）。

### Changed

- **日志精简**：每步无变化的上下文估算/截断检查（`estimate_context_tokens`、`maybe_truncate`）降为 TRACE；删除逐流式 chunk 日志（此前每次 LLM 调用刷数十至数百行）；同一次工具调用的多条重复日志合并为一条 INFO（`Executing tool`），工具注册表不再每次调用 dump 全量工具列表；`repair_tool_pairing` 的孤儿消息告警改为每次调用聚合一条 WARN（此前每条孤儿消息每次 API 调用都 WARN）；可能超长的工具参数（如 `write` 的文件内容）进日志前截断。生图模块（`image_gen`）的连接详情/任务提交/任务成功降为 DEBUG（Agent 的 `[async]` 日志已覆盖），轮询中的 `still running` 降为 TRACE（此前每 3 秒一条 INFO），INFO 不再出现脱敏 API key。

### Fixed

- **robit-ai**：新增 `repair_tool_pairing` 历史修复，在 `chat_stream`/`chat` 发送前执行：丢弃没有前置 `assistant(tool_calls)` 声明的孤儿 `tool` 消息，为缺失响应的 `tool_calls` 合成占位 `tool` 结果。修复 GUI 恢复历史会话时报 400 `Messages with role 'tool' must be a response to a preceding message with 'tool_calls'`（DeepSeek 等严格校验的提供商）。
- **robit-gui**：GUI 关闭后不再残留 `.robit/LOCK` 锁文件。Tauri 退出时以 `process::exit` 终止进程、不执行 main 的析构函数，`DirectoryLock` 的 RAII 释放从未运行。改为在 `RunEvent::Exit` 事件中显式释放锁（解锁并删除 LOCK 文件）。
- **robit-gui**：会话持久化补齐工具调用结构。此前 `ToolCallRequested` 只存 `tool` 消息、不存携带 `tool_calls` 的 assistant 消息，且 `tool` 消息的 `content` 一直停留在调用参数而非实际输出，导致恢复的历史出现孤儿 tool 消息。现在每次工具调用前插入对应的 `assistant(tool_calls)` 消息，工具完成时回写真实输出；前端跳过空内容 assistant 消息的渲染。
- **robit-agent**：不支持图片的模型（如 DeepSeek）现在会在每次 API 调用前净化历史（`sanitize_history_for_model`），移除 `image_url` 内容块，避免 400 `unknown variant 'image_url'`。

## [0.1.18] - 2026-08-11

### Added

- **robit-ai**：日志改用本地时间。时间戳和每日日志文件名（`robit-YYYY-MM-DD.log`）按系统本地时区输出（如 `2026-08-11T10:19:01.790373+08:00`、本地午夜切文件），取不到本地偏移时回退 UTC。
- **robit-ai**：新增 `app.log_retention_days` 配置（默认 14 天）。启动时扫描日志目录，删除超过保留期的 `robit-*.log`；`0` 禁用清理。此前日志按天生成但永不删除、无限累积。
- **robit-ai**：安装 panic hook，将 panic 信息经 `tracing::error!` 写入日志文件再链到默认 stderr hook。此前 detached 运行的二进制（nohup/systemd 下的 robit-qq 等）panic 只进 stderr，日志文件无痕迹，故障表现为无声停滞。

### Fixed

- **robit-qq**：修复重连 supervisor 的 `JoinHandle polled after completion` panic。`tokio::select!` 消费了先结束任务的 JoinHandle 后又 re-await 同一个句柄触发 panic；panic 只进 stderr，supervisor 静默死亡，导致每次 ~30 分钟网关轮换后不再重连、机器人掉线（"该机器人未连接服务"）。改为只 await 未被消费的另一个句柄。
- **robit-gui**：GUI 产物名长期停在 `Robit_0.1.1_*`。移除 `tauri.conf.json` 中硬编码的 `version` 字段，Tauri 改从 Cargo.toml（经 `CARGO_PKG_VERSION` 跟随 workspace）读版本，产物名与运行时版本显示均正确。

## [0.1.17] - 2026-08-10

### Fixed

- **robit-qq**：WebSocket 重连不再重启进程。QQ 网关约每 ~30 分钟要求重连，此前适配器发出 `Disconnected` 事件导致 `ChatbotManager` 退出、进程被外部 supervisor 重启，所有进行中的异步任务（如生图）被中途丢弃。现在重连逻辑收进适配器内部的后台 supervisor：dispatch / heartbeat 任一退出时取消另一个并按指数退避重建 WebSocket，不再发出 `Disconnected`，manager 与其持有的 Agent 跨重连存活。
- **robit-agent**：`image_gen::truncate_str` 按字节下标切片字符串会 panic（`end byte index 2000 is not a char boundary; it is inside '着'`），日志记录含中文的 DashScope 响应体时触发。改用 `floor_char_boundary` 在最近的字符边界切割。
- **robit-agent**：`AsyncTaskRunner::submit` 无 panic 兜底——work future panic 会跳出 `select!`、跳过 `done_tx.send`，导致 Agent 永远收不到结果、`query_task` 永久 pending。现在用 `AssertUnwindSafe(...).catch_unwind()` 捕获 panic，记录 ERROR 日志并投递 error 结果给 Agent。

> 上述后两条共同导致了「万象生图请求被拦截无响应、智能体得不到超时提醒」的现场现象：截断 panic 发生在异步生图任务里，而该任务无 panic 兜底，于是静默死掉、永不回报。

[Unreleased]: https://github.com/leexgone/robit/compare/v0.1.24...HEAD
[0.1.24]: https://github.com/leexgone/robit/compare/v0.1.23...v0.1.24
[0.1.23]: https://github.com/leexgone/robit/compare/v0.1.22...v0.1.23
[0.1.22]: https://github.com/leexgone/robit/compare/v0.1.21...v0.1.22
[0.1.20]: https://github.com/leexgone/robit/compare/v0.1.19...v0.1.20
[0.1.19]: https://github.com/leexgone/robit/compare/v0.1.18...v0.1.19
[0.1.18]: https://github.com/leexgone/robit/compare/v0.1.17...v0.1.18
[0.1.17]: https://github.com/leexgone/robit/compare/v0.1.16...v0.1.17
