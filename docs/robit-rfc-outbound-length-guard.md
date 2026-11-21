# robit 需求文档 · 出站长度守卫透明封装（子木第六轮移交 R16）

- **来源**：子木 StockAgent · `docs/07-backlog.md` **R16**（2026-11-21 登记）
- **日期**：2026-11-21
- **读者**：robit 维护者（本文档随取随用，可直接进 robit 仓库评审）
- **版本基准**：robit **0.4.0**（`main` @ `12e9cf9`）——全部行号已于本日在 0.4.0 源码逐条复核
- **影响面**：`robit-chatbot`（frontend.rs / adapter.rs）、`robit-wechat`（platform.rs）、`robit-qq`（platform.rs）；飞书等其余平台 adapter 同款排查
- **性质**：框架侧缺口与诉求，方案选项列出后**由 robit 拍板**；不含任何宿主领域逻辑（协作原则见子木 ADR-0005：框架级缺口先改 robit，宿主不写绕过补丁）。**子木用户已拍板架构方向：长度处理应在 robit 侧透明封装，子木不在宿主侧复刻分段器。**

> 回路记录：第五轮 R14 → v0.4.0（出站节流 + HintPolicy + 长回复分段 + RateLimited 语义）。本件是 v0.4.0 分段能力的**覆盖面收口**：分段做进了 chatbot 回复路径，但 adapter 直调路径漏了——同一 adapter 两个入口行为不一致。子木 P7-T8（chat 屏手动问答推送）阻塞于此，robit 发版后子木消费官方直调面完成。

## 1. 缺口：caps 宣称长度上限，直调发送路径不强制

### 1.1 同一 adapter 的两个入口，长度处理不一致（0.4.0 @ 12e9cf9）

| 入口 | 代码 | 长度处理 |
|---|---|---|
| **chatbot 会话回复** | `robit-chatbot/src/frontend.rs:240-338` `flush_buffer` | ✅ 完整：`supports_markdown` 时清洗（:256-260）→ 编辑平台 `truncate_to_max`（:267，私有 fn :545）/ 非编辑平台 `split_for_platform` 分段 ≤3 段（:302-303，私有 fn :577）→ 逐段发送 |
| **宿主直调 `adapter.send_message`** | `robit-wechat/src/platform.rs:633` → `send_text_inner`（:308） | ❌ **零处理**：节流 reserve 后**原样透传** `client.send_text(&peer, text, token)`——无长度检查、无截断、无分段 |
| 同上 · QQ | `robit-qq/src/platform.rs:328` `send_message` | ❌ 同样零长度处理（全文件无 `max_message_length` 消费点） |
| 同上 · 媒体/ best_effort | `robit-wechat platform.rs:643`（best_effort）/ `:673`（media） | best_effort 同走 `send_text_inner` 无守卫；媒体是文件路径不涉及文本长度 |

而 caps 是**公开宣称的契约**：`PlatformCaps::wechat()`（`adapter.rs:152`）`max_message_length = 2000`、`supports_edit = false`；`PlatformCaps::qq()` 同为 2000。宿主按 caps 理解「这个平台单条消息上限 2000 字」，但直调发 10000 字会**原样出站**——服务端拒绝、静默截断还是别的什么，robit 未验证、行为未定义。

### 1.2 为什么这是框架缺口而不是宿主使用不当

1. **节流已经下沉 adapter 了**（v0.4.0 W1 的正确做法）：`SendLimiter` 挂在 `WechatPlatformAdapter` 内部（`send_text_inner` 单一咽喉，platform.rs:308），任何调用方——chatbot 回复、宿主直调推送——都过账号级预算。**长度守卫却留在了 frontend 层**，同一 adapter 的出站保障不完整、不对称。
2. **分段器是私有的**：`split_for_platform`（frontend.rs:577）及其助手 `split_head`（:612）/ `byte_index_of_char`（:639）/ `boundary_candidates`（:647）/ `inside_code_fence`（:667）、`truncate_to_max`（:545）、常量 `MAX_OUTBOUND_SEGMENTS`（:564）/ `TRUNCATION_MARKER`（:567）全部 crate 私有，宿主想复用同一套边界规则也无门。
3. **宿主被迫各自补偿**：子木现以宿主侧 `clip_by_lines` 按行截断兜底（预警/摘要推送）；每个直调消费方都要自己发明一套长度处理，规则各异、与 chatbot 回复行为漂移——正是「框架级重复劳动」的典型形态（R1 Web 桥、R12 媒体消毒同款）。

### 1.3 触发场景（子木 P7-T8，真实需求）

子木 chat 屏新增「把最后一条推送给我」：把会话档案里最后一条助手回答（复盘长文可达数千字）经 `push_to_bound` 直调推给绑定的微信/QQ 会话。用户拍板**超长分段不截断**（与 chatbot 回复同款体验）。当前直调面做不到——要么宿主复刻分段器（用户已否决：不重复开发），要么等 robit 透明封装。

## 2. 诉求批 L1–L3

### L1 【核心】adapter 直调发送路径透明长度守卫

