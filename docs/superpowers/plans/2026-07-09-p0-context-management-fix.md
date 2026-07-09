# P0 上下文管理修复：Token 估算改进 + 提前截断

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复上下文管理中的两个 P0 问题：(1) token 估算不准确导致 API 端静默截断，(2) 截断触发太晚且没有保留最近轮次的下限保护。

**Architecture:** 分层改进 `ContextManager`——先改进 token 估算算法（引入安全系数），再调整截断触发阈值（从 80% 提前到 70%），最后添加 `min_keep_rounds` 下限保护。所有新参数通过 `ContextConfig` 配置，保持向后兼容。

**Tech Stack:** Rust, `serde` for config, 无新依赖

## Global Constraints

- 向后兼容：所有新增配置项使用 `Option`，未配置时使用默认值
- 现有测试必须全部通过
- 不引入新的外部依赖
- 遵循项目现有的错误处理和日志风格

---

## 文件结构

| 文件 | 职责 | 变更类型 |
|------|------|----------|
| `crates/robit-ai/src/config.rs` | `ContextConfig` 新增 3 个字段 | 修改 |
| `crates/robit-agent/src/context.rs` | 改进 token 估算 + 提前截断 + 保留最近轮次 | 修改 |

---

### Task 1: 扩展 ContextConfig 配置结构

**Files:**
- Modify: `crates/robit-ai/src/config.rs:101-111`

**Interfaces:**
- Produces: `ContextConfig` 新增字段 `truncation_ratio: Option<f32>`, `min_keep_rounds: Option<usize>`, `token_safety_margin: Option<f32>`

- [ ] **Step 1: 添加新字段到 ContextConfig**

```rust
#[derive(Debug, Deserialize)]
pub struct ContextConfig {
    pub max_output_lines: Option<usize>,
    pub max_output_bytes: Option<usize>,
    pub reserve_ratio: Option<f32>,
    /// Token threshold for triggering compression (default 5000).
    /// Only compress when removed messages exceed this token count.
    pub compression_token_threshold: Option<usize>,
    /// Enable/disable context compression (default true).
    pub compression_enabled: Option<bool>,
    /// Fraction of max_tokens at which truncation triggers (default 0.7).
    /// Lower = earlier truncation, more headroom for estimation errors.
    pub truncation_ratio: Option<f32>,
    /// Minimum conversation rounds to keep after truncation (default 3).
    /// Prevents losing all recent context when truncation is aggressive.
    pub min_keep_rounds: Option<usize>,
    /// Safety multiplier applied to token estimates (default 1.3).
    /// Compensates for heuristic underestimation vs actual tokenizer counts.
    pub token_safety_margin: Option<f32>,
}
```

- [ ] **Step 2: 验证编译**

```bash
cargo build -p robit-ai
```

Expected: 编译成功。

- [ ] **Step 3: Commit**

```bash
git add crates/robit-ai/src/config.rs
git commit -m "feat: add truncation_ratio, min_keep_rounds, token_safety_margin to ContextConfig"
```

---

### Task 2: 改进 ContextManager 结构体和构造函数

**Files:**
- Modify: `crates/robit-agent/src/context.rs:137-175`

**Interfaces:**
- Consumes: `ContextConfig` 的新字段
- Produces: `ContextManager` 新增字段 `truncation_ratio`, `min_keep_rounds`, `token_safety_margin`

- [ ] **Step 1: 添加新字段到 ContextManager**

```rust
/// Manages the context window, truncating history when approaching token limits.
pub struct ContextManager {
    /// Model's context window size in tokens.
    pub max_tokens: usize,
    /// Ratio of context window to reserve for LLM response (default 0.2 = 20%).
    pub reserve_ratio: f32,
    /// Fraction of max_tokens at which truncation triggers (default 0.7).
    pub truncation_ratio: f32,
    /// Minimum conversation rounds to keep after truncation (default 3).
    pub min_keep_rounds: usize,
    /// Safety multiplier for token estimates (default 1.3).
    pub token_safety_margin: f32,
    /// Max output lines for tool results.
    pub max_output_lines: usize,
    /// Max output bytes for tool results.
    pub max_output_bytes: usize,
    /// Token threshold for triggering compression.
    pub compression_token_threshold: usize,
    /// Whether compression is enabled.
    pub compression_enabled: bool,
}
```

- [ ] **Step 2: 更新 ContextManager::new 读取新配置**

