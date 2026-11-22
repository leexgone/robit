# robit 实现设计 · IM 出站 Markdown 表格投影（回应子木 R17）

- **对应需求**：`docs/robit-rfc-im-table-projection.md`（子木第七轮移交 R17，2026-11-22）
- **日期**：2026-11-22
- **基线复核**：本仓库 `main` = 0.4.1 @ `de26925`（+ 需求文档提交 `e617d46`），与需求文档行号基准一致；本文所有行号已在当前源码逐条复核
- **状态**：**已实施（0.5.0）**。2026-11-22 维护者拍板：D1–D6 全部按推荐采纳（D1 做 T3；D2 `text (url)`；D3 接受 QQ 改善性变化；D4 0.5.0；D5 提示词渠道提示缓议；D6 F1 随本次修复、F4 images 对称激活、F3 任务列表勾选态后续）。实施备注见 §9
- **结论**：需求成立、事实准确、方向正确。**建议 T1 + T2 + T3 全采纳**，版本 **0.5.0**；在需求方案基础上增加一项关键增强——**纯文本快速通道**（§4.2），它使「幂等直通」从假设变成构造性保证，并大幅降低既有测试迁移量

---

## 1. 事实复核（需求文档 vs 当前源码）

### 1.1 需求文档声称的事实——全部属实

