# robit 需求文档 · QQ 附件链路问题批 + chatbot 宿主形态六件套（子木第四轮移交）

- **来源**：子木 StockAgent · `docs/07-backlog.md` **R10**（2026-11-14 新登记，附件批 M1–M7）+ **R8**（2026-11-07 登记，宿主形态 H1–H6，本文档全量复述移交）
- **日期**：2026-11-14
- **读者**：robit 维护者（本文档随取随用，可直接进 robit 仓库评审）
- **版本基准**：robit **0.1.29**（`main` @ `0ada848`）——**全部行号已于本日在 0.1.29 源码逐条复核**；与 R8 登记时（0.1.28）相比仅一处增补：H5 的硬编码实为 4 个调用点（`tool/memory.rs:110/:254/:386/:518`），其余锚点行号未漂移（0.1.29 的 chatbot 改动只动了 ThinkingDelta 日志形态）
- **影响面**：`robit-agent`（media.rs / event.rs / agent.rs）、`robit-chatbot`（manager.rs / adapter.rs）、`robit-qq`（platform.rs）；H4 另及 `robit-agent::storage` 表形状
- **性质**：框架侧问题清单与诉求，方案选项列出后**由 robit 拍板**；不含任何宿主领域逻辑（协作原则见子木 ADR-0005：框架级缺口先改 robit，宿主不写绕过补丁）

> 回路记录：第三轮 R7 → v0.1.28；第四轮已开：R9 → v0.1.29 收口。本文档 = 第四轮其余候选的正式移交件（R8 登记时注明「移交与取舍等 robit 发布节奏」，即此件）。

## 1. 共同语境：QQ 附件从入站到模型的完整链路

2026-11-14 子木真机排查「QQ 上传文件如何存储」逐站核实（行号为 0.1.29）：

| 站 | 代码 | 行为 | 问题 |
|---|---|---|---|
| ① QQ 事件入站 | `robit-qq/src/platform.rs:918-929` | `QqAttachment`（url / content_type / filename / size / width / height）转平台无关 `MediaAttachment` | — |
| ② 文本标注 | 同文件 `:931-939` | **第一遍** `describe()` 追加进消息文本 | **M3** |
| ③ 本地落盘 | `robit-chatbot/src/manager.rs:283-295` → `robit-agent/src/media.rs:24-51` | 每附件 GET url 全量进内存、写 `<working_dir>/media/<filename>`；返回值（保存路径）**被丢弃** | **M4 / M5 / M6 / M7** |
| ④ 注入 Agent | `manager.rs:297-315` → `robit-agent/src/agent.rs:385,408` | `FrontendMessage::UserInput { text, attachments }`——text 已含第一遍标注；附件仍只带 **CDN url** | **M1** |
| ⑤ 进模型 | `agent.rs:1080-1167` | 图像且模型支持 vision → **对同一 url 第二遍下载**、压缩、base64 拼多模态消息（`:1120-1141`）；其余情况 → **第二遍** `describe()` 拼文本（`:1094-1098` 降级分支、`:1152-1160` 多模态非图分支） | **M2 / M3** |
| ⑥ 会话档案 | `manager.rs:305` `save_user_message(&msg.text)` | 只存文本（含第一遍标注）；图片本体与 base64 不入库 | — |

**净效果**：`media/` 本地副本纯留档（无代码消费它）；**非图片文件（csv/pdf/zip…）对模型只有一行描述，内容完全不可达**；文本模型或文件消息的标注行会重复出现；图片在 CDN url 过期后编码必失败、降级成描述行。

## 2. 附件批 M1–M7

### M1 【功能缺口·高】附件落盘后内容对模型不可达

- **现象/证据**：`media.rs:24-51` 的 `download_media` 返回 `Ok(PathBuf)`，`manager.rs:286-294` 调用处对返回值**直接丢弃**；`MediaAttachment` 两个形状（chatbot `adapter.rs:136-149`、agent `event.rs:15-28`）均无本地路径字段；`From` 转换（`adapter.rs:173-184`）自然也无从带起。
- **影响**：宿主纵有消费意愿也无路可循——子木发对账单 CSV，模型只见 `[用户发送了文件: 对账单.csv (50.0KB)]`。R7 已批注（子木 R2 拍板工具面不开系统 read），但**前提是路径可得**；M1 不修，任何附件消费方案（工具 / 预处理 / 注入 prompt）都无从落地。
- **诉求**：保存路径回填进附件结构（如 `local_path: Option<PathBuf>`，两形状 + `From` 同批）；**可选加强**：`describe()` 支持含路径形态，或 Manager 暴露 `on_media_saved(chat_id, path)` 回调。形态 robit 定。

