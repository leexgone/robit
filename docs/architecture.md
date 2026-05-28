# 架构设计

## Agent 运行时

Agent 采用**事件驱动的消息循环**：

```
用户输入
  ↓
Agent Loop
  ├── 组装上下文（历史消息 + 工具定义 + 系统提示词）
  ├── 调用 LLM（流式）
  ├── 解析响应
  │     ├── 纯文本 → 发射 TextDelta 事件，回到等待输入
  │     └── 工具调用 → 发射 ToolCallRequested 事件
  │           ├── 等待前端确认（可配置自动批准）
  │           ├── 确认后执行工具
  │           ├── 发射 ToolCallResult 事件
  │           ├── 结果回填到消息历史
  │           └── 回到「调用 LLM」（继续循环）
  └── 上下文管理
        ├── Token 计数（防止溢出）
        └── 历史压缩策略（摘要 / 截断）
```

**关键设计决策：**

- **循环控制权**：LLM 决定下一步（继续调工具还是结束），Agent 只负责执行，不做决策
- **工具执行策略**：默认写操作需用户确认，读操作自动执行，可通过配置覆盖
- **上下文窗口**：先用简单策略（超出时截断最早消息），后续再引入摘要压缩
- **并发**：Agent 循环在 tokio 异步任务中运行，前端通过 channel 订阅事件

## Frontend Trait

`robit-agent` 定义 `Frontend` trait 作为前端抽象接口。Agent 不知道前端是 TUI、飞书还是 QQ，只通过 trait 交互。

```rust
#[async_trait]
pub trait Frontend: Send + Sync {
    /// Agent 推送事件给前端（文本、工具调用、错误等）
    async fn on_event(&self, event: AgentEvent) -> Result<()>;

    /// 请求用户确认工具调用（阻塞等待）
    async fn request_tool_confirmation(
        &self,
        tool_call: &ToolCall,
    ) -> Result<ConfirmationResult>;

    /// 接收前端发来的消息（用户输入、取消、确认回复）
    fn event_receiver(&self) -> mpsc::Receiver<FrontendMessage>;
}
```

**各前端实现：**

| 前端 | 输入方式 | 输出方式 | 确认方式 |
|------|---------|---------|---------|
| `robit-tui` | 键盘直接输入 | 终端实时渲染（流式） | 弹窗选择 |
| `robit-feishu`（计划） | 消息事件推送 | 飞书 API 发送消息 | 消息卡片按钮 |
| `robit-qq`（计划） | 消息事件推送 | QQ API 发送消息 | 消息卡片按钮 |

**TUI 与消息平台的差异处理：**

- 流式输出：TUI 逐字显示；飞书/QQ 可选择分段发送或完成后一次性发送
- 会话模型：TUI 单进程单会话；飞书/QQ 天然多用户多会话

## 会话管理

当前 MVP 实现单会话，但 `SessionId` 从第一天引入，为未来多会话做准备。

```rust
pub struct AgentSession {
    session_id: SessionId,
    history: Vec<Message>,
    // 上下文窗口管理
}

pub struct Agent {
    llm_client: Arc<LlmClient>,
    tools: Arc<ToolRegistry>,
    sessions: HashMap<SessionId, AgentSession>,
}
```

**MVP 阶段**：只有一个默认 session，HashMap 中只有一条记录。

**多会话阶段**（飞书/QQ 接入时）：每个用户/群聊对应一个 `AgentSession`，共享 `LlmClient` 和 `ToolRegistry`。

## 技能系统

技能是**预定义的提示词模板**，以 YAML frontmatter + Markdown body 格式存储。系统在启动时加载技能，按需注入到 Agent 的系统提示词中。

### 技能文件格式

每个技能是一个 `.md` 文件，采用 YAML frontmatter + Markdown body 结构：

```markdown
---
name: code-review
description: 审查代码变更，关注正确性、性能和安全性
version: 1.0.0
triggers:
  - /review
  - /代码审查
tools_required:
  - bash
  - read
  - grep
enabled: true
---

# 代码审查技能

请按以下步骤审查代码变更：

## 步骤

1. 运行 `git diff` 查看当前变更
2. 逐文件分析变更内容
3. 对每个文件给出以下维度的评价：
   - **正确性**：逻辑是否正确，边界条件是否处理
   - **性能**：是否存在性能隐患
   - **安全性**：是否引入安全风险
   - **可维护性**：代码是否清晰易懂
4. 给出整体评价和改进建议

## 输出格式

使用结构化输出，每个文件单独评审，最后给出总结。
```

### Frontmatter 字段定义

| 字段 | 类型 | 必填 | 说明 |
| ---- | ------ | ---- | ---- |
| `name` | `string` | 是 | 技能唯一标识符，用于内部引用 |
| `description` | `string` | 是 | 技能描述，展示给用户和 Agent |
| `version` | `string` | 否 | 语义版本号 |
| `triggers` | `string[]` | 否 | 触发命令列表（如 `/review`），为空则仅通过系统提示词注入 |
| `tools_required` | `string[]` | 否 | 该技能依赖的工具列表，用于检查工具可用性 |
| `enabled` | `bool` | 否 | 是否启用，默认 `true` |

### 技能加载优先级

```txt
~/.robit/skills/          ← 全局技能（低优先级）
cwd/.robit/skills/        ← 项目技能（高优先级，同名覆盖全局）
```

### 技能注入时机

- **Agent 启动时**：将所有 `enabled: true` 的技能描述注入系统提示词，让 LLM 知晓可用技能
- **用户触发时**：当用户输入匹配 `triggers` 中的命令时，将对应技能的完整 Markdown body 作为系统消息注入当前对话
