# P1 上下文管理修复：摘要压缩 + 精简系统提示词

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** (1) 实现真正的 LLM 摘要压缩，替代占位符通知；(2) 精简 system.md 中冗长的 Memory System 章节。

**Architecture:** 在 `context.rs` 添加 `format_removed_messages_as_transcript()` 将已移除消息转为紧凑文本；在 `Agent` 添加 `generate_summary()` 调用 LLM 生成摘要；在 `run_one_step` 中用真实摘要替换占位符。`system.md` 中 Memory System 从 ~50 行压缩到 ~10 行。

**Tech Stack:** Rust, 无新依赖

## Global Constraints

- 向后兼容：`compression_enabled = false` 时行为不变
- 摘要生成失败时降级为静态通知（不阻断 Agent 循环）
- 现有测试全部通过

---

## 文件结构

| 文件 | 职责 | 变更类型 |
|------|------|----------|
| `crates/robit-agent/src/context.rs` | 新增 `format_removed_messages_as_transcript()` | 修改 |
| `crates/robit-agent/src/agent.rs` | 新增 `generate_summary()` + 集成到 `run_one_step` | 修改 |
| `crates/robit-agent/prompts/system.md` | 精简 Long-Term Memory System 章节 | 修改 |

---

### Task 1: 添加 format_removed_messages_as_transcript

**Files:**
- Modify: `crates/robit-agent/src/context.rs`

**Interfaces:**
- Produces: `pub fn format_removed_messages_as_transcript(messages: &[ChatCompletionRequestMessage]) -> String`

- [ ] **Step 1: 在 context.rs 中添加函数**

在 `is_system_message` 之后、测试模块之前添加：

```rust
/// Format removed messages into a compact transcript for summary generation.
/// Extracts user messages, assistant text, and tool call names only.
/// Each message is truncated to keep the transcript concise.
pub fn format_removed_messages_as_transcript(
    messages: &[ChatCompletionRequestMessage],
) -> String {
    let mut transcript = String::new();

    for msg in messages {
        match msg {
            ChatCompletionRequestMessage::User(user_msg) => {
                let text = match &user_msg.content {
                    async_openai::types::chat::ChatCompletionRequestUserMessageContent::Text(t) => {
                        t.clone()
                    }
                    async_openai::types::chat::ChatCompletionRequestUserMessageContent::Array(parts) => {
                        parts.iter()
                            .filter_map(|p| match p {
                                async_openai::types::chat::ChatCompletionRequestUserMessageContentPart::Text(t) => Some(t.text.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join(" ")
                    }
                };
                let truncated = truncate_str(&text, 200);
                transcript.push_str(&format!("User: {}\n", truncated));
            }
            ChatCompletionRequestMessage::Assistant(assistant_msg) => {
                if let Some(content) = &assistant_msg.content {
                    let truncated = truncate_str(content, 300);
                    transcript.push_str(&format!("Assistant: {}\n", truncated));
                }
                if let Some(tool_calls) = &assistant_msg.tool_calls {
                    for tc in tool_calls {
                        if let async_openai::types::chat::ChatCompletionMessageToolCalls::Function(f) = tc {
                            transcript.push_str(&format!(
                                "  [Tool: {}({})]\n",
                                f.function.name,
                                truncate_str(&f.function.arguments, 100)
                            ));
                        }
                    }
                }
            }
            ChatCompletionRequestMessage::Tool(tool_msg) => {
                let truncated = truncate_str(&tool_msg.content, 150);
                transcript.push_str(&format!("  [Result: {}]\n", truncated));
            }
            _ => {}
        }
    }

    if transcript.is_empty() {
        transcript.push_str("(no conversation content)");
    }

    transcript
}

/// Truncate a string to at most `max_chars` characters, adding "..." if truncated.
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        s.to_string()
    } else {
        let mut end = max_chars;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}
```

- [ ] **Step 2: 验证编译**

```bash
cargo build -p robit-agent
```

- [ ] **Step 3: Commit**

---

### Task 2: 在 Agent 中实现 generate_summary + 集成到 run_one_step

**Files:**
- Modify: `crates/robit-agent/src/agent.rs`

**Interfaces:**
- Consumes: `format_removed_messages_as_transcript` from context.rs
- Produces: `Agent::generate_summary()` private method

- [ ] **Step 1: 添加 generate_summary 方法**