### M2 【健壮性·中】同一 url 两次下载，vision 路径与 TTL 竞态

- **现象/证据**：③ 落盘一遍；⑤ `agent.rs:1123` `download_and_encode_base64(&attachment.url, …)`（`media.rs:64-77`）再拉一遍。而 robit 自家已有 `encode_file_base64(path, max_dim)`（`media.rs:82-94`，MIME 按扩展名、复用同一压缩管线）——**恰好是复用本地副本的现成形状**，chatbot 路径未用。
- **影响**：流量×2；落盘与编码两副本可能不一致；QQ CDN url 有时效，过期后即使本地文件在盘上，编码也失败降级成描述行。
- **诉求**：附件有 `local_path` 时编码读本地（M1 是前置），url 仅兜底。

### M3 【展示/存储缺陷·中】附件标注行重复，且两套 describe 模板并存

- **现象/证据**：② 拼第一遍；⑤ 再拼第二遍。两模板行为不一致：chatbot 版（`adapter.rs:158-169`）filename 缺省用 `image`/`file`，agent 版（`event.rs:37-45`）用 `unknown`。纯文本模型所有附件双份；vision 模型非图片附件双份、图片单份（多模态分支对图片不重复拼）。
- **影响**：提示词噪声、浪费 token；存档文本（⑥）永远固化第一遍——重复只出现在模型侧，排查时易误判。
- **诉求**：标注职责单一化——建议平台侧透传原文、agent 侧统一标注（与 M1 路径注入天然同一处）；若保留双处，至少合并 describe 实现并去重。

### M4 【安全加固·中】落盘文件名未消毒，存在路径穿越面

- **现象/证据**：`media.rs:33-37`：`save_dir.join(save_filename)`，filename 来自平台事件字段（`platform.rs:924` 原样搬运）。`Path::join` 语义：绝对路径整体替换 base；`..` 分量可越出目录。
- **影响**：实际暴露取决于 QQ 服务端对 filename 的消毒（未实测）；但 `robit-chatbot` 是多平台框架，**第二个 adapter 平台的字段形状不受 robit 控制**——库的信任边界应自我防御，不应把消毒责任悬空给平台。
- **诉求**：落盘名净化——取 `Path::file_name()` 分量、过滤路径分隔符与控制字符，或直接 uuid 改名 + 扩展名映射；与 M5 合并解决最省。

### M5 【正确性·低】同名静默覆盖 + uuid 名无扩展名

- **现象/证据**：同一消息两个同名附件（QQ 允许连发同名文件）→ `tokio::fs::write`（`media.rs:48`）后写覆盖；filename 缺省 → uuid（`media.rs:35`）**无扩展名**，`encode_file_base64` 的扩展名 MIME 推断失效（`media.rs:136-149`）、人工翻档也无法识别类型。
- **诉求**：冲突追加 `-1/-2` 或短 uuid 前缀；无名字时按 content_type 补扩展名。

### M6 【健壮性·中】落盘下载无大小上限、非流式

- **现象/证据**：`media.rs:40-48`：无 max-size 检查、不信任/不核对 `Content-Length`、全量 `bytes()` 驻留内存。同文件编码路径已有 decode-bomb 守卫（`MAX_DECODE_DIMENSION=16384` / `MAX_DECODE_ALLOC=256MB`，`:185-187`，守卫注释自己写着「Enough to OOM a resident bot process」）——**同一模块防御姿态不一致**。
- **影响**：常驻服务可被一个大文件顶出内存峰值 + 磁盘无界增长（M7 叠加）。
- **诉求**：对齐自家守卫先例——可配置 max bytes（缺省如 20MB）、优先流式写或先查长度再拉取。

### M7 【运维·低】media 目录无清理/保留策略

- **现象/证据**：全仓库对 `media/` 只写不删；`cleanup_loop`（`manager.rs:716-732`）本体即 no-op（见 H3）。
- **诉求**：按 mtime/count 的 rotation（缺省 off，维持现行为）；与 H3 同一处收口成本最低。

## 3. R8 六件套 H1–H6（2026-11-07 登记 · 本次正式移交）

> 来源 = `ChatbotManager` 接线为「常驻服务 + IM 通道」形态（子木 P7-T2，ADR-0011）时逐条源码核实。子木侧**零绕过**（未写合成消息 hack、未改路径行为）。

