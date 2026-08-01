# 通信协议

## LLM 消息层（`robit-ai`）

统一消息格式，兼容 OpenAI 协议，适配各提供商：

```rust
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

pub struct Message {
    pub role: Role,
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,      // assistant 发出的工具调用
    pub tool_call_id: Option<String>,   // tool 结果回填时使用
}

pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,  // JSON string，各工具自行解析
}
```

> **说明**：`arguments` 使用 JSON string 而非强类型，保持协议层的通用性。工具层负责解析各自的参数结构。

## Agent 事件层（`robit-agent` ↔ Frontend）

### Agent → 前端（`AgentEvent`）

| 事件 | 说明 |
|------|------|
| `TextDelta(String)` | 流式文本片段，前端逐段渲染 |
| `ToolCallRequested { tool_call_id, name, arguments }` | LLM 请求调用工具，等待前端确认 |
| `ToolCallResult { tool_call_id, result }` | 工具执行结果，回填到对话历史。`result.is_pending=true` 时为异步任务占位，实际结果后续由 `AsyncToolCompleted` 送达 |
| `TurnComplete` | 本轮对话结束，Agent 等待新输入 |
| `Error(AgentError)` | Agent 运行错误 |
| `SkillTriggered { name, description }` | 技能被触发（前端可选展示） |
| `AsyncToolCompleted { task_id, tool_call_id, result }` | 异步后台任务完成（成功/失败/取消）。结果已回灌对话历史并唤醒 LLM，前端可用于更新任务面板 |

### 前端 → Agent（`FrontendMessage`）

| 消息 | 说明 |
|------|------|
| `UserInput(String)` | 用户新消息 |
| `Cancel` | 取消当前操作（取消所有进行中的异步后台任务） |
| `ConfirmationResponse { id, approved }` | 工具调用确认回复 |
| `CancelTask { task_id }` | 取消指定的异步后台任务 |

## 消息流向

```
[前端]
  │
  │── UserInput ──────────────────────────────────────► [Agent]
  │                                                        │
  │                                                        ├─ 组装上下文
  │                                                        ├─ 调用 LLM
  │                                                        │
  │◄── TextDelta（多次）────────────────────────────────────┤
  │                                                        │
  │◄── ToolCallRequested ─────────────────────────────────┤
  │                                                        │
  │── ConfirmationResponse ──────────────────────────────►│
  │                                                        │
  │                                                        ├─ 执行工具
  │                                                        │
  │◄── ToolCallResult ────────────────────────────────────┤
  │                                                        │
  │                                                        └─ 继续循环...
  │                                                        │
  │◄── TurnComplete ──────────────────────────────────────┘
```

### 异步工具执行流程

长耗时工具（如 `generate_image`）走异步路径。工具在 `execute` 内部自行决定是否异步（运行时自适应，通常依据 provider 协议），调用 `ctx.async_runner.submit(..)` 提交后台 task 并返回 `ToolResult::pending(..)` 占位：

```
[Agent] run_one_step
  │── 工具 execute 返回 pending(task_id) + 后台 spawn work
  │── 占位 content 作为 tool message 入历史 → 发 ToolCallResult(is_pending=true)
  │── LLM 可继续其他工作或结束本轮
  │   ...（后台 task 运行；Agent 在 select! 上等待用户输入或任务完成）...
  │◄── AsyncTaskDone（后台完成）── from spawned task
  │── 追加 user 通知消息（任务结果）入历史
  │── 发 AsyncToolCompleted 给前端
  │── 唤醒 LLM（run_agent_loop）处理通知
  │◄── TextDelta / TurnComplete
```

**取消**：前端发 `Cancel`（取消全部）或 `CancelTask { task_id }`（取消指定），Agent 触发对应任务的 `CancellationToken`，后台 task 走取消分支后回灌"已取消"结果。Agent 退出（Drop）时也会取消所有未完成任务。

**查询**：`query_task` 工具可查询任务状态（pending/completed/failed/cancelled）与结果摘要。完成任务的结果本已通过通知送达，该工具用于查仍在进行的任务或复核状态。