```rust
impl ContextManager {
    pub fn new(context_window: Option<u64>, config: Option<&ContextConfig>) -> Self {
        let max_tokens = context_window.unwrap_or(65536) as usize;

        let (
            max_output_lines,
            max_output_bytes,
            reserve_ratio,
            truncation_ratio,
            min_keep_rounds,
            token_safety_margin,
            compression_token_threshold,
            compression_enabled,
        ) = match config {
            Some(c) => (
                c.max_output_lines.unwrap_or(500),
                c.max_output_bytes.unwrap_or(51200),
                c.reserve_ratio.unwrap_or(0.2),
                c.truncation_ratio.unwrap_or(0.7),
                c.min_keep_rounds.unwrap_or(3),
                c.token_safety_margin.unwrap_or(1.3),
                c.compression_token_threshold.unwrap_or(5000),
                c.compression_enabled.unwrap_or(true),
            ),
            None => (500, 51200, 0.2, 0.7, 3, 1.3, 5000, true),
        };

        Self {
            max_tokens,
            reserve_ratio,
            truncation_ratio,
            min_keep_rounds,
            token_safety_margin,
            max_output_lines,
            max_output_bytes,
            compression_token_threshold,
            compression_enabled,
        }
    }
```

- [ ] **Step 3: 更新 available_tokens 方法使用 truncation_ratio**

`available_tokens` 改名为更准确的表达，保持旧方法兼容：

```rust
    /// Maximum tokens available before truncation is triggered.
    /// Uses truncation_ratio (default 0.7) rather than reserve_ratio,
    /// so truncation happens earlier to leave headroom for estimation errors.
    pub fn truncation_threshold(&self) -> usize {
        (self.max_tokens as f32 * self.truncation_ratio) as usize
    }

    /// Maximum tokens available for the full context (including response).
    #[deprecated(note = "Use truncation_threshold() instead")]
    pub fn available_tokens(&self) -> usize {
        (self.max_tokens as f32 * (1.0 - self.reserve_ratio)) as usize
    }
```

- [ ] **Step 4: 验证编译**

```bash
cargo build -p robit-agent
```

Expected: 编译成功。

- [ ] **Step 5: Commit**

```bash
git add crates/robit-agent/src/context.rs
git commit -m "feat: add truncation_ratio, min_keep_rounds, token_safety_margin to ContextManager"
```

---

### Task 3: 改进 token 估算算法

**Files:**
- Modify: `crates/robit-agent/src/context.rs:85-130`

**Interfaces:**
- Consumes: `token_safety_margin` from `ContextManager`
- Produces: 更准确的 `estimate_tokens` 和 `estimate_messages_tokens`

- [ ] **Step 1: 重写 estimate_tokens 函数**

将原来粗糙的 `chars/4` + `chars/2` 改为更精确的分类估算：

```rust
/// Estimate token count for a string.
///
/// Uses a more nuanced heuristic based on character type:
/// - ASCII letters/digits: ~3.5 chars/token (BPE tokenizer average)
/// - CJK characters: ~1.5 chars/token (most CJK chars are 1-2 tokens each)
/// - Whitespace: minimal token cost (usually merged with adjacent tokens)
/// - Punctuation/symbols: ~1 token per char (often individual tokens)
/// - Code (braces, operators): ~1 token per char
///
/// This is still an estimate; apply `token_safety_margin` at the message level.
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }

    let mut ascii_alnum = 0usize;
    let mut cjk = 0usize;
    let mut whitespace = 0usize;
    let mut other = 0usize; // punctuation, symbols, code characters

    for ch in text.chars() {
        if ch.is_whitespace() {
            whitespace += 1;
        } else if ch.is_ascii_alphanumeric() {
            ascii_alnum += 1;
        } else {
            let cp = ch as u32;
            // CJK Unified Ideographs + extensions
            if (0x4E00..=0x9FFF).contains(&cp)
                || (0x3400..=0x4DBF).contains(&cp)
                || (0xF900..=0xFAFF).contains(&cp)
                || (0xFF00..=0xFFEF).contains(&cp)
                || (0x3000..=0x303F).contains(&cp) // CJK punctuation
                || (0x3040..=0x309F).contains(&cp) // Hiragana
                || (0x30A0..=0x30FF).contains(&cp) // Katakana
                || (0xAC00..=0xD7AF).contains(&cp) // Hangul
            {
                cjk += 1;
            } else {
                other += 1;
            }
        }
    }

    // BPE tokenizer averages:
    // - ASCII alphanumeric: ~3.5 chars per token
    // - CJK: ~1.5 chars per token (most are 1 token each, some pairs)
    // - Whitespace: negligible (merged with adjacent tokens)
    // - Other (punctuation/code): ~1 char per token
    let ascii_tokens = (ascii_alnum as f64 / 3.5).ceil() as usize;
    let cjk_tokens = (cjk as f64 / 1.5).ceil() as usize;
    let whitespace_tokens = (whitespace as f64 / 10.0).ceil() as usize;
    let other_tokens = other; // ~1:1

    ascii_tokens + cjk_tokens + whitespace_tokens + other_tokens
}
```

