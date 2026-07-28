# 渐进式上下文压缩实现计划

> **目标**：将当前"一次截断全删 + 单条摘要"的单层压缩，改进为"分段摘要 + 合并 + 丢弃"的渐进式压缩，改善长对话的上下文保留质量，同时控制实现复杂度和信息失真。
> **对应路线图条目**：[上下文管理增强 — 渐进式压缩](../roadmap.md#上下文管理增强)

## 背景

当前实现（`crates/robit-agent/src/context.rs` 的 `maybe_truncate`）存在以下问题：

1. **断崖式压缩**：一旦触发截断，一次性删除所有超量轮次，只保留最近 `min_keep_rounds` 轮，早期历史全部揉进一条摘要。
2. **摘要粒度粗**：单条摘要可能覆盖十几甚至几十轮对话，LLM 容易丢失关键细节。
3. **没有时间层次感**：Agent 只看到一坨"早期对话摘要"，不知道事情发生的先后顺序。

本方案用"一层半"渐进式压缩来改进：历史被逐步压缩为多条细粒度摘要段，摘要段过多时合并最旧的，合并达到上限后丢弃。在不大幅增加复杂度的前提下获得明显的质量提升。

## 设计概述

### 历史结构

```
┌─────────────────────────────────────────────────────┐
│  System Prompt + Skills（始终保留）                    │
├─────────────────────────────────────────────────────┤
│  [丢弃通知]（可选，最多 1 条）                          │
├─────────────────────────────────────────────────────┤
│  [摘要段 1]（最旧，可能已合并过几次）                    │
│  [摘要段 2]                                             │
│  ...                                                    │
│  [摘要段 N]（最新的摘要段）                              │
├─────────────────────────────────────────────────────┤
│  近期完整轮次（原文，min_keep_rounds 起）              │
└─────────────────────────────────────────────────────┘
```

### 摘要段标识

通过 `ChatCompletionRequestMessage::User` 的 `name` 字段区分：

| name 值 | 含义 |
|---------|------|
| `summary_segment` | 摘要段（未合并过） |
| `summary_segment_m1` | 摘要段（已合并 1 次） |
| `summary_segment_m2` | 摘要段（已合并 2 次，达上限） |
| `discard_notice` | 丢弃通知（最多 1 条） |
| `system_notice` | 旧版截断通知（向后兼容，识别为 0 次合并的摘要段） |

内容格式示例：
```
[Summary (Rounds 4-6): 用户要求实现登录功能，Agent 创建了 auth.rs 模块 ...]
```

### 压缩优先级

每次 `maybe_truncate` 只执行一次操作（保证延迟可控），按以下优先级选择：

1. **生成新摘要段**：完整轮次足够多时，取最旧的 `rounds_per_summary` 个完整轮次压缩为一条摘要段。
2. **合并摘要段**：摘要段数量超过 `max_summary_segments` 且最旧段未达合并上限时，合并最旧的 `merge_count` 条。
3. **丢弃最旧摘要段**：摘要段数量超限且最旧段已达合并上限时，直接丢弃最旧段，并确保有丢弃通知。
4. **退化截断**：以上都不可行但 token 仍超阈值时，退化为当前的直接删除行为。

### 合并次数上限与丢弃机制

- 每条摘要最多被合并 `max_merges_per_segment` 次（默认 2）。
- 达到上限后不再合并，直接丢弃。
- 丢弃时确保存在一条丢弃通知，告知 LLM "更早的历史已被丢弃"。
- 目的：控制信息失真的上限。与其给 LLM 一条被反复合并、严重失真的摘要，不如诚实告知已丢弃。

## 配置参数

所有参数位于 `[app.context]` 下。新增参数均有默认值，旧配置文件无需修改。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `progressive_compression` | bool | true | 是否启用渐进式分段压缩（false = 退化为旧的单层单次压缩） |
| `rounds_per_summary` | usize | 3 | 每个摘要段覆盖的完整轮数 |
| `max_summary_segments` | usize | 5 | 最多保留的摘要段数量，超过则合并/丢弃最旧的 |
| `merge_count` | usize | 2 | 每次合并的摘要段数量 |
| `max_merges_per_segment` | usize | 2 | 单条摘要最多被合并次数，超过则丢弃 |

> 既有参数（`truncation_ratio`、`min_keep_rounds`、`compression_enabled`、`compression_token_threshold` 等）保持不变。

### 默认行为估算

以 64k 上下文、每轮约 2000 token 为例：
- 摘要层最多 5 段，每段覆盖 3 轮 ≈ 15 轮历史有摘要覆盖
- 加上最近 3 轮完整原文 ≈ 总共能追溯约 18 轮历史
- 超过后，最旧摘要经过最多 2 次合并后开始丢弃

## 改动范围

### 1. `crates/robit-ai/src/config.rs`

`ContextConfig` 新增 5 个可选字段：
- `progressive_compression: Option<bool>`
- `rounds_per_summary: Option<usize>`
- `max_summary_segments: Option<usize>`
- `merge_count: Option<usize>`
- `max_merges_per_segment: Option<usize>`

### 2. `crates/robit-agent/src/context.rs`

**新增类型**：
- `TruncationAction` 枚举：`NewSegment` / `MergeSegments` / `TruncateOnly`
- `TruncationResult` 新增 `action: TruncationAction` 字段

**新增辅助函数**：
- `is_summary_segment(msg) -> bool` — 判断一条消息是否为摘要段
- `get_merge_level(msg) -> usize` — 获取摘要段的合并次数（0-2）
- `find_summary_segments(messages) -> Vec<(usize, usize, String)>` — 扫描历史，返回所有摘要段的（位置，合并次数，内容）
- `find_discard_notice_pos(messages) -> Option<usize>` — 查找丢弃通知的位置
- `make_summary_placeholder(rounds: Range<usize>) -> ChatCompletionRequestMessage` — 生成摘要占位消息
- `make_merge_placeholder(count: usize) -> ChatCompletionRequestMessage` — 生成合并占位消息
- `make_discard_notice() -> ChatCompletionRequestMessage` — 生成丢弃通知消息

**重构函数**：
- `maybe_truncate()` — 重写为渐进式逻辑，保留退化路径作为兜底

**新增测试**（约 8-10 个）：
- 单次分段压缩（生成新摘要段）
- 多次压缩后摘要段数量不超限
- 摘要合并触发与合并次数递增
- 达合并上限后丢弃
- 丢弃通知存在性（有且仅有一条）
- 向后兼容：旧的 `system_notice` 消息被识别为摘要段
- `progressive_compression = false` 时退化为旧行为
- 极端情况的退化截断

### 3. `crates/robit-agent/src/agent.rs`

**新增函数**：
- `merge_summaries(llm_client, summaries: &[String]) -> String` — 调用 LLM 合并多条摘要为一条

**修改逻辑**（两处压缩处理）：
- `with_history()` 中的 pending compression 处理
- `run()` 循环中每次 tool call 后的压缩处理

根据 `TruncationResult.action` 分支处理：
- `NewSegment` → 调用 `generate_summary`（现有逻辑）
- `MergeSegments` → 调用 `merge_summaries`
- `TruncateOnly` → 不做 LLM 调用

`pending_truncation` 字段类型不变，靠 `TruncationResult.action` 区分操作。

### 4. `docs/roadmap.md`

更新"上下文管理增强"条目勾选状态。

### 5. `CLAUDE.md`

更新 `[app.context]` 配置章节，加入新增的 5 个参数说明。

### 6. `README.md` & `README.cn.md`

更新"Agent 运行时"功能描述中的上下文管理部分，反映渐进式压缩。

## 实现步骤（按依赖顺序）

### Step 1: 配置结构扩展

修改 `crates/robit-ai/src/config.rs` 的 `ContextConfig`，新增 5 个可选字段。

修改 `crates/robit-agent/src/context.rs` 的 `ContextManager`，新增对应字段和默认值，在 `new()` 中从 `ContextConfig` 读取。

### Step 2: 辅助函数与类型

在 `context.rs` 中新增 `TruncationAction` 枚举、扩展 `TruncationResult`，实现所有识别与占位消息辅助函数。

### Step 3: 渐进式 `maybe_truncate`

重写 `maybe_truncate` 函数，实现四级优先级的渐进压缩逻辑。保留旧逻辑作为 `progressive_compression = false` 时的退化路径和极端情况兜底。

### Step 4: Agent 侧 `merge_summaries` 与适配

在 `agent.rs` 中新增 `merge_summaries` 函数。修改两处压缩处理逻辑，根据 `action` 分支。

### Step 5: 测试

为 `context.rs` 补充所有新增场景的单元测试。

### Step 6: 文档更新

更新 `roadmap.md`、`CLAUDE.md`、`README.md`、`README.cn.md`。

## 向后兼容

| 场景 | 行为 |
|------|------|
| `progressive_compression = false` | 完全等价于当前单层单次压缩 |
| 旧会话恢复（含 `name="system_notice"` 的消息） | 识别为 1 个合并次数 = 0 的摘要段，后续正常工作 |
| 旧配置文件（没有新增字段） | 全部使用默认值，渐进压缩默认开启 |

## 风险与注意事项

1. **合并摘要的信息损失**：每合并一次信息约损失 30%，默认最多合并 2 次，最旧摘要的信息保留度约 50%。这是有意的权衡 — 达到上限后宁可丢弃也不继续失真。
2. **压缩触发频率**：渐进式每次只压缩 3 轮，触发频率比旧方案高（每增加几轮就可能触发一次）。每次压缩需要一次 LLM 调用，有延迟和成本。如果用户觉得太频繁，可以调大 `rounds_per_summary`。
3. **初始化时的多段压缩**：`with_history` 恢复长会话时，可能需要多次压缩才能降到阈值以下。当前 pending 机制只处理一次，需要改为循环处理直到低于阈值。

## 估算代码量

| 文件 | 新增行数 | 修改行数 |
|------|----------|----------|
| `robit-ai/src/config.rs` | ~15 | 0 |
| `robit-agent/src/context.rs` | ~200 | ~80 |
| `robit-agent/src/agent.rs` | ~50 | ~30 |
| 测试 | ~150 | 0 |
| 文档 | ~80 | ~20 |
| **合计** | **~495** | **~130** |