```rust
    /// Generate a summary of removed conversation messages using the LLM.
    /// Falls back to a static notice on failure.
    async fn generate_summary(
        &self,
        removed_messages: &[ChatCompletionRequestMessage],
    ) -> String {
        let transcript = crate::context::format_removed_messages_as_transcript(removed_messages);

        let system_prompt = "Summarize the following conversation transcript in 1-2 concise sentences. Focus on: what the user asked for, what actions were taken, and the outcomes. Be brief and factual.";

        let messages = vec![
            ChatCompletionRequestMessage::System(
                ChatCompletionRequestSystemMessage {
                    content: system_prompt.into(),
                    name: None,
                }
                .into(),
            ),
            ChatCompletionRequestMessage::User(
                ChatCompletionRequestUserMessage {
                    content: format!("Conversation transcript:\n\n{}", transcript).into(),
                    name: None,
                }
                .into(),
            ),
        ];

        match self.llm_client.chat(messages, None).await {
            Ok(response) => {
                if let Some(choice) = response.choices.first() {
                    if let Some(content) = &choice.message.content {
                        let summary = content.trim().to_string();
                        if !summary.is_empty() {
                            tracing::info!("Generated summary: {}", summary);
                            return summary;
                        }
                    }
                }
                tracing::warn!("Summary generation returned empty response, using fallback");
                "Conversation history compressed.".to_string()
            }
            Err(e) => {
                tracing::warn!("Summary generation failed: {}, using fallback", e);
                "Conversation history compressed.".to_string()
            }
        }
    }
```

- [ ] **Step 2: 更新 run_one_step 中的压缩处理**

将：
```rust
        // Handle async compression if needed (logging only;
        // maybe_truncate already inserted the appropriate notice).
        if truncation_result.needs_compression {
            tracing::info!(
                "Compression triggered: removed {} tokens (threshold: {}). Async summary generation not yet implemented.",
                crate::context::estimate_messages_tokens(&truncation_result.removed_messages),
                self.context_manager.compression_token_threshold
            );
        }
```

替换为：
```rust
        // Handle async compression: generate actual summary via LLM
        if truncation_result.needs_compression {
            let summary = self.generate_summary(&truncation_result.removed_messages).await;

            // Replace the placeholder notice with the actual summary
            if let Some(msg) = session.history.get_mut(truncation_result.insert_position) {
                let notice = format!(
                    "[Earlier conversation summary: {}]",
                    summary
                );
                *msg = ChatCompletionRequestMessage::User(
                    ChatCompletionRequestUserMessage {
                        content: notice.into(),
                        name: Some("system_notice".to_string()),
                    }
                    .into(),
                );
            }

            tracing::info!(
                "Compression completed: removed {} tokens, summary inserted",
                crate::context::estimate_messages_tokens(&truncation_result.removed_messages),
            );
        }
```

- [ ] **Step 3: 验证编译**

```bash
cargo build -p robit-agent
```

- [ ] **Step 4: Commit**

---

### Task 3: 精简 system.md 中的 Long-Term Memory System 章节

**Files:**
- Modify: `crates/robit-agent/prompts/system.md`

- [ ] **Step 1: 替换 Memory System 章节**

将原来的 ~50 行 Memory System 章节替换为精简版：

```markdown
## Memory

You have a persistent file-based memory at `{cwd}/.robit/memory/`. Each memory is one file holding one fact.

- **`memory.md`** — persistent memory across sessions. Read at startup, update when you learn important info.
- **`YYYY-MM-DD.md`** — daily memory. Review at end of day, migrate important info to `memory.md`.

**When to write:** user says "remember this", shares preferences, or you discover project conventions.
**When to read:** at startup, and when user references past conversations or decisions.

Keep entries concise. Don't log trivial details. Memory is per working directory.
```

- [ ] **Step 2: 验证编译**

```bash
cargo build -p robit-agent
```

- [ ] **Step 3: Commit**

---

### Task 4: 添加测试 + 端到端验证

- [ ] **Step 1: 添加 format_removed_messages_as_transcript 测试**

- [ ] **Step 2: 运行所有测试**

```bash
cargo test -p robit-agent
```

- [ ] **Step 3: 运行 clippy**

```bash
cargo clippy --all-targets -p robit-agent
```

- [ ] **Step 4: Commit**