- [ ] **Step 2: 更新 estimate_messages_tokens 加安全系数**

```rust
/// Estimate tokens for a list of messages with safety margin applied.
pub fn estimate_messages_tokens(messages: &[ChatCompletionRequestMessage]) -> usize {
    let mut total = 0;
    for msg in messages {
        // Each message has overhead: role marker (~3 tokens), formatting (~1 token)
        total += 4;
        total += estimate_message_content_tokens(msg);
    }
    total
}

/// Estimate tokens for messages, applying the configured safety margin.
pub fn estimate_messages_tokens_with_margin(
    messages: &[ChatCompletionRequestMessage],
    safety_margin: f32,
) -> usize {
    let raw = estimate_messages_tokens(messages);
    (raw as f32 * safety_margin).ceil() as usize
}
```

- [ ] **Step 3: 验证编译**

```bash
cargo build -p robit-agent
```

Expected: 编译成功。

- [ ] **Step 4: Commit**

```bash
git add crates/robit-agent/src/context.rs
git commit -m "feat: improve token estimation with per-character-type heuristics"
```

---

### Task 4: 更新 maybe_truncate —— 提前触发 + 保留最近轮次

**Files:**
- Modify: `crates/robit-agent/src/context.rs:192-325`

**Interfaces:**
- Consumes: `truncation_threshold()`, `min_keep_rounds`, `token_safety_margin`
- Produces: 更新后的 `maybe_truncate` 行为

- [ ] **Step 1: 重写 maybe_truncate 方法**

