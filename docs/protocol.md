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
| `ConfirmationResponse { id, approved }` | 工具调用确认回复（**保留变体**：现行实现的确认走 `Frontend::request_tool_confirmation` 的返回值，Agent 收到此消息只 warn） |
| `CancelTask { task_id }` | 取消指定的异步后台任务 |

## Web JSON 协议（`robit-web`）

`robit-web` 为 `AgentEvent` 提供官方 JSON 序列化 schema（`WebEvent`），是 Web 客户端（JS/TS）与 robit 之间的稳定 wire 契约。每个事件一个 JSON 对象，内部 `type` 标签、snake_case 命名：

| wire 事件（`type`） | 字段 | 来源 |
|---|---|---|
| `text_delta` | `text` | `AgentEvent::TextDelta` |
| `tool_call` | `tool_call_id` `name` `arguments` | `AgentEvent::ToolCallRequested` |
| `tool_result` | `tool_call_id` `is_error` `content` | `AgentEvent::ToolCallResult` |
| `skill_triggered` | `name` `description` | `AgentEvent::SkillTriggered` |
| `async_tool_completed` | `task_id` `tool_call_id` `is_error` | `AgentEvent::AsyncToolCompleted` |
| `confirm_request` | `tool_call_id` `name` `arguments` | 桥内 `request_tool_confirmation` 被调用时推送（非 AgentEvent 投影） |
| `turn_complete` | — | `AgentEvent::TurnComplete` |
| `error` | `message` | `AgentEvent::Error`（Display 文案） |

**确认握手**：客户端收到 `confirm_request` 后经宿主自己的 HTTP 端点把答复送回；宿主调用 `WebBridge::resolve_confirm(tool_call_id, approved)` 唤醒挂起的 `request_tool_confirmation`（Web 路径不使用 `FrontendMessage::ConfirmationResponse`）。responder 被 drop（新请求顶替、`reject_pending_confirms`、桥销毁）一律按**拒绝**处理。

**消息注入**：用户输入由宿主经 `WebBridge::send_user_text` / `send_message` 注入（内部构造 `FrontendMessage`）；Agent 循环退出后注入返回 `FrontendGone`，宿主应据此降级（如 HTTP 503）。

**宿主职责边界**：SSE/WS 编码、keep-alive、断线重连、`connected` 等元数据首帧都在宿主侧——`robit-web` 零 HTTP 框架依赖，只保证事件投影与通道语义。

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