| 声称 | 复核位置 | 结论 |
|---|---|---|
| 微信 `flush_buffer` 整体跳过清洗器 | [frontend.rs:256-260](../crates/robit-chatbot/src/frontend.rs#L256-L260) `supports_markdown=false → text.clone()` | ✅ 属实 |
| QQ 表格走对齐伪表格，宽度按 `chars().count()` | [markdown.rs:276-312](../crates/robit-chatbot/src/markdown.rs#L276-L312) `render_table` | ✅ 属实（CJK 宽 2 按 1 计） |
| `features.tables` 死标志 | 表格分支 [markdown.rs:174-200](../crates/robit-chatbot/src/markdown.rs#L174-L200) 不读 features | ✅ 属实 |
| `features.code_blocks` 死标志 | 代码块分支 [markdown.rs:82-95](../crates/robit-chatbot/src/markdown.rs#L82-L95) 恒输出栅栏 | ✅ 属实 |
| 标题降级不尊重 `bold` | [markdown.rs:73-75](../crates/robit-chatbot/src/markdown.rs#L73-L75) 恒输出 `**` | ✅ 属实（QQ bold=false，用户看到字面星号） |
| 引用降级输出字面 `> ` | [markdown.rs:137-140](../crates/robit-chatbot/src/markdown.rs#L137-L140) | ✅ 属实 |
| 链接降级丢弃 URL | [markdown.rs:155-157](../crates/robit-chatbot/src/markdown.rs#L155-L157) | ✅ 属实 |
| QQ features：仅 `code_blocks=true` | [adapter.rs:70-86](../crates/robit-chatbot/src/adapter.rs#L70-L86) | ✅ 属实 |
| 微信 features 全 false、caps 宣称纯文本 | [adapter.rs:109-111](../crates/robit-chatbot/src/adapter.rs#L109-L111)、[adapter.rs:156](../crates/robit-chatbot/src/adapter.rs#L156) | ✅ 属实 |
| weixin-agent 0.3.0 有意保留表格 | 本地 registry 源码 `weixin-agent-0.3.0/src/messaging/markdown_filter.rs:1-9` 模块文档、`:557` `table_preserved` 单测 | ✅ 属实（上游事实，不改 SDK） |
| 两 adapter 已在发送咽喉调 `prepare_outbound` | [robit-qq/platform.rs:333](../crates/robit-qq/src/platform.rs#L333)、[robit-wechat/platform.rs:322](../crates/robit-wechat/src/platform.rs#L322) | ✅ 属实 → **T3 无需改任何 adapter crate** |

### 1.2 复核中的额外发现（需求文档未覆盖，纳入设计）

| # | 发现 | 位置 | 处理 |
|---|---|---|---|
| F1 | **`blockquotes=true` 时引用标记也丢失**：`in_blockquote` 置位后无任何消费点，feishu（未来）引用会退化为普通段落 | markdown.rs:131-136 | 随 T2 一并修复（按行前缀 `> `），低危——现网无平台 `blockquotes=true` |
| F2 | **`strikethrough=false` 时字面 `~~` 泄漏**：`ENABLE_STRIKETHROUGH` 解析选项按 flag 开关，flag=false 时 `~~x~~` 作为纯文本直通 | markdown.rs:28-30 | 解析恒开、发射按 flag（与 bold/italic 同构），纳入 T2 |
| F3 | **任务列表勾选态丢失**：`- [x]` 与 `- [ ]` 都渲染为 `- ` | markdown.rs:124-128 | 登记备查，可选后续（§8），不进核心 |
| F4 | **`features.images` 也是死标志**：恒输出 `[Image: alt]` | markdown.rs:159-167 | 与 tables 对称激活（`=true` 直通 `![alt](url)`），可选 |
| F5 | **微信 SDK 会整体删除图片语法** `![alt](url)`（模块文档 :9 "Removes entirely: images"） | SDK markdown_filter.rs | T2 后投影产出 `[Image: alt]` 文本反而**修复**了微信图片信息全丢的问题——额外收益 |
| F6 | **T3 的影响面不止宿主直调**：确认提示（含 pretty-print JSON 参数，confirmer.rs:233-236）、/help 文本（manager.rs:714-725）、错误通知、进度提示全部经 `send → prepare_outbound`，T3 后都会过投影 | confirmer.rs / manager.rs | 快速通道（§4.2）+ 系统消息回归测试兜底 |
| F7 | **`strip_last_text` 捕获 hack 有泄漏边界**：单元格内 HardBreak（`"  \n"`）因以换行结尾无法回收，残留到 output | markdown.rs:316-334 | T1 重构为事件路由（§4.3），删除该 hack |
| F8 | `HardBreak` 恒输出 `"  \n"`（行尾双空格是 Markdown 语法残留） | markdown.rs:54 | 纯文本平台改 `"\n"`；QQ 不可见差异，随 T2 清理 |
| F9 | `supports_markdown` 在 T2/T3 后失去消费点（唯一消费就是 flush 的门控） | frontend.rs:256 | 保留字段、改语义为「平台原生渲染 Markdown」的**描述性**声明，文档注明投影恒跑 |
| F10 | adapter.rs 两处注释「the SDK strips Markdown」与 SDK 实际行为（保留表格/栅栏/加粗）不符 | adapter.rs:107-108、156 | 随 T2 更正注释 |

---

## 2. 需求评估

### 2.1 总体判断

**需求成立，建议采纳。** 核心论点经得起推敲：

1. **原理正确**：IM 客户端比例字体下，任何依赖列对齐的表格形态原理性不可读；线性化把「排版问题」转成「记录序列化问题」，对齐假设整个消失。这不是优化，是正确的投影策略。CJK 宽度问题（`chars().count()`）随之蒸发，无需引入 unicode-width。
2. **缺口真实**：两条泄漏路径（微信跳过清洗器、QQ 伪表格）与六个 feature 不一致全部源码实锤（§1.1）。caps 宣称纯文本却出站原始 Markdown，属于契约违背，修复优先级合理。
3. **边界清晰**：不改上游 SDK、不含宿主领域逻辑、Web 端不受影响（react-markdown 正常渲染）、与 R16 长度守卫同构复用同一咽喉。改动面收敛在 `robit-chatbot` 三个文件（markdown.rs / outbound.rs / frontend.rs）+ adapter.rs 文档。
4. **不建议路径的排除理由充分**（unicode-width 治标、渲染成图过度设计、ark 卡片非通用解），同意留档。

### 2.2 需求文档需要修正/补充的三点

**① 「QQ 路径除 T1 表格外逐位不变」声称过强（文档内部张力）。**
T2 前置要求「降级尊重 features」，而 QQ 的 `headings=false, bold=false, blockquotes=false`：

- 标题：现状字面 `**Title**` → 修复后纯文本 `Title`（**变了**）
- 引用：现状字面 `> text` → 修复后 `text`（**变了**）
- 删除线（F2）：现状字面 `~~x~~` → 修复后 `x`（**变了**）
- 链接（若采纳 D2）：现状丢 URL → `text (url)`（**变了**）

这些都是**改善**（与 caps 诚实一致），且既有单测全部仍通过（`unsupported_headings_become_bold` 显式设 `bold=true`，不受影响），但「逐位不变」应改述为：**「QQ 纯文本回复逐位不变；含标题/引用/删除线/链接/表格的回复按 features 诚实降级（改善性变化，CHANGELOG 明示）」**。需维护者确认（D3）。

**② T3 的幂等是假设而非保证，需构造性兜底。**
文档假设「纯文本无 Markdown 标记，二次投影无操作」。但投影函数恒做 `trim_end + "\n"` 归一化，且任意 plain text 可能含 Markdown 显著字符（`2*3*4` 会被解析为强调吃掉星号；系统确认提示里的 JSON 参数同理，F6）。本设计用**纯文本快速通道**（§4.2）把「无语法文本逐字节直通」变成构造性保证，同时把「出站文本按 Markdown 语义处理」写成 trait 契约。残余风险（含成对 `*`/`_` 的纯文本）登记在 §5。

**③ 线性化规则需补全边界情形才能「确定性、可单测钉死」。**
文档给了两列/N 列/宽表/空单元格四条，缺：空记录头、空表头名、单列表、仅表头无记录、宽表拆行的**幂等陷阱**（续行缩进会被二次解析剥掉——CommonMark 段落续行剥前导空格，故续行**不得缩进**，§4.4）。本设计全部补齐并给黄金字符串。

### 2.3 诉求批逐条意见

| 诉求 | 意见 | 理由 |
|---|---|---|
| **T1 线性化** | ✅ 采纳，规则按 §4.4 定稿 | 原理正确；激活 `tables` 死标志用 offset 原文切片（比语法重建更保真，保留 `\|` 转义与行内语法） |
| **T2 纯文本投影** | ✅ 采纳，含 F1/F2/F8 修复；QQ 变化按 D3 明示 | caps 诚实优先；快速通道使微信**纯文本回复逐字节不变**，只有含 Markdown 语法的回复才变化——比文档预期的行为变化面更小 |
| **T3 投影下沉** | ✅ 采纳（D1），带快速通道 + 契约文档 + 系统消息回归 | 与 R16 透明封装同构；两 adapter 已在咽喉调 `prepare_outbound`，**零 adapter 改动**；不做则每个新宿主重蹈 R16 前困境 |
| 辅助：系统提示词渠道格式提示 | ⏸ 缓议（D5），投影落地后单独小 PR | 模型服从性不保证，确定性投影已是兜底；提示注入需 manager→agent 提示词接线，独立改动面，不捆绑 |

---

## 3. 架构总览

```
                        ┌────────────────────────────────────────────┐
 LLM Markdown ──────►   │ prepare_markdown_for_platform(text, feats) │
 （chatbot 回复 /       │  ① 纯文本快速通道：无语法 → 逐字节直通      │
  宿主直调推送 /        │  ② pulldown-cmark 事件投影：                │
  系统通知）            │     features=true  → 保留/直通该语法        │
                        │     features=false → 诚实降级（线性化等）   │
                        └──────────────────┬─────────────────────────┘
                                           │ 幂等：已投影文本再过一遍不变
                        ┌──────────────────▼─────────────────────────┐
                        │ prepare_outbound(text, caps)               │
                        │  = 内容投影 + 长度守卫（R16）               │
                        │  投影 → 分段/截断（顺序不可反：投影改变长度）│
                        └──────────────────┬─────────────────────────┘
                                           │
              ┌────────────────────────────┼────────────────────────────┐
              ▼                            ▼                            ▼
   flush_buffer（回复路径）      adapter.send_message 咽喉        edit_message 兜底
   只调 prepare_outbound         QQ:333 / WeChat:322 已就位       （同走 send）
   （删除 supports_markdown      （T3 后自动获得投影，无需改动）
    门控与重复投影调用）
                                           │
              ┌────────────────────────────┴───────────────┐
              ▼                                            ▼
   QQ：直接发送（仅 code_blocks 渲染）          微信：SDK filter_markdown 二次过滤
                                               （表格/栅栏/加粗已被投影掉 → 无害；
                                                 图片语法已变 [Image: alt] → 不再被 SDK 整体删除）
```

**投影语义定位**（写入模块文档）：清洗器不是「Markdown 渲染器」，是「平台可读性投影」。`MarkdownFeatures` 全 false = 纯文本投影，本就该跑；`supports_markdown` 退化为描述性声明（F9）。

**两个调用点、一份语义**：`flush_buffer` 与 `prepare_outbound` 都走同一个投影函数。回复路径投影两次（flush 一次、adapter 咽喉一次），靠幂等保证逐位不变；直调路径投影一次。投影必须先于分段（线性化会改变文本长度，表头名按记录重复展开通常变长）。

---

## 4. 详细设计

### 4.1 改动清单

| 文件 | 改动 | 诉求 |
|---|---|---|
| `crates/robit-chatbot/src/markdown.rs` | 快速通道；降级尊重 features（标题/代码块/引用/链接/删除线）；`render_table` → `linearize_table`；`tables=true` 原文直通；单元格捕获重构；模块文档更新 | T1+T2 |
| `crates/robit-chatbot/src/outbound.rs` | `prepare_outbound` = 投影 + 长度守卫；空文本快速路径；契约文档；测试迁移 | T3 |
| `crates/robit-chatbot/src/frontend.rs` | `flush_buffer` 删除 `supports_markdown` 门控，收敛为单次 `prepare_outbound(&text, &caps)`；微信 caps 集成测试 | T2+T3 |
| `crates/robit-chatbot/src/adapter.rs` | `send_message` 契约文档扩为「内容投影 + 长度守卫」；`supports_markdown` 语义注释；更正两处「SDK strips Markdown」错误注释（F10） | 文档 |
| `crates/robit-qq` / `crates/robit-wechat` | **零改动**（咽喉已就位） | — |
| `CHANGELOG.md` | 行为变化明示（微信投影、QQ 降级改善、出站 Markdown 契约） | 发版 |

### 4.2 纯文本快速通道（本设计的关键增强）

在 `prepare_markdown_for_platform` 入口：

```rust
/// 保守判定：文本不含任何可能触发 Markdown 构造的字符/形态时，逐字节直通。
fn is_plain_text(text: &str) -> bool {
    // 触发字符集（每个都有对应的 Markdown 构造）：
    //   * _ ~  强调/删除线/水平线      `  代码
    //   [ ] !  链接/图片/引用定义       |  表格
    //   > #    引用/标题                <  HTML/自动链接
    //   &      实体引用（&amp; → &）    \  反斜杠转义
    //   - +    列表/水平线              .  有序列表（N.）
    //   =      setext 标题下划线
    const TRIGGERS: &[char] = &['*','_','~','`','[',']','!','|','>','#','<','&','\\','-','+','.','='];
    if text.chars().any(|c| TRIGGERS.contains(&c)) {
        return false;
    }
    // 缩进代码块：行首 4 空格或 tab（无触发字符也能成块）
    !text.lines().any(|l| l.starts_with("    ") || l.starts_with('\t'))
}
```

- 命中 → `return text.to_string()`（**不追加换行、不 trim、零分配语义**）。
- 未命中 → 走既有事件投影。

**收益**：
1. 「≤cap 已投影文本幂等直通」升级为「**任何无语法文本逐字节直通**」——outbound.rs / frontend.rs 现有全部精确字节断言测试（`"A".repeat(n)`、`"你好世界"`、7000 字符长回复分段）**无需迁移**，测试迁移量从「几乎全部」降为「仅表格黄金例」。
2. 系统消息（确认提示 JSON、/help、错误通知、进度提示，F6）绝大多数不含触发字符 → 逐字节不变，T3 影响面收敛到真正含 Markdown 的文本。
3. 微信纯文本回复（T2 后）与现状逐字节一致，行为变化面最小化。
4. 省掉无语法文本的解析开销（直调高频小消息场景）。

**保守性论证**：触发集覆盖 pulldown-cmark 0.13 在当前 Options（TABLES/TASKLISTS/STRIKETHROUGH，无 smart-punctuation）下全部构造的必需字符；漏判只会导致「多解析一次」，不会导致「该投影的没投影」。快速通道本身幂等（直通文本再过仍直通）。

**残余边界**（登记，§5）：含触发字符但语义上是纯文本的内容（`2*3*4`、`a * b * c` 中成对星号）仍会被按 Markdown 消化——这是「出站即 Markdown」契约的固有代价，快速通道不解决、也不试图解决。

### 4.3 markdown.rs 投影重构

**a) 单元格捕获：事件路由取代 `strip_last_text` hack（F7）。**
`ctx.in_cell` 期间，行内事件直接写入 `cell_buf` 而非 output：

| 事件 | 路由到 cell_buf 的内容 |
|---|---|
| `Text` | 原文 |
| `Code` | 按 `inline_code`：`` `code` `` 或 `code` |
| `Strong`/`Emphasis`/`Strikethrough` | 按 features 发射标记或裸文本 |
| `Link` | 按 links 规则（§4.3-d），文本捕获进 link sink |
| `SoftBreak`/`HardBreak` | 空格（GFM 单元格无真实换行，防御性处理） |
| 其余 | 忽略 |

实现上把行内渲染抽成 `render_inline(event, features, ctx, sink: &mut String)`，output 与 cell_buf 共用；`End(TableCell)` 仍 `trim()` 入行。删除 `strip_last_text`。**单元格内容即「按 features 投影后的行内文本」**——线性化记录里的值自然继承平台能力（QQ 单元格内加粗已被剥除）。

**b) 降级尊重 features（T2 核心 + F1/F2/F8）：**

| 语法 | `features=true` | `features=false`（新语义） | 现状 false 行为 |
|---|---|---|---|
| headings | `#`×n + 文本（不变） | `bold=true` → `**文本**`；`bold=false` → **纯文本 + 空行** | 恒 `**` |
| code_blocks | ` ``` ` 栅栏（不变，QQ 逐位不变） | **去栅栏、代码内容原样保留** + 空行分隔（不缩进，保可复制性） | 恒栅栏 |
| blockquotes | **按行前缀 `> `（修复 F1**：quote_depth 计数，段落起始/SoftBreak 后发射前缀，嵌套 `>> `） | 无前缀，内容作普通段落 | 块首一个字面 `> ` |
| strikethrough | `~~文本~~`（**解析选项恒开**，修复 F2） | 剥标记留文本 | 字面 `~~` 直通 |
| links | `[text](url)`（不变） | **D2 决策**：推荐 `text (url)`（text 空或与 url 相同则只留 url）；否决则维持丢 URL | 丢 URL |
| tables | **原文直通**（§4.3-c） | **线性化**（§4.4） | 对齐伪表格 |
| images | 可选激活（F4）：`![alt](url)` | `[Image: alt]`（不变；微信侧因 F5 反成修复） | 恒 fallback |
| lists / task_lists | 标记 `- `/`N. ` 恒保留（纯文本自然约定，flag 为描述性；文档注明） | 同左 | 同左 |
| HardBreak | `"\n"`（F8 清理，行尾双空格去除；QQ 不可见差异） | `"\n"` | `"  \n"` |
| bold/italic/inline_code | 已尊重 features，不变 | 不变 | — |

**c) `tables=true` 原文直通（激活死标志）：**
主循环改用 `Parser::into_offset_iter()`（pulldown-cmark 0.13 支持，其余 handler 忽略 range）：

```rust
Event::Start(Tag::Table(_)) if features.tables => {
    ctx.verbatim_table = Some(range.start);   // 进入直通模式，抑制内部事件
}
Event::End(TagEnd::Table) if ctx.verbatim_table.is_some() => {
    let start = ctx.verbatim_table.take().unwrap();
    output.push_str(&text[start..range.end]); // 原文切片，字节级保真
    output.push_str("\n\n");
}
```

选原文切片而非「收集 rows 重建管道语法」：保留 `\|` 转义、行内语法、对齐空格，且天然幂等（二次投影切同一段原文）。`tables=false` 时维持 rows 收集 → 线性化。

**d) 链接降级（若 D2 采纳）：** `links=false` 时 `Start(Link)` 记录 url 并开启 link sink，行内文本捕获进 sink；`End(Link)` 发射 `text (url)`（text 为空或 text==url → 只发 url）。与单元格捕获共用 sink 机制。

### 4.4 线性化规则定稿（`linearize_table`，替换 `render_table`）

输入 `rows: &[Vec<String>]`，`rows[0]` 为表头（pulldown-cmark 将 body 行规范化为表头列数——单测钉死），单元格已 trim。

```text
cols = header.len()
cols == 0 或无记录行            → ""（仅表头无数据，无信息量）
cols == 1                      → 每个非空记录单元格一行，"\n" 连接
cols == 2（键值形态）           → 逐记录：
    key,val 均非空 → "{key}: {val}"
    仅 key        → "{key}"
    仅 val        → "{val}"
    均空          → 跳过该记录
    记录间 "\n"（紧凑，不空行）
cols >= 3（记录形态）           → 逐记录：
    head = r[0]；pairs = 对 i in 1..cols 且 r[i] 非空：
        header[i] 非空 → "{header[i]}: {r[i]}"；header[i] 空 → "{r[i]}"
    cols <= 5：单行 "{head} — {pairs.join(\" · \")}"
        head 空 → 仅 pairs；pairs 空 → 仅 head；均空 → 跳过
    cols > 5（宽表）：head 独占一行（非空时），pairs 每 3 个一行、
        " · " 连接，续行不缩进（幂等要求，见下）
    记录间 "\n\n"（空行分隔）
块尾统一 "\n\n"（交给全局 3+ 换行折叠与 trim_end 归一）
```

**分隔符约定**：记录头与属性间 ` — `（U+2014 两侧空格）；键值对间 ` · `（U+00B7 两侧空格）；键值间 `: `（ASCII 冒号+空格）。三者均非 Markdown 触发字符。

**黄金例**（需求文档持仓场景，逐字节钉死）：

```text
输入：
| 持仓 | 现价 | 盈亏 | 仓位 |
|---|---|---|---|
| 贵州茅台 | 105.00 | -4.59% | 28.0% |
| 五粮液 | 118.20 | -1.50% | 15.2% |

输出：
贵州茅台 — 现价: 105.00 · 盈亏: -4.59% · 仓位: 28.0%

五粮液 — 现价: 118.20 · 盈亏: -1.50% · 仓位: 15.2%
```

两列：`总资产: 290362.30\n仓位: 68.9%`。空单元格：`| 贵州茅台 | 105.00 | | 28.0% |` → `贵州茅台 — 现价: 105.00 · 仓位: 28.0%`。宽表（6 列）：

```text
贵州茅台
现价: 105.00 · 盈亏: -4.59% · 仓位: 28.0%
今开: 104.00 · 昨收: 109.99 · 成交量: 12345
```

**幂等陷阱与对策（设计要点）**：宽表续行**不得缩进**——CommonMark 段落续行会剥前导空格，缩进 2 空格的续行二次投影后缩进丢失 → 逐字节不等。不缩进 + 记录间空行已足够可读。同理，记录头/键若以 `# `、`> `、`- ` 等块级标记开头，二次投影会被重解释（罕见，登记 §5，黄金例覆盖真实数据形态）。负数 `-4.59%` 安全（`-` 后无空格不构成列表标记）。

### 4.5 outbound.rs：投影 + 长度守卫（T3）

```rust
pub fn prepare_outbound(text: &str, caps: &PlatformCaps) -> OutboundResult {
    // 1) 内容投影（空/纯空白文本保持原样，维持空段语义）
    let projected = if text.trim().is_empty() {
        text.to_string()
    } else {
        prepare_markdown_for_platform(text, &caps.markdown_features)
    };
    // 2) 长度守卫：既有逻辑原样作用于 projected（分段/截断/标记）
    length_guard(projected, caps)   // 现 prepare_outbound 函数体，改名内移
}
```

- **顺序不可反**：投影改变长度（线性化展开表头名通常变长），必须先投影后分段。
- **幂等**：已投影文本 = 无语法文本（快速通道直通）或已归一化文本（二次投影不变，§4.4 陷阱已规避）→ chatbot 回复路径（flush 已投影）经 adapter 咽喉逐位不变。
- **契约更新**（adapter.rs `send_message` 文档）：长度守卫契约扩为「内容投影 + 长度守卫」；明示**出站文本按 Markdown 源处理**——宿主推送纯文本若含成对 `*`/`_` 等显著字符会被消化（§5-R2）；框架系统通知经回归测试钉死投影稳定。
- 既有 outbound 测试因快速通道**全部原样通过**；新增投影组合测试（§4.7）。

### 4.6 frontend.rs：门控删除（T2 接线）

```rust
// 现状（256-265）：
let prepared = if caps.supports_markdown { prepare_markdown_for_platform(...) } else { text.clone() };
let outbound = prepare_outbound(&prepared, &caps);
// 改为（T3 采纳时）：
let outbound = prepare_outbound(&text, &caps);   // 投影在守卫内，单次调用
```

- DB 存档仍用原始 `text`（不变，存档保真）。
- 若 D1 否决 T3：退化为 `let prepared = prepare_markdown_for_platform(&text, &caps.markdown_features);`（无条件投影），其余不变——T2 可独立落地。
- `supports_markdown` 字段保留，注释改为描述性语义（F9）。

### 4.7 测试计划（映射需求 §3 验收标准）

**markdown.rs（验收 1、2）**
- 线性化黄金例：两列 / N 列（持仓例逐字节）/ 宽表 >5 列 / 空单元格 / 空记录头 / 空表头名 / 单列 / 仅表头 → 空 / CJK 内容
- 残留断言：输出不含 `|` 管道与 `-+-`/`|---|` 分隔行
- **幂等**：全部黄金例 `project(project(x)) == project(x)` 逐字节
- `tables=true` 原文直通（含 `\|` 转义单元格、行内加粗）；二次投影仍直通
- all-false 纯文本投影：标题/加粗/栅栏/引用/表格/删除线/图片 → 无字面 Markdown 标记（列表标记除外）
- QQ 回归：既有 12 个单测除 `converts_table_to_aligned_text`（改写为线性化黄金例）外全部原样通过；新增 QQ 标题纯文本化、引用去 `> `、删除线剥除三个**改善性变化**钉子测试（D3）
- `blockquotes=true` 按行前缀（F1，feishu features）
- 快速通道：无语法文本逐字节直通（含尾随空白）；每类触发字符确实进入投影
- 单元格行内代码/加粗捕获（路由重构回归）

**outbound.rs（验收 4）**
- 长表格文本直调（wechat caps）→ 线性化 + 分段组合正确；QQ caps 直调 → 线性化 + 截断
- ≤cap 已投影文本幂等直通；空/纯空白文本原样
- 系统消息回归（F6）：确认提示（含 pretty JSON 参数的 bash/edit 工具）、/help 文本、进度提示、错误通知 → 投影稳定（快速通道直通或黄金例）
- 既有 13 个长度守卫测试原样通过（快速通道保证）

**frontend.rs（验收 3）**
- 微信 caps MockSender（`supports_markdown=false`、全 false features、无 edit）：含表格回复 → 发送内容为线性化形态、无原始管道语法；纯文本回复逐字节不变
- QQ 回复路径既有集成测试全部回归

**真机（验收 5）**：子木侧发版后走查（Web 表格正常 / 微信线性化 / QQ 同款），与 P7-T8 合并执行。

---

## 5. 风险与边界（登记）

| # | 风险 | 评估 | 对策 |
|---|---|---|---|
| R1 | 线性化变长（表头名按记录重复）→ 长表格更易触发分段/截断 | 固有代价；可读的截断优于不可读的全量 | 宽表拆行缓解单行长度；截断标记沿用 R16 |
| R2 | 「出站即 Markdown」契约下，含成对 `*`/`_` 的纯文本被消化（`2*3*4`→`234`；JSON 参数内标记对） | 快速通道已覆盖绝大多数系统消息；残余场景在中文助理域罕见（乘号惯用 ×） | trait 契约明示 + 系统消息回归测试；宿主纯文本推送避免显著字符（文档示例） |
| R3 | 幂等边界：记录头/键以 `# `/`> `/`- ` 开头时二次投影重解释；硬切断裂的未闭合栅栏在 adapter 二次投影时补闭合 | 罕见；后者实为改善（渲染正确） | 黄金例覆盖真实形态；边界登记于模块文档；不做过度工程 |
| R4 | 微信 SDK 二次过滤与投影冲突 | 已核实 SDK 保留集（表格/栅栏/加粗/引用/水平线）恰为投影消除集 → 无害；SDK 删图片语法反被投影修复（F5） | 无需动作；真机走查确认 |
| R5 | QQ 改善性变化被误读为回归 | 标题/引用/删除线/链接输出确实变化 | D3 拍板 + CHANGELOG 明示 + 钉子测试 |
| R6 | 双投影解析开销（回复路径 flush + adapter 各一次） | pulldown 事件解析 O(n)，回复文本量级下可忽略；快速通道使无语法文本零解析 | 无需动作 |

---

## 6. 决策点（需 robit 维护者拍板）

| # | 决策 | 推荐 | 备选与代价 |
|---|---|---|---|
| **D1** | T3 投影下沉 `prepare_outbound` | **做**（快速通道 + 契约文档兜底） | 不做：trait 文档写明直调方自行调清洗器；每个新宿主重蹈 R16 前困境 |
| **D2** | `links=false` 降级形态 | **`text (url)`**（信息保全，助理场景链接是有效载荷；既有 QQ 链接测试仍通过） | 维持丢 URL：QQ 严格逐位不变，但微信/QQ 用户丢失来源链接 |
| **D3** | QQ 标题/引用/删除线改善性变化 | **接受**（caps 诚实优先，CHANGELOG 明示） | 拒绝：QQ 保留字面 `**`/`> `/`~~`，与 T2 语义自相矛盾，不建议 |
| **D4** | 版本号 | **0.5.0**（双平台可见行为变化，0.x 语义下 minor 承载） | 0.4.2：仅当 D2/D3 均否决、变化面收敛到表格时可选 |
| **D5** | 系统提示词渠道格式提示 | **缓议**，投影落地后独立小 PR（manager 构造 agent 时按 `caps.markdown_features.tables==false` 追加渠道提示，可挂 `app.bot.system_prompt` 链路） | 捆绑本次：扩大改动面到 manager/agent 接线，收益不确定（模型服从性不保证） |
| **D6** | F1 引用修复 / F4 images 激活 / F3 任务列表勾选态 | F1 **随本次修**（正确性缺陷，现网无影响）；F4 可选对称激活；F3 **后续**（`[x] `/`[ ] ` 前缀，独立小改） | 全缓：feishu 接入时再补，登记 backlog |

---

## 7. 实施分步与发版

**Step 1 — markdown.rs 投影语义（T1+T2 核心，纯函数，可独立评审）**
快速通道 → 降级尊重 features（含 F1/F2/F8）→ 单元格路由重构 → `linearize_table` → `tables=true` 原文直通 → 全部 §4.7 markdown 测试。此步不改任何调用方，现网行为不变（死代码路径就绪）。

**Step 2 — 接线（T2+T3 落地）**
outbound.rs 投影+守卫 → frontend.rs 门控删除 → adapter.rs 契约/注释更新 → outbound/frontend 测试（新增 + 回归）。此步产生全部可见行为变化，微信泄漏路径与 QQ 伪表格同时修复。
（若 D1 否决：Step 2 仅做 frontend.rs 无条件投影，outbound.rs 不动。）

**Step 3 — 发版与消费**
`cargo test -p robit-chatbot -p robit-qq -p robit-wechat` + clippy → CHANGELOG（行为变化明示）→ 版本三处 bump（根 version / workspace.dependencies / Cargo.lock，CLAUDE.md 发版流程）→ 0.5.0 tag 发版 → 子木升版消费（推送链 T1 即得、微信会话回复 T2 自动修复、直调面 T3 补完整）→ 真机走查（验收 5）。

**预估改动量**：markdown.rs ~+250/-120 行（含测试），outbound.rs ~+60/-10，frontend.rs ~+80/-10（含微信集成测试），adapter.rs 文档 ~30 行。无新依赖（pulldown-cmark 0.13 已在 workspace）。

---

## 8. 可选后续（不进本次范围，登记 backlog）

1. **渠道格式提示注入**（D5）：源头减少表格产出，与确定性投影两条腿走路。
2. **任务列表勾选态**（F3）：`task_lists=false` 时发射 `[x] `/`[ ] ` 纯文本前缀，保住 done/todo 语义。
3. **代码块语言标签**：`code_blocks=true` 时保留 ` ```lang `（现状丢弃 lang，QQ 渲染无碍，信息 minor）。
4. **feishu 接入时**：全 features 投影黄金例（feishu 预留位已就位；F4 images 激活已随本次落地）。

## 9. 实施记录（2026-11-22，0.5.0）

按 §7 三步实施完毕，`cargo test -p robit-chatbot -p robit-qq -p robit-wechat` 全绿（123 + 21 + 17），改动文件 clippy 无新增警告。实施中对设计的两处修正（均由单测发现并钉死）：

1. **快速通道判定扩展行缘空白**：原触发字符集漏了「行尾双空格 = 硬换行」「段落行首/尾空白被 CommonMark 剥除」这类**无触发字符**的形态。`is_plain_text` 增加「任一行以空格/tab 开头或结尾 → 走慢路径」判定（同时涵盖 4 空格缩进代码块），保守性论证不变。
2. **嵌套引用黄金例修正**：pulldown 在外层段落与嵌套引用之间插入段落分隔，且每层前缀为 `> `（depth 2 → `> > `），故 `> 外\n> > 内` 投影为 `> 外\n\n> > 内\n`（幂等已验证），而非设计稿假设的紧凑形态。

测试迁移实况：outbound/frontend 既有精确字节测试因快速通道**全部原样通过**，唯二例外是两处「代码栅栏内不分段」测试——其输入含 `` ` `` 触发字符且微信 caps 会剥栅栏，按设计意图迁移到 QQ features（`code_blocks=true`）的非编辑 caps，栅栏保护启发式本身不变；另新增微信剥栅栏投影测试。

发版状态：版本三处 bump + Cargo.lock 同步 + CHANGELOG 0.5.0 条目已完成并本地提交；**tag 未打、未推送**——crates.io 发布不可逆，待维护者确认后按 CLAUDE.md 发版流程执行（`git tag -a v0.5.0` + push 触发 CI）。子木消费与真机走查（验收 5）在发版后执行。