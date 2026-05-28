# 构建路线图

## 阶段 1：LLM API 层（`robit-ai`）

**目标**：能够与 LLM 完成一次流式对话。

- [ ] 定义 `Message`、`ToolCall` 等核心数据结构
- [ ] 实现 OpenAI 兼容协议的统一 HTTP 客户端
- [ ] 适配 DeepSeek 提供商（认证、模型列表）
- [ ] 流式响应（SSE）解析与回调
- [ ] 配置加载（`llms.toml`、`.env`）

**验证**：CLI 直接对话测试，不依赖 Agent。

## 阶段 2：Agent 运行时（`robit-agent`）

**目标**：Agent 能够调用工具完成一个简单编程任务。

- [ ] Agent 事件驱动循环
- [ ] `Frontend` trait 定义
- [ ] 工具注册与执行框架
- [ ] 实现核心工具：`bash`、`read`
- [ ] 会话管理（单会话，带 `SessionId`）

**验证**：命令行 Agent 对话（无 TUI，使用简单的 stdin/stdout 前端）。

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