```rust
    /// Check if history needs truncation and perform it if necessary.
    /// Returns `TruncationResult` with removed messages for async compression.
    ///
    /// Strategy:
    /// 1. Uses `truncation_threshold()` (default 70% of max_tokens) as trigger point
    /// 2. Removes oldest non-system rounds first
    /// 3. Always keeps at least `min_keep_rounds` recent rounds
    /// 4. Applies `token_safety_margin` to estimates to avoid underestimation
    pub fn maybe_truncate(
        &self,
        messages: &mut Vec<ChatCompletionRequestMessage>,
    ) -> TruncationResult {
        let estimated = estimate_messages_tokens_with_margin(messages, self.token_safety_margin);
        let threshold = self.truncation_threshold();

        if estimated <= threshold {
            return TruncationResult {
                rounds_removed: 0,
                messages_removed: 0,
                removed_messages: Vec::new(),
                insert_position: 0,
                needs_compression: false,
            };
        }

        tracing::info!(
            "Context truncation needed: estimated {} tokens (with {:.1}x margin), threshold {} tokens, max {} tokens",
            estimated,
            self.token_safety_margin,
            threshold,
            self.max_tokens
        );

        // Find round boundaries: a round starts with a User message
        let mut round_starts: Vec<usize> = Vec::new();
        for (i, msg) in messages.iter().enumerate() {
            if is_user_message(msg) {
                round_starts.push(i);
            }
        }

        if round_starts.is_empty() {
            return TruncationResult {
                rounds_removed: 0,
                messages_removed: 0,
                removed_messages: Vec::new(),
                insert_position: 0,
                needs_compression: false,
            };
        }

        // Count how many rounds we must keep
        let total_rounds = round_starts.len();
        let must_keep = self.min_keep_rounds.min(total_rounds);

        let mut removed_messages: Vec<ChatCompletionRequestMessage> = Vec::new();
        let mut rounds_removed = 0;
        let mut messages_removed = 0;

        // Remove oldest rounds while:
        // - estimated tokens still exceed threshold
        // - we still have more rounds than must_keep
        while round_starts.len() > must_keep
            && estimate_messages_tokens_with_margin(messages, self.token_safety_margin) > threshold
        {
            let start_idx = round_starts[0];
            let end_idx = if round_starts.len() > 1 {
                round_starts[1]
            } else {
                messages.len()
            };

            // Collect removed messages for potential compression
            if self.compression_enabled {
                removed_messages.extend(messages[start_idx..end_idx].to_vec());
            }

            let count = end_idx - start_idx;
            messages.drain(start_idx..end_idx);

            // Update round_starts indices
            round_starts.remove(0);
            for idx in round_starts.iter_mut() {
                *idx = idx.saturating_sub(count);
            }

            rounds_removed += 1;
            messages_removed += count;
        }

        if rounds_removed == 0 {
            return TruncationResult {
                rounds_removed: 0,
                messages_removed: 0,
                removed_messages: Vec::new(),
                insert_position: 0,
                needs_compression: false,
            };
        }

        // Calculate removed tokens for threshold check
        let removed_tokens = estimate_messages_tokens(&removed_messages);
        let needs_compression =
            self.compression_enabled && removed_tokens >= self.compression_token_threshold;

        // Insert informative notice after system messages
        let system_msg_count = messages
            .iter()
            .take_while(|m| is_system_message(m))
            .count();

        let notice = if needs_compression {
            format!(
                "[Context compressed: {} earlier rounds ({} messages, ~{} tokens) have been summarized. {} most recent rounds preserved.]",
                rounds_removed,
                messages_removed,
                removed_tokens,
                round_starts.len()
            )
        } else {
            format!(
                "[Context truncated: {} earlier rounds ({} messages) removed to stay within token limit. {} most recent rounds preserved.]",
                rounds_removed,
                messages_removed,
                round_starts.len()
            )
        };

        let notice_msg = ChatCompletionRequestMessage::User(
            async_openai::types::chat::ChatCompletionRequestUserMessage {
                content: notice.into(),
                name: Some("system_notice".to_string()),
            }
            .into(),
        );

        messages.insert(system_msg_count, notice_msg);

        tracing::info!(
            "Context truncation: removed {} rounds ({} messages, ~{} tokens), kept {} rounds, needs_compression={}",
            rounds_removed,
            messages_removed,
            removed_tokens,
            round_starts.len(),
            needs_compression
        );

        TruncationResult {
            rounds_removed,
            messages_removed,
            removed_messages,
            insert_position: system_msg_count,
            needs_compression,
        }
    }
```

- [ ] **Step 2: 验证编译**

```bash
cargo build -p robit-agent
```

Expected: 编译成功。

- [ ] **Step 3: Commit**

```bash
git add crates/robit-agent/src/context.rs
git commit -m "feat: early truncation trigger + min_keep_rounds floor protection"
```

---

### Task 5: 更新现有测试 + 添加新测试

**Files:**
- Modify: `crates/robit-agent/src/context.rs:340-471`

**Interfaces:**
- Consumes: 更新后的 `ContextManager` 和估算函数
- Produces: 覆盖新行为的测试

- [ ] **Step 0: 修复现有测试的 ContextConfig 构造（编译必须）**

三个旧测试直接构造 `ContextConfig` 结构体，需要补上新增的 3 个字段：

```rust
    #[test]
    fn test_truncation_result_no_compression() {
        let mut messages = vec![
            make_system_message("You are a helpful assistant"),
            make_user_message("Hello"),
        ];

        let config = ContextConfig {
            max_output_lines: Some(500),
            max_output_bytes: Some(51200),
            reserve_ratio: Some(0.2),
            truncation_ratio: None,
            min_keep_rounds: None,
            token_safety_margin: None,
            compression_token_threshold: Some(5000),
            compression_enabled: Some(true),
        };

        let manager = ContextManager::new(Some(65536), Some(&config));
        let result = manager.maybe_truncate(&mut messages);

        assert_eq!(result.rounds_removed, 0);
        assert!(!result.needs_compression);
    }

    #[test]
    fn test_truncation_result_with_compression() {
        let mut messages = vec![
            make_system_message("You are a helpful assistant"),
        ];

        for i in 0..20 {
            let content = format!("User message {}: {}", i, "x".repeat(2000));
            messages.push(make_user_message(&content));
        }

        let config = ContextConfig {
            max_output_lines: Some(500),
            max_output_bytes: Some(51200),
            reserve_ratio: Some(0.2),
            truncation_ratio: None,
            min_keep_rounds: None,
            token_safety_margin: None,
            compression_token_threshold: Some(1000),
            compression_enabled: Some(true),
        };

        let manager = ContextManager::new(Some(5000), Some(&config));
        let result = manager.maybe_truncate(&mut messages);

        assert!(result.rounds_removed > 0);
        assert!(result.needs_compression);
        assert!(!result.removed_messages.is_empty());
    }

    #[test]
    fn test_compression_disabled() {
        let mut messages = vec![
            make_system_message("You are a helpful assistant"),
        ];

        for i in 0..20 {
            let content = format!("User message {}: {}", i, "x".repeat(2000));
            messages.push(make_user_message(&content));
        }

        let config = ContextConfig {
            max_output_lines: Some(500),
            max_output_bytes: Some(51200),
            reserve_ratio: Some(0.2),
            truncation_ratio: None,
            min_keep_rounds: None,
            token_safety_margin: None,
            compression_token_threshold: Some(1000),
            compression_enabled: Some(false),
        };

        let manager = ContextManager::new(Some(5000), Some(&config));
        let result = manager.maybe_truncate(&mut messages);

        assert!(result.rounds_removed > 0);
        assert!(!result.needs_compression);
    }
```