| # | 缺口与证据（0.1.29 逐条复核） | 诉求 |
|---|---|---|
| H1 | **无向活跃会话注入消息的公开 API**：公开面只有 `new`（`manager.rs:115`）/`run`/`active_session_count`；`handle_message` 私有（`:233`）。服务端调度器要给 IM 会话触发回合，只能借道 adapter 合成入站事件——会过 Confirmer/斜杠命令解析、污染档案，且与真实消息不可分辨 | 公开 API：按 chat_id 向活跃会话投递并留档（可带 source 标记区分注入/真实） |
| H2 | **落库 source 硬编码 "qq"**：`manager.rs:588` `insert_session(..., "qq")`——第二平台共用时来源失真 | source 参数化（平台名由 adapter/构造方提供） |
| H3 | **cleanup_loop 是 no-op + 僵尸会话静默丢消息**：`:716-732` tick 只打一行 debug，注释自认「no-op in MVP」；长跑进程内存会话只增不减；`/exit` 杀死 Agent 任务后 map 中句柄仍在，后续消息命中死 channel 只剩 warn（`:307-315`）、用户得不到任何回复，且 `save_user_message`（`:305`）先落库——档案里有、模型没看见 | cleanup 实装（空闲会话回收 + 死句柄剔除）；注入失败对用户可见（至少回执） |
| H4 | **会话库路径/查询无平台维度**：路径只由 (working_dir, global_storage) 决定；`robit-agent/src/storage.rs:296-297` `CREATE UNIQUE INDEX idx_sessions_chat_id ON sessions(chat_id) WHERE chat_id IS NOT NULL AND is_active = 1`——chat_id 全局唯一。同 working_dir 起两个 Manager（两个 QQ 应用/两平台），id 撞名即**互相劫持会话** | 库路径或查询键加平台/实例维度（表形状变更走 migration） |
| H5 | **memory 系工具不跟随 global_storage**：`tool/memory.rs:110/:254/:386/:518` 四处硬编码 `resolve_db_path(&ctx.working_dir, false)`——宿主开全局存储时，工具读写与会话库**不同库**（本次复核较登记时增补：不止一处） | 跟随宿主配置（如经 ToolContext 传 flag），缺省行为不变 |
| H6 | **人格提示词无宿主注入 API**：`agent.rs:159/:217`（`new`/`with_history` 同形）`PromptBuilder::with_working_dir(Some(&working_dir))`——文件搜索绑死 working_dir，产品人格资产只能躺 `<working_dir>/.robit/prompts/agent.md`（子木以 `.gitignore` 入库例外 + 守护测试凑合） | `Agent::new`/`ChatbotManager` 接受 custom prompt 入口（字符串/路径均可） |

## 4. 验收标准（只认行为，方案 robit 拍板）

- **M1+M2**：宿主开启附件本地消费后，QQ 发 csv → 模型能复述文件首行；发图 → 断网/CDN 过期场景下（本地副本已在盘）仍能识图。
- **M3**：纯文本模型发文件，`[用户发送了…]` 在提示词中恰出现一次。
- **M4+M5**：平台给出恶意/极端 filename（`..` 段、绝对路径、同名连发）后——所有落盘文件仍在 media 目录内、无互相覆盖。
- **M6**：超限附件下载被拒并留 warn，服务内存无 GB 级尖峰。
- **M7+H3**：长跑一周后 media 与会话内存有界（策略开启时）。
- **H1**：宿主调度器按 chat_id 注入回合，不经合成入站事件、不过斜杠解析，档案可区分来源。
- **H2**：双平台宿主下 archive source 逐条真实。
- **H4**：同 working_dir 两平台/实例互不劫持会话。
- **H5**：`global_storage=true` 时 memory 工具与 Manager 读写同库。
- **H6**：宿主不改 working_dir 布局即可注入人格文件/字符串；缺省（不注入）行为与现版本逐字节一致。

## 5. 子木侧配合面备忘（不必 robit 操心，仅交代接法）

- 子木工具面按 **R2 拍板**不含系统 read 类工具（工作目录含 `.env`，密钥安全面）；M1 落地后子木的消化路径是**宿主侧领域工具**（读注入路径、按 store 纪律脱敏）或回合前预处理，不要求 robit 开放系统工具。
- `backend/media/` 已在子木 `.gitignore`（入站附件永不入库，与真实持仓同纪律）。
- 附件消费若涉真实持仓文件，落账仍走 ADR-0012/0013 既有纪律（数字来自用户明示、先试算后落账）——M 系列只解决「看得见内容」，不改记账口径。

---

**关联记录**：子木 `docs/07-backlog.md` R8/R10 行；回路历史 R1→v0.1.26、R2–R5→v0.1.27、R6/R7→v0.1.28、R9→v0.1.29。
