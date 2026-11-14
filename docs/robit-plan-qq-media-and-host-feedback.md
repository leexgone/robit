# robit 评审意见与实现方案 · QQ 附件链路（M1–M7）+ chatbot 宿主形态（H1–H6）

- **评审对象**：`docs/robit-rfc-qq-media-and-host-feedback.md`（子木第四轮移交，R10 附件批 + R8 宿主六件套）
- **日期**：2026-11-14
- **版本基准**：robit **0.1.29**（`main` @ `0ada848`）。本文所有锚点已在该基准逐条复核
- **读者**：robit 维护者（评审与排期用）；子木侧作为接法预告
- **状态**：**已实现**（2026-11-14，feat/rfc-media-r10 分支）。三批全部落地，按本文方案执行，无偏离；实现细节以代码与 `CHANGELOG.md` [Unreleased] 为准。§5.1 拍板点采用推荐方案（`(source, namespace)` 查询键，单库不变）。测试：workspace 全量通过（含 v5→v6→v7 迁移、双实例隔离、注入绕过斜杠解析、本地优先编码、文件名消毒、下载上限、cleanup busy 保护等新增用例）。

## 1. 评审结论

### 1.1 逐项判定

RFC 全部 13 项问题的代码锚点、现象描述、影响推断**均属实**，无漂移、无夸大。定级基本克制准确，仅 H3 建议上调。

| # | 判定 | 级别（RFC → 评审） | 批次 | 一句话结论 |
|---|---|---|---|---|
| M1 | 属实 | 高 → 高 | 1 | 本批枢纽：`download_media` 返回值被丢弃（`manager.rs:286-294`），两形状均无路径字段。M2/M3 及宿主一切消费方案的前置 |
| M2 | 属实 | 中 → 中 | 1 | 同一 url 双下载（`agent.rs:1123`）；`encode_file_base64` 是现成的本地复用形状，但 MIME 按扩展名推断需补 content_type 覆盖（见 §1.2-b） |
| M3 | 属实 | 中 → 中 | 1 | 双份标注 + 两套 describe 模板属实。「平台透传、agent 统一」方向正确，但有档案/标题回归风险（见 §1.2-a） |
| M4 | 属实 | 中 → 中 | 1 | `media.rs:37` 裸 `join`，`..`/绝对路径可越界。多平台库信任边界应自防御，论证成立 |
| M5 | 属实 | 低 → 低 | 1 | 同名覆盖、uuid 无扩展名致 MIME 推断失效，均属实。与 M4 同一函数解决 |
| M6 | 属实 | 中 → 中 | 1 | 落盘下载无上限、全量驻留属实；**补充**：vision 二次下载路径同样无上限，两条路径同批加守卫 |
| M7 | 属实 | 低 → 低 | 2 | media 只写不删属实。缺省 off 稳妥，与 H3 同一 cleanup 收口 |
| H1 | 属实 | — | 2 | 公开面仅 `new`/`run`/`active_session_count`，注入只能借道合成事件。需 messages 表加 source 列（migration v6） |
| H2 | 属实 | — | 2 | `manager.rs:588` 硬编码 `"qq"`。六件套中最便宜，`PlatformAdapter` 加静态 `platform_name()` |
| H3 | 属实 | 隐含低 → **中** | 2 | cleanup no-op + 死句柄静默丢消息 + 先落库后发送，是长跑服务的实际正确性问题，建议优先于其余 H 项 |
| H4 | 属实 | — | 3 | 唯一索引 `sessions(chat_id)` 全局唯一（`storage.rs:296-297`）。唯一动表形状的项，单独排期；**波及面比 RFC 描述的更大**（见 §1.2-c） |
| H5 | 属实 | — | 2 | 四处 `resolve_db_path(&ctx.working_dir, false)` 全部命中（`memory.rs:110/254/386/518`），与 Manager 的 global_storage 行为分裂，是 bug 不是取舍 |
| H6 | 属实 | — | 2 | `PromptBuilder` 只有 `new`/`with_working_dir`；全局 fallback `~/.robit/prompts/agent.md` 是进程级共享，多实例宿主确实无路可走 |

### 1.2 评审新发现（RFC 未提，方案必须处理）