- [ ] **Step 1: 更新 estimate_tokens 测试**

```rust
    #[test]
    fn test_estimate_tokens_english() {
        let text = "Hello world, this is a test of the token estimation system.";
        let tokens = estimate_tokens(text);
        // ~14 words, ~60 chars ASCII, ~3.5 chars/token = ~17 tokens
        assert!(tokens >= 10, "Expected at least 10 tokens, got {}", tokens);
        assert!(tokens <= 30, "Expected at most 30 tokens, got {}", tokens);
    }

    #[test]
    fn test_estimate_tokens_chinese() {
        let chinese = "你好世界，这是一个测试。";
        let tokens = estimate_tokens(chinese);
        // 12 CJK chars / 1.5 = ~8 tokens
        assert!(tokens >= 5, "Expected at least 5 tokens, got {}", tokens);
        assert!(tokens <= 15, "Expected at most 15 tokens, got {}", tokens);
    }

    #[test]
    fn test_estimate_tokens_code() {
        let code = "fn main() {\n    println!(\"Hello\");\n}";
        let tokens = estimate_tokens(code);
        // Code has lots of punctuation/symbols which are ~1:1
        assert!(tokens >= 10, "Expected at least 10 tokens, got {}", tokens);
        assert!(tokens <= 40, "Expected at most 40 tokens, got {}", tokens);
    }

    #[test]
    fn test_estimate_tokens_empty() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn test_estimate_tokens_mixed() {
        let mixed = "Hello 你好 world 世界！fn test() {}";
        let tokens = estimate_tokens(mixed);
        assert!(tokens > 0);
        // Should be higher than pure English but lower than pure code per char
        assert!(tokens <= 40, "Expected at most 40 tokens, got {}", tokens);
    }
```

- [ ] **Step 2: 更新 ContextManager 测试使用新字段**