- **诉求**：`send_message` / `send_message_best_effort` 直调路径按 caps 自动处理超长文本，**任何调用方免费获得与 chatbot 回复一致的长度保障**：
  - **非编辑平台**（微信）：自动分段——沿用 `split_for_platform` 同款预算与边界规则（≤3 段、`\n\n` > `\n` > 句末（。！？；.!?;）> 空格 > 硬切、窗口前 1/3 边界拒绝、代码栅栏保护、尾段 `TRUNCATION_MARKER`）。
  - **编辑平台**（QQ）：至少强制 cap（截断 + 显式标记，`truncate_to_max` 同款）；是否也分段由 robit 拍板（QQ 单条上限语义与微信不同，编辑合并模型下分段会破坏「单消息编辑」预期——**保守方案 = 截断**，不劣化现行为）。
- **层位选项**（robit 定）：
  - **A. 共享助手 + 各 adapter 接线**：分段/截断器挪到 robit-chatbot 公开位置（如 `robit_chatbot::outbound` 模块），各平台 adapter 的 send 咽喉调用。规则单源，平台差异（编辑/非编辑）由 caps 驱动。
  - **B. 仅公开分段器**：`split_for_platform` 等公开导出，adapter 不动，宿主自行分段后逐段直调。改动最小，但「透明封装」不成立——每个宿主仍要自己接线，长度守卫依旧不是 adapter 契约的一部分。**子木倾向 A**（与节流下沉同款架构）；若 robit 选 B，子木接受但 P7-T8 需宿主接线（不算复刻规则，只是调用官方面）。
- **幂等/不双重分段**：frontend 已分好的段 ≤ cap，再过 adapter 守卫必须是**无操作**（`total <= max_len` 直通分支已保证，frontend.rs:582-584）——chatbot 回复路径行为逐位不变。
- **向后兼容**：宿主既有自截补偿层（子木 `clip_by_lines` 传 ≤cap 文本）行为逐位不变；robit 发版后子木自行退役补偿层（宿主侧动作，不劳 robit）。

### L2 【错误语义】多段发送的结果与部分失败语义

直调一条长文本变成 N 段发送后，`send_message` 的返回契约需要定义（现状 `Result<SendResult>` 只承载单条 msg_id）：

- **成功**：返回什么 msg_id？候选：末段 msg_id（宿主留痕够用）/ SendResult 扩展段数信息（`#[non_exhaustive]` 若已是）。robit 定，**文档写明**。
- **部分失败**（第 i 段失败）：宿主需要知道「已发出 i-1 段」才能如实留痕（子木 push_log 纪律：不装送达）。候选：错误变体携带已发段数 / 文档约定「Err 即视为该次发送失败，已发段不回滚」。
- **失败后余段策略**：子木倾向**放弃余段**（错误后继续刷无意义且加剧节流压力）；若 robit 倾向继续发完，须文档化，宿主按契约留痕。
- **逐段过节流**：每段独立 `throttle.reserve`（现 `send_text_inner` 形态天然满足）；段间隔 ~2s 是正常表现，宿主端点响应会等全部段完成（子木接受，不做异步化）。

### L3 【一致性】其余平台与文档收口

- 飞书等其余平台 adapter 的直调路径同款排查收口（有 `max_message_length` 宣称就要强制）。
- `PlatformAdapter` trait 文档（adapter.rs:265-345）写明：**直调 send 面按 caps 强制长度**（非编辑分段 ≤3 / 编辑截断），宿主无需也不应再自截；`send_media_message` 不涉及文本长度，不在本件范围。
- changelog 记名子木消费反馈（R9 先例）。

## 3. 验收标准（建议）

1. **单测**（robit-chatbot / robit-wechat / robit-qq）：
   - 直调 `send_message` 传 >cap 长文本（非编辑平台 mock）→ 恰 ≤3 次底层发送、每段 ≤cap、边界规则与 `split_for_platform` 既有单测同款、超预算尾段带 `TRUNCATION_MARKER`；
   - 直调传 ≤cap 文本 → 底层恰 1 次发送、内容逐字节不变（幂等直通）；
   - 编辑平台直调超长 → 截断 + 标记（或 robit 拍板的分段形态），单消息模型不破坏；
   - 部分失败：第 2 段注入失败 → 按拍板策略（放弃余段/继续）断言底层发送次数与返回语义；
   - **frontend 回归**：chatbot 回复路径既有分段/截断单测逐位不变（不双重分段）。
2. **节流协同**：多段直调每段过 `SendLimiter`（既有 throttle 单测形态扩展）。
3. **文档**：trait doc + changelog；若选方案 B，公开 API 的 doc 写明宿主接线责任。
4. **真机**（子木侧，发版后）：P7-T8 按钮 → 数千字回答推微信 → 收到 ≤3 段、段间隔 ~2s、无 -2 触发；QQ 收到截断/分段形态符合拍板。

## 4. 子木侧现状与消费计划

- **零绕过**：子木未复刻分段器（用户拍板否决）；既有 `clip_by_lines` 补偿层维持预警/摘要现状（传 ≤cap，与本件向后兼容）。
- **P7-T8 阻塞于此**：端点/档案取数/前端按钮均为宿主侧工作（不劳 robit），唯分段能力等本件发版。robit 发版（0.4.1 或 0.5.0，robit 定）后子木升版消费：问答推送直调官方面、退役该路径的宿主自截、push_log 按 L2 契约留痕。
- 移交节奏与方案取舍由 robit 拍板；本文档行号基准 0.4.0 @ 12e9cf9，若 robit 已有漂移以源码为准。