**a) M3 的档案/标题回归**。平台侧去掉第一遍 describe 后，`save_user_message(&msg.text)`（`manager.rs:305`）的档案文本与 `generate_title(first_message)` 的会话标题都会失去附件标注——**纯附件消息（text 为空）将变成空档案 + 标题退化为「QQ 会话」**。因此「标注单一化」不能只删第一遍：manager 须在落库/取标题处用统一 describe 补一次标注（模型侧仍由 agent 标注，每条文本流恰一份）。详见 §3.3。

**b) M2 的 MIME 陷阱**。`encode_file_base64`（`media.rs:82-94`）按**扩展名**推 MIME（`mime_from_extension` 只认 png/jpg/gif/webp）。附件自带平台给的 `content_type`，落地时应优先用它，否则错误/缺失扩展名的图片会被当 `application/octet-stream` 跳过压缩管线。需新增带 MIME 覆盖的编码入口。详见 §3.4。

**c) H4 的波及面大于「唯一索引」**。同 chat_id 的跨平台劫持不止发生在 `find_session_by_chat_id`：

- `activate_session`（`storage.rs:563-579`）`UPDATE ... WHERE chat_id = ?1` 会**停用其他平台的同 id 会话**（`/switch` 触发）；
- `list_all_sessions_by_chat_id`（`storage.rs:550-560`）会把其他平台的会话列进 `/list`。

三处查询必须同批加平台维度。另外 **`(chat_id, source)` 仍不足以隔离「同平台两实例」**（两个 QQ 应用 source 都是 `"qq"`），需要可选 instance 命名空间维度。详见 §5。

**d) 形状变更波及面小（利好）**。全仓库 `MediaAttachment` 字面量构造点仅 `robit-qq/src/platform.rs:921` 一处（GUI/TUI 不构造附件），加字段是同仓同步改，无外部破坏面。`download_media` 另有一个调用点 `tool/generate_image.rs:286`（生图结果落盘），签名演进需保留兼容入口（§3.5）。`insert_message` 调用点共 5 个文件（chatbot frontend、gui state/commands、storage 自身、gui 集成测试），加参数可控。

### 1.3 总体取舍

- **照单全收**：13 项全部纳入，分三批交付（§2.2）。全部诉求不含宿主领域逻辑，符合 ADR-0005 分工，无需退回子木侧。
- **M1 之后宿主走领域工具消化**（读注入路径、按 store 纪律脱敏），robit 不开系统 read 类工具——与 R2 拍板一致，RFC §5 备忘确认。
- **H4 完成前，双平台/双实例宿主不应上生产**：H1 注入与现有路由在 H4 落地前仍按全局 chat_id 查库。

## 2. 总体设计原则与分批

### 2.1 原则

1. **加法优先**：新字段一律 `Option`/带缺省值；新配置一律有缺省且缺省行为与 0.1.29 一致（H6 验收要求逐字节一致）。
2. **职责单一**：附件标注模型侧归 agent、档案侧归 manager，共用唯一 describe 实现；下载守卫两条路径同一套。
3. **信任边界自防御**：文件名消毒、大小上限不依赖平台服务端行为。
4. **表形状变更走 migration chain**：现有 `_schema_meta` 版本链（当前 v5，`storage.rs:345-364`）已成熟，v6/v7 顺接，各配迁移测试。

### 2.2 分批与依赖

| 批次 | 内容 | schema | 依赖 |
|---|---|---|---|
| **1 · 附件链路** | M1 M2 M3 M4 M5 M6 | 无 | 无。M1→M2 顺序实现；M4+M5 同函数；M6 两路径同批 |
| **2 · 宿主形态** | H1 H2 H3 H5 H6 + M7 | **v6**（messages.source） | H2 先行（H1/H3 用到 platform_name）；H3+M7 同一 cleanup 收口 |
| **3 · 会话隔离** | H4 | **v7**（sessions.namespace + 索引重建） | 依赖批 2 的 H2（source 参数化）；单独排期 + 迁移测试 |

H2、H5 是小时级独立修复，可提前于批 1 合入（无 schema 变更、无耦合），不必等批次节奏。

## 3. 批 1 详细设计：附件链路 M1–M6

### 3.1 M1 · 附件形状加 `local_path`

两个形状同批加字段（`crates/robit-agent/src/event.rs:15-28`、`crates/robit-chatbot/src/adapter.rs:136-149`）：

