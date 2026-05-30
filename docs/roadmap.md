# 构建路线图

## 阶段 1：LLM API 层（`robit-ai`） ✅ 已完成

**目标**：能够与 LLM 完成一次流式对话。

- [x] 定义 `Message`、`ToolCall` 等核心数据结构（使用 `async-openai` 类型）
- [x] 实现 OpenAI 兼容协议的统一 HTTP 客户端（`LlmClient` 封装）
- [x] 适配提供商（QWen、DeepSeek 等 OpenAI 兼容协议）
- [x] 流式响应（SSE）解析与回调（`chat_stream` 方法）
- [x] 配置加载（统一 `robit.toml`，支持 `${ENV_VAR}` 替换）

**验证**：`cargo run -p robit-chat`（`examples/robit-chat`）启动 REPL 交互式对话测试。✅ 已通过

## 阶段 2：Agent 运行时（`robit-agent`） ✅ 已完成

**目标**：Agent 能够调用工具完成一个简单编程任务。

- [x] Agent 事件驱动循环（`agent.rs` — 流式 LLM 调用 + tool call 组装 + 多轮循环）
- [x] `Frontend` trait 定义（`frontend.rs` — `on_event` + `request_tool_confirmation`）
- [x] 工具注册与执行框架（`tool/mod.rs` — `Tool` trait + `ToolRegistry` + `ToolContext`）
- [x] 实现核心工具：`bash`（跨平台 shell）、`read`（带行号 + 截断）
- [x] 会话管理（单会话 + `SessionId`，上下文截断两层策略）
- [x] 提示词系统（`prompt.rs` — 动态组装系统提示词）
- [x] 上下文管理（`context.rs` — 输出截断 + 历史按轮次截断 + token 估算）

**验证**：`cargo run -p robit-agent-cli` 启动命令行 Agent，使用 `read`/`bash` 工具完成任务。

## 阶段 3：TUI 前端（`robit-tui`）

**目标**：完整的终端交互体验，可用于日常编程。

- [ ] 实现 `Frontend` trait 的 TUI 前端
- [ ] 流式文本显示（Markdown 渲染）
- [ ] 工具调用状态展示与用户确认交互
- [ ] 对话历史管理
- [ ] 跨平台终端适配（Windows / Linux / macOS）

**验证**：用 robit 完成一个真实的编程任务（如修复一个 bug）。

## 阶段 4：扩展

**目标**：扩展工具、技能和多平台接入。

- [ ] 补齐工具：`edit`、`write`、`grep`、`find`、`ls`
- [ ] 技能系统实现（Markdown/YAML 模板，系统提示词注入）
- [ ] 更多 LLM 提供商适配（QWen、其他）
- [ ] 上下文压缩策略（摘要 / 智能截断）
- [ ] 代码高亮（引入 `syntect`）
- [ ] Token 精确计数（引入 `tiktoken-rs`）
- [ ] 飞书前端（`robit-feishu`）
- [ ] QQ 前端（`robit-qq`）
- [ ] 多会话管理完善