```rust
    fn make_test_config() -> ContextConfig {
        ContextConfig {
            max_output_lines: Some(500),
            max_output_bytes: Some(51200),
            reserve_ratio: Some(0.2),
            truncation_ratio: Some(0.7),
            min_keep_rounds: Some(3),
            token_safety_margin: Some(1.3),
            compression_token_threshold: Some(5000),
            compression_enabled: Some(true),
        }
    }

    #[test]
    fn test_truncation_threshold() {
        let config = make_test_config();
        let manager = ContextManager::new(Some(65536), Some(&config));
        // 65536 * 0.7 = 45875
        assert_eq!(manager.truncation_threshold(), 45875);
    }

    #[test]
    fn test_truncation_result_no_truncation() {
        let mut messages = vec![
            make_system_message("You are a helpful assistant"),
            make_user_message("Hello"),
        ];

        let config = make_test_config();
        let manager = ContextManager::new(Some(65536), Some(&config));
        let result = manager.maybe_truncate(&mut messages);

        assert_eq!(result.rounds_removed, 0);
        assert!(!result.needs_compression);
    }

    #[test]
    fn test_truncation_respects_min_keep_rounds() {
        let mut messages = vec![
            make_system_message("You are a helpful assistant"),
        ];

        // Add 10 rounds of large messages
        for i in 0..10 {
            let content = format!("User message {}: {}", i, "x".repeat(2000));
            messages.push(make_user_message(&content));
        }

        let mut config = make_test_config();
        config.min_keep_rounds = Some(3); // Must keep at least 3 rounds

        // Use small context window to force aggressive truncation
        let manager = ContextManager::new(Some(5000), Some(&config));
        let result = manager.maybe_truncate(&mut messages);

        // Should have removed some rounds...
        assert!(result.rounds_removed > 0, "Should have removed some rounds");
        // ...but should still have at least 3 user messages (min_keep_rounds)
        let user_count = messages.iter().filter(|m| matches!(m, ChatCompletionRequestMessage::User(_))).count();
        // The notice is also a User message, so we need at least 3 + 1 notice = 4
        assert!(user_count >= 4, "Should have at least 3 user rounds + notice, got {}", user_count);
    }

    #[test]
    fn test_truncation_early_trigger() {
        let mut messages = vec![
            make_system_message("You are a helpful assistant"),
        ];

        // Add 8 rounds of messages, each ~2000 chars
        for i in 0..8 {
            let content = format!("User message {}: {}", i, "x".repeat(2000));
            messages.push(make_user_message(&content));
        }

        let mut config = make_test_config();
        config.truncation_ratio = Some(0.7); // Trigger at 70%
        config.min_keep_rounds = Some(2);
        config.token_safety_margin = Some(1.3);

        // With 65536 context, truncation threshold = 45875
        // 8 rounds * ~2000 chars each ≈ much less than 45875, so no truncation
        let manager = ContextManager::new(Some(65536), Some(&config));
        let result = manager.maybe_truncate(&mut messages);
        assert_eq!(result.rounds_removed, 0, "Should not truncate small messages in large window");

        // With 8000 context, truncation threshold = 5600
        // 8 rounds * ~2000 chars ≈ 16000 chars, estimated tokens with margin > 5600
        let manager2 = ContextManager::new(Some(8000), Some(&config));
        let mut messages2 = messages.clone();
        let result2 = manager2.maybe_truncate(&mut messages2);
        assert!(result2.rounds_removed > 0, "Should truncate when exceeding small window");
    }

    #[test]
    fn test_token_safety_margin_effect() {
        let mut messages = vec![
            make_system_message("You are a helpful assistant"),
        ];

        for i in 0..10 {
            let content = format!("User message {}: {}", i, "x".repeat(500));
            messages.push(make_user_message(&content));
        }

        // With margin 1.0 (no safety), truncation may not trigger
        let mut config_low = make_test_config();
        config_low.token_safety_margin = Some(1.0);
        config_low.truncation_ratio = Some(0.7);
        config_low.min_keep_rounds = Some(1);

        let mut msgs_low = messages.clone();
        let manager_low = ContextManager::new(Some(8000), Some(&config_low));
        let result_low = manager_low.maybe_truncate(&mut msgs_low);

        // With margin 2.0 (very conservative), truncation more likely triggers
        let mut config_high = make_test_config();
        config_high.token_safety_margin = Some(2.0);
        config_high.truncation_ratio = Some(0.7);
        config_high.min_keep_rounds = Some(1);

        let mut msgs_high = messages.clone();
        let manager_high = ContextManager::new(Some(8000), Some(&config_high));
        let result_high = manager_high.maybe_truncate(&mut msgs_high);

        // Higher margin should result in >= rounds removed
        assert!(
            result_high.rounds_removed >= result_low.rounds_removed,
            "Higher safety margin should trigger at least as much truncation: high={}, low={}",
            result_high.rounds_removed,
            result_low.rounds_removed
        );
    }
```

- [ ] **Step 3: 运行所有测试**

```bash
cargo test -p robit-agent
```

Expected: 所有测试通过。

- [ ] **Step 4: Commit**

```bash
git add crates/robit-agent/src/context.rs
git commit -m "test: update context tests for new estimation and truncation behavior"
```

---

### Task 6: 端到端验证

**Files:**
- 无新建文件，运行现有测试套件

- [ ] **Step 1: 运行完整测试套件**

```bash
cargo test
```

Expected: 所有测试通过。

- [ ] **Step 2: 运行 clippy 检查**

```bash
cargo clippy --all-targets
```

Expected: 无新增警告。

- [ ] **Step 3: 手动验证编译所有 crate**

```bash
cargo build --workspace
```

Expected: 全部编译成功。

- [ ] **Step 4: Commit**

```bash
git commit --allow-empty -m "chore: final verification after P0 context management fixes"
```