```rust
/// Local copy saved by the host (bot manager). None when not downloaded
/// (e.g. GUI/TUI paths, or download failed).
pub local_path: Option<std::path::PathBuf>,
```

- `From<adapter::MediaAttachment> for event::MediaAttachment`（`adapter.rs:173-184`）同批携带。
- `platform.rs:921` 构造点置 `None`（平台侧不落盘）。
- **manager 回填**（`manager.rs:283-295` 重写）：下载与转换合并为一次遍历，按序把 `download_media` 返回的 `Ok(path)` 写进对应附件的 `local_path`；下载失败保持 `None` + warn（现行为不变）。

```rust
let mut attachments: Vec<event::MediaAttachment> = Vec::with_capacity(msg.attachments.len());
for att in msg.attachments {
    let local_path = media::download_media_with_opts(&att.url, att.filename.as_deref(), &media_dir, &opts)
        .await
        .map_err(|e| tracing::warn!("Failed to download media: {}", e))
        .ok();
    let mut converted: event::MediaAttachment = att.into();
    converted.local_path = local_path;
    attachments.push(converted);
}
```

不引入 `on_media_saved` 回调——`local_path` 已覆盖宿主消费诉求（领域工具从注入路径读），回调属过度设计，暂不做。

### 3.2 M3 · describe 统一与标注单一化

1. **唯一实现保留在 `event.rs`**（agent 侧）。缺省名统一为 `image`/`file`（取 chatbot 版，信息量优于 `unknown`）。`adapter.rs:158-169` 的 describe 删除，`is_image` 保留。
2. **平台侧透传原文**：删除 `platform.rs:931-939` 的第一遍标注。
3. **manager 负责档案/标题标注**：新增 helper（chatbot 内）：

```rust
fn annotated_text(text: &str, attachments: &[event::MediaAttachment]) -> String {
    // text 为空时仅拼接标注；否则 text + "\n" + 各标注（与原平台侧行为同构）
}
```

`handle_message` 新流程：

```text
下载回填 local_path（§3.1）
→ let archive_text = annotated_text(&msg.text, &attachments)
→ get_or_create_session(chat_id, &archive_text)   // 标题用标注后文本
→ frontend.save_user_message(&archive_text)        // 档案用标注后文本
→ tx.send(UserInput { text: msg.text, attachments })  // 模型侧拿原文
```

4. **模型侧标注不变**：`build_user_message`（`agent.rs:1080-1167`）已在降级分支/多模态非图分支拼 describe，恰好一份。

**净效果**：模型提示词恰一份标注；档案/标题保持含标注（与现行为同构）；纯附件消息不再空档案。
**已知微差**：无名附件的档案文本缺省名由 `unknown` 变为 `image`/`file`（模板统一的必然结果，验收只约束模型侧份数，可接受）。

### 3.3 M2 · 本地优先编码

新增带 MIME 覆盖的入口（`media.rs`），`encode_file_base64` 保留为扩展名推断的便捷封装（`read`/`read_image` 工具继续用）：

```rust
pub async fn encode_file_base64_with_mime(
    path: &Path, mime: &str, max_image_dim: u32,
) -> Result<EncodedImage, MediaError>
```

`build_multimodal_message`（`agent.rs:1120-1151`）图片分支改为三级兜底：

```text
local_path 存在 → encode_file_base64_with_mime(path, &attachment.content_type, max_dim)
  失败 → warn + download_and_encode_base64(url, content_type, max_dim)   // url 兜底
    再失败 → warn + describe 降级（现行为）
local_path 为 None → 直接 url 路径（现行为）
```

`content_type` 为空串时回落扩展名推断。断网/CDN 过期场景（验收 M1+M2）由第一级覆盖。

### 3.4 M4+M5 · 文件名消毒与冲突

`media.rs` 新增私有 `fn sanitize_filename(filename: Option<&str>, content_type: Option<&str>) -> String`：

1. 取 `Path::new(name).file_name()` 分量（一次剥离目录与 `..`）；再滤控制字符（`< 0x20`、`0x7f`）与残留分隔符；trim 首尾空白与点。
2. 结果为空 → uuid 名 + 按 `content_type` 补扩展名（`image/jpeg→.jpg`、`image/png→.png`、`application/pdf→.pdf` 等常见映射，未知则不加）。有名字但无扩展名时同样按 content_type 补（修 M5 的 MIME 推断失效）。
3. **冲突处理**：目标路径已存在 → 追加 `-1`、`-2`…（扩展名前插入），异步探测上限 100 次，超限退 uuid 名。同一消息两个同名附件不再互相覆盖。

