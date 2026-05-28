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
| `ToolCallRequested(ToolCall)` | LLM 请求调用工具，等待前端确认 |
| `ToolCallResult(id, Result)` | 工具执行结果，回填到对话历史 |
| `TurnComplete` | 本轮对话结束，Agent 等待新输入 |
| `Error(AgentError)` | Agent 运行错误 |

### 前端 → Agent（`FrontendMessage`）

| 消息 | 说明 |
|------|------|
| `UserInput(String)` | 用户新消息 |
| `Cancel` | 取消当前操作 |
| `ConfirmationResponse { id, approved }` | 工具调用确认回复 |

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
