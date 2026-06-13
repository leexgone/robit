# 构建路线图

## 阶段 1：LLM API 层（`robit-ai`） ✅ 已完成

**目标**：能够与 LLM 完成一次流式对话。

- [x] 定义 `Message`、`ToolCall` 等核心数据结构（使用 `async-openai` 类型）
- [x] 实现 OpenAI 兼容协议的统一 HTTP 客户端（`LlmClient` 封装）
- [x] 适配提供商（QWen、DeepSeek 等 OpenAI 兼容协议）
- [x] 流式响应（SSE）解析与回调（`chat_stream` 方法）
- [x] 配置加载（统一 `robit.toml`，支持 `${ENV_VAR}` 替换）
- [x] 配置格式重构（`providers` + `models` 嵌套结构，`default_model` 使用 `provider/model` 格式）

**验证**：`cargo run -p robit-chat`（`examples/robit-chat`）启动 REPL 交互式对话测试。✅ 已通过

## 阶段 2：Agent 运行时（`robit-agent`） ✅ 已完成

**目标**：Agent 能够调用工具完成一个简单编程任务。

- [x] Agent 事件驱动循环（`agent.rs` — 流式 LLM 调用 + tool call 组装 + 多轮循环）
- [x] `Frontend` trait 定义（`frontend.rs` — `on_event` + `request_tool_confirmation`）
- [x] 工具注册与执行框架（`tool/mod.rs` — `Tool` trait + `ToolRegistry` + `ToolContext`）
- [x] 实现核心工具：`read`（带行号 + 截断）、`bash`（跨平台 shell）、`edit`、`write`、`load_skill`
- [x] 会话管理（单会话 + `SessionId`，上下文截断两层策略）
- [x] 提示词系统（`prompt.rs` — 动态组装系统提示词）
- [x] 上下文管理（`context.rs` — 输出截断 + 历史按轮次截断 + token 估算 + 压缩策略）
- [x] 技能系统（目录式技能，`SKILL.md` frontmatter，系统提示词注入，斜杠命令触发）

**验证**：`cargo run -p robit-agent-cli` 启动命令行 Agent，使用 `read`/`bash` 工具完成任务。

## 阶段 3：TUI 前端（`robit`） ✅ 已完成

**目标**：完整的终端交互体验，可用于日常编程。

- [x] 实现 `Frontend` trait 的 TUI 前端（`TuiFrontend` — channel-based，事件循环驱动）
- [x] 流式文本显示（Markdown 渲染 — `pulldown-cmark` 解析，代码块边框 + 粗体/斜体）
- [x] 工具调用状态展示与用户确认交互（工具卡片 + Y/N 确认弹窗）
- [x] 对话历史管理（`Vec<ConversationEntry>` 模型 + 滚动 + 自动滚到底部）
- [x] 跨平台终端适配（`ratatui` + `crossterm` — Windows / Linux / macOS）
- [x] 斜杠命令（`/exit`、`/clear`、`/model`、`/tools`、`/skills`、`/scroll`）
- [x] 输入编辑器（历史记录、多行切换、光标移动）
- [x] 鼠标支持（滚轮滚动 + 点击选择）

**验证**：`cargo run -p robit` 启动 TUI，用 `robit` 命令进行对话。

## 阶段 4：扩展

**目标**：扩展工具、技能和多平台接入。

### 工具补齐

- [x] `grep` — 搜索文件内容
- [x] `find` — 按模式查找文件
- [x] `ls` — 列出目录内容

### 上下文管理增强

- [x] 基础压缩策略（token 阈值触发 + 截断提示）
- [ ] 摘要压缩（调用 LLM 生成对话摘要，替换占位符）
- [ ] 渐进式压缩（近期保留原文 → 中期摘要 → 远期结构化提取）

### LLM 提供商扩展

- [x] OpenAI 兼容协议适配（DeepSeek、QWen、Moonshot 等）
- [ ] 非 OpenAI 协议适配（Anthropic Claude、Google Gemini）

### 体验优化

- [ ] 代码高亮（引入 `syntect`）
- [ ] Token 精确计数（引入 `tiktoken-rs`）
- [ ] `/model` 指令完善（支持 `provider/model` 和 `model_id` 切换）
- [ ] 对话历史持久化（保存到 `~/.robit/sessions/`）
- [ ] 多会话管理完善

### 多平台接入

- [ ] 飞书前端（`robit-feishu`）
- [ ] QQ 前端（`robit-qq`）

### 基础设施

- [ ] Workspace 依赖统一管理（已完成第三方包统一管理）
- [ ] CI/CD 流水线（自动测试 + 发布）
- [ ] 文档完善（API 文档 + 用户指南）