`download_media` 内部改用该函数；`save_dir.join` 的入参从此只能是单分量文件名。

### 3.5 M6 · 大小上限与流式落盘

**配置**：`[app.context]` 加 `max_media_bytes`（缺省 `20 * 1024 * 1024` = 20MB，`0` = 不限）。放 `[app.context]` 的理由：agent 侧 url 兜底下载经 `ContextManager` 取配置（与 `max_image_dimension` 同处），manager 侧从 `config.app.context` 读同一值——**一个旋钮管两条路径**，避免双配置分裂。

**API 演进**（保留 `generate_image.rs:286` 兼容）：

```rust
pub struct MediaDownloadOptions {
    pub max_bytes: u64,          // 0 = unlimited
    pub content_type: Option<String>,  // 供 §3.4 扩展名映射
}
impl Default for MediaDownloadOptions { /* 20MB, None */ }

pub async fn download_media_with_opts(url, filename, save_dir, opts) -> Result<PathBuf, MediaError>;
pub async fn download_media(url, filename, save_dir) -> Result<PathBuf, MediaError> {
    download_media_with_opts(url, filename, save_dir, &Default::default()).await
}
```

**行为**：

- 响应 `Content-Length` 超限 → 立即拒（`MediaError::TooLarge`，新错误变体），不发 body；
- 无 Content-Length 或不可信 → `response.chunk()` 流式累加，超上限即中止连接、不落盘、返回 `TooLarge`；内存峰值 ≤ 上限 + 一个 chunk；
- manager 侧对 `TooLarge` 打 warn（含 url 与上限），附件 `local_path = None`，消息照常处理（模型侧仍有标注行）。
- `download_and_encode_base64`（vision url 兜底）套同一上限（经 `ContextManager` 传入），同样流式累加。

`generate_image` 保持旧签名 → 缺省 20MB 守卫（此前无守卫，属收紧；生图产物极少超限，可接受）。

### 3.6 批 1 改动文件清单

| 文件 | 改动 |
|---|---|
| `robit-agent/src/event.rs` | `local_path` 字段；describe 缺省名统一 |
| `robit-agent/src/media.rs` | sanitize_filename、冲突后缀、MediaDownloadOptions、流式+上限、`encode_file_base64_with_mime`、`TooLarge` |
| `robit-agent/src/agent.rs` | 多模态图片分支三级兜底 |
| `robit-agent/src/context.rs` | `max_media_bytes` 解析与暴露 |
| `robit-ai/src/config.rs` | `ContextConfig.max_media_bytes` |
| `robit-chatbot/src/adapter.rs` | `local_path` 字段、From 携带、删 describe |
| `robit-chatbot/src/manager.rs` | 下载回填、annotated_text、档案/标题/发送分流 |
| `robit-qq/src/platform.rs` | 删第一遍标注、构造点补 `local_path: None` |

## 4. 批 2 详细设计：宿主形态 H1/H2/H3/H5/H6 + M7

### 4.1 H2 · platform_name（先行）

`PlatformAdapter` 加静态方法（与 `capabilities()` 同形，`adapter.rs:225-228`）：

```rust
/// Platform identifier used as session `source` ("qq", "feishu", ...).
fn platform_name() -> &'static str;
```

QQ 实现返回 `"qq"`。`manager.rs:588` 改 `insert_session(..., T::platform_name())`。Manager 存 `source: &'static str` 字段备用（H1/H4 用到）。

### 4.2 H5 · memory 工具跟随 global_storage

**关键事实**：`MemorySettings.dir`（`memory.rs:26-31`）经 `resolve_memory_settings` 解析时**已尊重 global_storage**（含降级 fallback），且 `resolve_db_path = resolve_memory_dir(...).join("robit.db")`。Agent 在 `new`/`with_history` 已收到 `memory: MemorySettings` 但用完即弃。

**方案**（零签名变更）：

1. `Agent` 存 `memory_dir: PathBuf`（取自 `memory.dir`）。
2. `ToolContext`（`tool/mod.rs:181-210`）加 `pub memory_dir: PathBuf`；`agent.rs:929` 构造处克隆填入（三个测试构造器同步）。
3. `tool/memory.rs` 四处改 `ctx.memory_dir.join("robit.db")`，删除 `resolve_db_path(&ctx.working_dir, false)`。

缺省（global_storage=false）时 `memory_dir = <working_dir>/.robit/memory`，与现路径逐字节一致；开启时与 Manager 同库（验收 H5）。边界：`resolve_memory_settings` 的 home 解析失败降级为 project-local，与 Manager 的硬失败略有差异——属既有降级纪律（spec §8），文档注明即可。

### 4.3 H3 + M7 · cleanup 实装

**结构改造**：

- `ChatbotManager.agents` 改 `Arc<Mutex<HashMap<String, AgentHandle>>>`（run 的 cleanup 任务需要共享所有权）。
- `AgentHandle` 加 `busy: Arc<AtomicBool>`，与 `ChatbotFrontend` 共享：回合开始（首个 TextDelta/ToolCallRequested/ThinkingDelta）置 true，`TurnComplete`/错误终态置 false。**防止长回合被空闲回收误杀**。
- `cleanup_loop` 签名改为接收 `Arc<Mutex<HashMap>>` + timeout + media 配置 + shutdown。

**每 tick（现 300s）做三件事**：

1. **死句柄剔除**：`message_tx.is_closed()` → 移除（`/exit`、panic 后的残留）。
2. **空闲回收**：`!busy && last_active_at.elapsed() > session_timeout` → 移除 handle。Drop `message_tx` 使 Agent run 循环退出（现注释描述的机制，`manager.rs:711-715`）；DB 会话记录保留，下次消息经 `get_or_create_session` 从库复活。
3. **media rotation（M7，缺省 off）**：配置见 §6。`retention_days > 0` → 按 mtime 删超龄文件；`max_files > 0` → 按 mtime 保留最新 N 个。只扫 `<working_dir>/media/` 顶层文件，不递归。

**死句柄发送失败自愈**（`handle_message:307-315` 重写）：

```text
try_send 失败 → 移除该 handle → get_or_create_session 重建（从 DB 复活）→ 重发一次
  再失败 → platform_sender 回执「❌ 消息处理失败，请重试」+ error 日志
```

**落库时序修正**：`save_user_message` 移到**发送成功之后**（含重试成功），消除「档案里有、模型没看见」。重试路径只落库一次。

### 4.4 H1 · 注入 API

```rust
/// Inject a user turn into a chat's session from the host (scheduler, etc.).
/// Bypasses confirmation-keyword routing and slash-command parsing.
/// `source` marks the archive row (e.g. "scheduler"); must be non-empty.
pub async fn inject_message(
    &self, chat_id: &str, text: &str, source: &str,
) -> Result<(), AgentError>;
```

行为：

- 不过 Confirmer、不过斜杠解析（注入 `"/help"` 就是字面文本进模型）；
- `get_or_create_session` → 落库（带 source 标记）→ 发送；死句柄走 §4.3 同款自愈；
- 失败返回 `Err` 由宿主决定回执（注入方是程序，不是 IM 用户，不强制平台通知）；
- 更新 `last_active_at`，注入回合参与正常工具确认流程（安全面不放松）。

**档案来源标记（migration v6）**：

- `messages` 加列 `source TEXT`（可空；NULL = 真实用户，注入行存 `"scheduler"` 等标签）；
- `insert_message`（`storage.rs:639`）加 `source: Option<&str>` 参数，5 个调用文件现有调用传 `None`；`ChatbotFrontend::save_user_message` 加 `save_user_message_with_source` 变体（原方法委托，传 None）；
- `load_chat_messages` **不过滤** source——注入内容是会话历史的一部分，模型下回合可见；
- GUI/TUI 列表渲染不受影响（新列可空、旧行 NULL）。

### 4.5 H6 · 人格提示词注入

**关键事实**：`PromptBuilder` 已有私有 `custom_prompt: Option<String>` 字段（`prompt.rs:20-22`），只缺公开入口。

1. `PromptBuilder::with_custom_prompt(prompt: String) -> Self`——直接置字段，跳过文件查找。
2. `Agent::new` / `with_history` 各加一个参数 `custom_prompt: Option<String>`：`Some` → `with_custom_prompt`；`None` → 现 `with_working_dir` 路径**逐字节不变**（验收 H6）。调用点全在仓内（GUI/TUI/chatbot/examples），同批改。
3. `ChatbotManager` 经配置注入（`[app.bot]`，见 §6）：`system_prompt`（字符串）> `system_prompt_file`（路径，读失败 warn 并回落文件查找）> 现 working_dir 查找。Manager 存解析结果，`spawn_session_agent` 传入 `Agent::with_history`。

人格资产从此可躺在宿主自己的目录（如子木 `backend/prompts/agent.md`），不必挤进 `<working_dir>/.robit/prompts/`，`.gitignore` 例外与守护测试可撤。

## 5. 批 3 详细设计：H4 会话隔离

### 5.1 键设计（拍板点）

**推荐：查询键加 `(source, namespace)` 双维度，单库不变。**

- `source`：批 2 的 `T::platform_name()`，隔离**跨平台**（qq vs feishu）。
- `namespace`：`[app.bot] instance_id`（可选，缺省 `""`），隔离**同平台多实例**（两个 QQ 应用）。仅 source 不够——两个 QQ 应用 source 同为 `"qq"`，RFC 的「两 QQ 应用」场景必须覆盖。

备选「库路径加平台维度」被否：会割裂 GUI/TUI 与 Bot 共库语义（GUI 需感知实例才能打开同一库），且 `global_storage` 组合爆炸。

### 5.2 migration v7

```sql
ALTER TABLE sessions ADD COLUMN namespace TEXT NOT NULL DEFAULT '';
DROP INDEX IF EXISTS idx_sessions_chat_id;
CREATE UNIQUE INDEX idx_sessions_chat_id
    ON sessions(chat_id, source, namespace)
    WHERE chat_id IS NOT NULL AND is_active = 1;
```

旧行 namespace=''、source 已真实存在（除 H2 修复前的 `"qq"` 硬编码行，本就属 qq）——单实例缺省行为不变。v6→v7 顺接现有 chain，配迁移测试（含「v5 旧库直升 v7」路径）。

### 5.3 查询面改造

Manager 存 `(source, namespace)`，以下函数加参并全部经 Manager 传入：

| 函数 | 改动 | 不改的后果 |
|---|---|---|
| `find_session_by_chat_id` | + source + namespace 过滤 | 跨平台/实例劫持会话（RFC 原场景） |
| `activate_session` | `UPDATE ... WHERE chat_id=? AND source=? AND namespace=?` | `/switch` 停用其他平台同 id 会话 |
| `list_all_sessions_by_chat_id` | + source + namespace 过滤 | `/list` 串台 |

GUI/TUI：chat_id 为 NULL，不受唯一索引约束；调用传各自 source（"gui"/"tui"）+ `""`。

### 5.4 验收

同 working_dir 起两个 Manager（不同 instance_id 或不同平台），同 chat_id 互不劫持；`/switch`、`/list` 不串台；单实例旧库升级后行为不变。

## 6. 配置面变更汇总

```toml
[app.context]
max_media_bytes = 20971520        # 新增（M6）：附件下载上限（字节），两条下载路径共用；0 = 不限。缺省 20MB

[app.bot]
instance_id = "stockagent-prod"   # 新增（H4，批 3）：会话命名空间，缺省 ""（单实例行为不变）
system_prompt = "..."             # 新增（H6）：人格提示词字符串，优先级最高
system_prompt_file = "backend/prompts/agent.md"  # 新增（H6）：人格文件路径（相对 working_dir 或绝对）

[app.bot.media]                   # 新增小节（M7）：media 目录保留策略，缺省全 off
retention_days = 0                # 按 mtime 删除超龄文件；0 = 不清理
max_files = 0                     # 按 mtime 保留最新 N 个；0 = 不限
```

CLAUDE.md 配置节、`docs/llm-config.md`（若涉及）、`docs/architecture.md` 媒体链路描述同批更新。

## 7. 验收 ↔ 测试映射

RFC §4 验收标准逐条落测试（单测为主，真机项标注）：

| 验收 | 测试 |
|---|---|
| M1+M2 csv 首行可复述 | 宿主侧真机验收（子木）；框架侧单测：local_path 回填、`encode_file_base64_with_mime` 用 content_type |
| M1+M2 断网识图 | 单测：local_path 在盘 + url 不可达 → 编码成功；local 损坏 → url 兜底；双失败 → describe |
| M3 标注恰一份 | 单测：`build_user_message` 纯文本模型/多模态非图分支，`[用户发送了` 计数 == 附件数；manager 档案/标题含标注、纯附件消息非空 |
| M4+M5 恶意 filename | 单测：`..` 段、绝对路径、控制字符、同名连发、无名 + content_type → 全部落在 media 内、无覆盖、扩展名正确 |
| M6 超限拒收 | 单测：Content-Length 超限即拒；无长度流式超限中止（本地起临时 HTTP 服务或 mock chunk 流）；`TooLarge` warn |
| M7+H3 长跑有界 | 单测：rotation 按 mtime/count；cleanup 空闲回收、死句柄剔除、busy 跳过；真机长跑由子木侧观察 |
| H1 注入 | 单测：注入 `"/help"` 不进斜杠解析、messages.source 落标签、load_chat_messages 含注入行、死句柄自愈 |
| H2 source 真实 | 单测：fake adapter platform_name="test" → insert_session source="test" |
| H4 互不劫持 | 单测：双 source/双 namespace 同 chat_id 各自独立；activate/list 不串台；v5→v7 迁移测试 |
| H5 同库 | 单测：global_storage=true 时 memory 工具写入路径 == Manager 库路径；false 时逐字节同旧 |
| H6 缺省逐字节一致 | 单测：custom_prompt=None 时 build_system_prompt 输出与 0.1.29 基线快照一致；Some 时覆盖生效、system.md 部分不变 |

## 8. 兼容性与风险

| 风险 | 处置 |
|---|---|
| `MediaAttachment` 加字段破坏字面量构造 | 仓内仅 platform.rs 一处（§1.2-d），同批改；对外属 minor 破坏面，CHANGELOG 注明 |
| `insert_message` / `Agent::new` / `with_history` 签名变更 | 调用点全在仓内（5 文件 / GUI+TUI+chatbot+examples），同批改；无外部消费者（子木经 ChatbotManager 接线，不直接调这些） |
| 无名附件档案文本微差（unknown → image/file） | 模板统一的必然结果，只影响档案展示不影响模型侧份数验收，CHANGELOG 注明 |
| 落库时序后移（H3） | 行为修正：发送失败不再留「模型没看见」的档案行；重试成功恰落一次 |
| generate_image 落盘从无守卫变 20MB 缺省 | 收紧而非放松，生图产物极少超限；如需豁免可传 opts（暂不） |
| 空闲回收误杀长回合 | busy 标志（§4.3）；回收只 drop 内存句柄，DB 记录保留可复活，最坏情况是回合结束后下条消息重建会话 |
| v6/v7 迁移 | 顺接现有 chain；v7 索引重建在「旧索引保证唯一」前提下无冲突行风险（同 v4→v5 先例）；各配旧库升级测试 |
| H4 前双平台上生产 | 文档明示：批 3 落地前双平台/双实例共享 working_dir 不受支持 |

## 9. 发布与文档同步

- 每批一个 release：批 1（附件链路）→ 批 2（宿主形态 + v6）→ 批 3（H4 + v7）。H2/H5 可提前单独合入。
- 版本号按仓库节奏在发布时定（语义化角度批 1/2 含新功能属 minor；0.1.x 阶段沿用现行 bump 惯例，三处同改 + `cargo check` 同步 lock，见 CLAUDE.md 发版流程）。
- 每批同步：`CHANGELOG.md`、CLAUDE.md 配置节（§6 新键）、`docs/architecture.md` 媒体/会话链路描述。
- 批 1 发布后通知子木接 M1 消费路径（领域工具读 `local_path`）；批 2 发布后子木可撤 H6 的 `.gitignore` 例外与守护测试、接 H1 调度注入；批 3 发布后双实例部署解禁。
- 本 RFC 评审状态回写：评审通过、13 项全收、H3 上调中级、分三批。

---

**关联**：`docs/robit-rfc-qq-media-and-host-feedback.md`（移交件）；子木 `docs/07-backlog.md` R8/R10；回路历史 R1→v0.1.26 … R9→v0.1.29。