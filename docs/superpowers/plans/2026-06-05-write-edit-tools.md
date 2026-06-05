# Write & Edit Tools Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `write` and `edit` tools to `robit-agent`, enabling the Agent to create, overwrite, and precisely modify files.

**Architecture:** Two new tool modules (`write.rs`, `edit.rs`) implementing the `Tool` trait. Shared `resolve_path` extracted to `mod.rs`. Tools registered in `robit-tui/src/main.rs`. No changes to the `robit-agent` lib API.

**Tech Stack:** Rust, `tokio::fs`, `async-trait`, `serde_json`

---

## File Map

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `crates/robit-agent/src/tool/mod.rs` | Add `pub mod write; pub mod edit;`; extract `resolve_path` as shared pub fn |
| Create | `crates/robit-agent/src/tool/write.rs` | `WriteTool` — create/overwrite files |
| Create | `crates/robit-agent/src/tool/edit.rs` | `EditTool` — exact string replacement with fuzzy diagnostics |
| Modify | `crates/robit-tui/src/main.rs` | Register `WriteTool` and `EditTool` in `create_tools()`; fix hardcoded tool count |
| Modify | `CLAUDE.md` | Update tool system table to mark `write` and `edit` as completed |

---

### Task 1: Extract `resolve_path` to `mod.rs` and add module declarations

**Files:**
- Modify: `crates/robit-agent/src/tool/mod.rs`

- [ ] **Step 1: Add module declarations and shared `resolve_path` function**

Add these lines after `pub mod read;` and before the `Tool` trait definition in `mod.rs`:

```rust
pub mod bash;
pub mod read;
pub mod write;   // new
pub mod edit;    // new
```

Add this shared helper function after the `ToolResult` impl block (around line 67):

```rust
/// Resolve a file path relative to the working directory.
pub fn resolve_path(file_path: &str, working_dir: &std::path::PathBuf) -> std::path::PathBuf {
    let p = std::path::PathBuf::from(file_path);
    if p.is_absolute() {
        p
    } else {
        working_dir.join(p)
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p robit-agent 2>&1 | head -20`
Expected: Errors for missing `write` and `edit` modules (expected — we'll create them in subsequent tasks).

- [ ] **Step 3: Update `read.rs` to use the shared function**

In `crates/robit-agent/src/tool/read.rs`, remove the local `resolve_path` function (lines 165-172) and update its call site on line 78 from:

```rust
let path = resolve_path(&parsed.file_path, &ctx.working_dir);
```

to:

```rust
let path = crate::tool::resolve_path(&parsed.file_path, &ctx.working_dir);
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p robit-agent 2>&1 | head -20`
Expected: Errors for missing `write` and `edit` modules only.

- [ ] **Step 5: Commit**

```bash
git add crates/robit-agent/src/tool/mod.rs crates/robit-agent/src/tool/read.rs
git commit -m "refactor(tool): extract resolve_path to shared module"
```

---

### Task 2: Implement `write` tool

**Files:**
- Create: `crates/robit-agent/src/tool/write.rs`

- [ ] **Step 1: Write the tool**

Create `crates/robit-agent/src/tool/write.rs`:

```rust
//! `write` tool — creates or overwrites files.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use super::{resolve_path, Tool, ToolContext, ToolResult};
use crate::error::Result;

#[derive(Debug, Deserialize)]
struct WriteArgs {
    file_path: String,
    content: String,
}

pub struct WriteTool;

impl WriteTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "创建或覆盖文件。自动创建父目录。如果文件已存在则覆盖。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "目标文件路径（相对或绝对路径）"
                },
                "content": {
                    "type": "string",
                    "description": "写入的文件内容"
                }
            },
            "required": ["file_path", "content"]
        })
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let parsed: WriteArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("参数解析失败: {}", e))),
        };

        // Validate file_path
        if parsed.file_path.trim().is_empty() {
            return Ok(ToolResult::error("文件路径不能为空".to_string()));
        }

        let path = resolve_path(&parsed.file_path, &ctx.working_dir);

        // Check if path is an existing directory
        if path.is_dir() {
            return Ok(ToolResult::error(format!(
                "路径是目录: {}",
                path.display()
            )));
        }

        // Check if file exists (for message differentiation)
        let existed = path.exists();

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    return Ok(ToolResult::error(format!(
                        "无法创建目录 '{}': {}",
                        parent.display(),
                        e
                    )));
                }
            }
        }

        // Write file
        let content_bytes = parsed.content.as_bytes();
        match tokio::fs::write(&path, &parsed.content).await {
            Ok(()) => {
                let msg = if existed {
                    format!(
                        "已覆盖文件: {} ({} bytes)",
                        path.display(),
                        content_bytes.len()
                    )
                } else {
                    format!(
                        "已创建文件: {} ({} bytes)",
                        path.display(),
                        content_bytes.len()
                    )
                };
                Ok(ToolResult::success(msg))
            }
            Err(e) => Ok(ToolResult::error(format!(
                "无法写入文件 '{}': {}",
                path.display(),
                e
            ))),
        }
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p robit-agent 2>&1 | head -20`
Expected: Errors for missing `edit` module only.

- [ ] **Step 3: Commit**

```bash
git add crates/robit-agent/src/tool/write.rs
git commit -m "feat(tool): add write tool for file creation and overwrite"
```

---

### Task 3: Implement `edit` tool

**Files:**
- Create: `crates/robit-agent/src/tool/edit.rs`

- [ ] **Step 1: Write the tool**

Create `crates/robit-agent/src/tool/edit.rs`:

```rust
//! `edit` tool — exact string replacement in files.
//!
//! Matches old_string exactly in the target file and replaces with new_string.
//! Requires unique match; returns diagnostic info on zero or multiple matches.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use super::{resolve_path, Tool, ToolContext, ToolResult};
use crate::error::Result;

/// Maximum similar matches to show when old_string is not found.
const MAX_SIMILAR_MATCHES: usize = 3;

#[derive(Debug, Deserialize)]
struct EditArgs {
    file_path: String,
    old_string: String,
    new_string: String,
}

pub struct EditTool;

impl EditTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "精确替换文件中的文本。old_string 必须在文件中唯一匹配。匹配失败时返回相似片段辅助修正。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "目标文件路径（相对或绝对路径）"
                },
                "old_string": {
                    "type": "string",
                    "description": "要替换的原始文本（必须在文件中唯一存在）"
                },
                "new_string": {
                    "type": "string",
                    "description": "替换后的新文本"
                }
            },
            "required": ["file_path", "old_string", "new_string"]
        })
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let parsed: EditArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("参数解析失败: {}", e))),
        };

        let path = resolve_path(&parsed.file_path, &ctx.working_dir);

        if !path.exists() {
            return Ok(ToolResult::error(format!("文件不存在: {}", path.display())));
        }

        if path.is_dir() {
            return Ok(ToolResult::error(format!(
                "'{}' 是一个目录，不是文件",
                path.display()
            )));
        }

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult::error(format!(
                    "无法读取文件 '{}': {}",
                    path.display(),
                    e
                )));
            }
        };

        // Find all match positions
        let matches: Vec<usize> = content
            .match_indices(&parsed.old_string)
            .map(|(pos, _)| pos)
            .collect();

        match matches.len() {
            0 => {
                // No exact match — find similar lines
                let similar = find_similar_matches(&content, &parsed.old_string);
                let mut msg = format!(
                    "在文件中未找到完全匹配的 old_string。\n\
                     以下是最相似的 {} 个匹配片段，请检查是否选择错误：\n\n",
                    similar.len()
                );
                for (i, m) in similar.iter().enumerate() {
                    msg.push_str(&format!(
                        "相似度 {} (第 {} 行):\n  期望: {}\n  实际: {}\n",
                        i + 1,
                        m.line_number,
                        truncate(&parsed.old_string, 120),
                        truncate(&m.actual, 120)
                    ));
                    if i + 1 < similar.len() {
                        msg.push('\n');
                    }
                }
                Ok(ToolResult::error(msg))
            }
            1 => {
                // Unique match — perform replacement
                let new_content = content.replacen(&parsed.old_string, &parsed.new_string, 1);

                match tokio::fs::write(&path, &new_content).await {
                    Ok(()) => {
                        let line_num = count_lines_before(&content, matches[0]);
                        Ok(ToolResult::success(format!(
                            "已修改文件: {} (第 {} 行)",
                            path.display(),
                            line_num
                        )))
                    }
                    Err(e) => Ok(ToolResult::error(format!(
                        "无法写入文件 '{}': {}",
                        path.display(),
                        e
                    ))),
                }
            }
            n => {
                // Multiple matches — show all positions
                let lines: Vec<&str> = content.lines().collect();
                let line_positions: Vec<usize> = matches
                    .iter()
                    .map(|&pos| count_lines_before(&content, pos))
                    .collect();

                let mut msg = format!(
                    "old_string 在文件中出现 {} 次",
                    n
                );
                let lines_str: Vec<String> = line_positions.iter().map(|l| l.to_string()).collect();
                msg.push_str(&format!("（第 {} 行）", lines_str.join("、")));
                msg.push_str("，无法唯一确定替换位置。\n请提供更多上下文使 old_string 唯一。\n\n");

                // Show context for each match (up to first 5)
                let show_count = n.min(5);
                for i in 0..show_count {
                    let line = line_positions[i];
                    msg.push_str("---\n");
                    msg.push_str(&format!("第 {} 行:\n", line));
                    // Show 3 lines of context around the match
                    let start = line.saturating_sub(1);
                    let end = (line + 2).min(lines.len());
                    for j in start..end {
                        if j + 1 == line {
                            msg.push_str(&format!("> {}\n", lines[j]));
                        } else {
                            msg.push_str(&format!("  {}\n", lines[j]));
                        }
                    }
                    msg.push_str("---\n");
                }
                if n > 5 {
                    msg.push_str(&format!("... 还有 {} 处匹配，未显示\n", n - 5));
                }

                Ok(ToolResult::error(msg))
            }
        }
    }
}

/// Match info for similar-match results.
struct SimilarMatch {
    line_number: usize,
    actual: String,
    score: usize, // number of matching characters (higher = better)
}

/// Find lines most similar to old_string using character overlap scoring.
fn find_similar_matches(content: &str, target: &str) -> Vec<SimilarMatch> {
    let lines: Vec<&str> = content.lines().collect();
    let target_lower = target.to_lowercase();
    let target_words: Vec<&str> = target_lower.split_whitespace().collect();

    let mut scored: Vec<(usize, usize, String)> = lines
        .iter()
        .enumerate()
        .map(|(idx, &line)| {
            let line_lower = line.to_lowercase();
            let score = word_overlap_score(&target_words, &line_lower);
            (idx + 1, score, line.trim().to_string())
        })
        .filter(|(_, score, _)| *score > 0)
        .collect();

    // Sort by score descending
    scored.sort_by(|a, b| b.1.cmp(&a.1));

    // Take top MAX_SIMILAR_MATCHES
    scored
        .into_iter()
        .take(MAX_SIMILAR_MATCHES)
        .map(|(line_number, score, actual)| SimilarMatch {
            line_number,
            actual,
            score,
        })
        .collect()
}

/// Count how many words from target appear in the line.
fn word_overlap_score(target_words: &[&str], line: &str) -> usize {
    let line_words: Vec<&str> = line.split_whitespace().collect();
    let mut score = 0;
    for &tw in target_words {
        if line_words.iter().any(|lw| lw.contains(tw) || tw.contains(lw)) {
            score += 1;
        }
    }
    score
}

/// Count 1-based line number for a given byte position in content.
fn count_lines_before(content: &str, byte_pos: usize) -> usize {
    content[..byte_pos].matches('\n').count() + 1
}

/// Truncate text to max_len characters.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len).collect();
        format!("{}...", truncated)
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p robit-agent 2>&1 | head -20`
Expected: No errors.

- [ ] **Step 3: Commit**

```bash
git add crates/robit-agent/src/tool/edit.rs
git commit -m "feat(tool): add edit tool with exact-match replacement and fuzzy diagnostics"
```

---

### Task 4: Register tools in TUI frontend

**Files:**
- Modify: `crates/robit-tui/src/main.rs`

- [ ] **Step 1: Add imports for the new tools**

In `crates/robit-tui/src/main.rs`, add after line 25 (after `use robit_agent::tool::read::ReadTool;`):

```rust
use robit_agent::tool::write::WriteTool;
use robit_agent::tool::edit::EditTool;
```

- [ ] **Step 2: Register tools in `create_tools()`**

Modify the `create_tools` function (around line 124-134) to:

```rust
fn create_tools(config: &robit_ai::config::RobitConfig) -> ToolRegistry {
    let mut tools = ToolRegistry::new();
    let context_config = config.app.as_ref().and_then(|a| a.context.as_ref());
    let max_lines = context_config.and_then(|c| c.max_output_lines).unwrap_or(500);
    let max_bytes = context_config
        .and_then(|c| c.max_output_bytes)
        .unwrap_or(51200);
    tools.register(ReadTool::new(max_lines, max_bytes));
    tools.register(BashTool::new(max_bytes));
    tools.register(WriteTool::new());
    tools.register(EditTool::new());
    tools
}
```

- [ ] **Step 3: Fix the hardcoded tool count**

Replace lines 102-103 (the hardcoded `app.status.tools_enabled = 2`):

```rust
        app.status.tools_enabled = 2; // read + bash
        app.status.tools_total = 2;
```

with:

```rust
        app.status.tools_enabled = tools.tool_names().len();
        app.status.tools_total = tools.tool_names().len();
```

And change line 100 to use the `tools` variable:

```rust
        let mut app = App::new(model, &tools);
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p robit-tui 2>&1 | head -20`
Expected: No errors.

- [ ] **Step 5: Commit**

```bash
git add crates/robit-tui/src/main.rs
git commit -m "feat(robit-tui): register write and edit tools, fix hardcoded tool count"
```

---

### Task 5: Update CLAUDE.md tool table

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update the tool system table**

In `CLAUDE.md`, find the tool system table and update `write` and `edit` rows:

```markdown
| `read` | 读取文件内容，支持图片 | 是 | 否 |
| `bash` | 执行 Shell 命令，流式输出 | 是 | 是 |
| `edit` | 精确查找替换，支持多处并行编辑 | 是 | 是 |
| `write` | 创建/覆盖文件，自动创建父目录 | 是 | 是 |
| `grep` | 搜索文件内容 | 否 | 否 |
| `find` | 按模式查找文件 | 否 | 否 |
| `ls` | 列出目录内容 | 否 | 否 |
```

(Previously `edit` and `write` were in the "计划" section without proper entries — now they have real entries.)

- [ ] **Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs(CLAUDE.md): update tool table — write and edit implemented"
```

---

### Task 6: Build verification and manual test

**Files:** N/A

- [ ] **Step 1: Full workspace build**

Run: `cargo build --workspace 2>&1`
Expected: No errors, no warnings.

- [ ] **Step 2: Verify tool count in TUI**

Run: `cargo run -p robit-tui` and check status bar shows 4 tools.
Then type `/tools` in the TUI — should show "已启用工具: 4 个".

- [ ] **Step 3: Quick smoke test**

In the TUI, send: "请用 write 工具创建一个文件 test_write.txt，内容为 Hello World"
Then: "请用 edit 工具把 test_write.txt 中的 World 改为 Rust"
Then: "请读取 test_write.txt 的内容"

Expected: Agent creates file → edits it → reads back "Hello Rust".

- [ ] **Step 4: Commit any leftover**

```bash
git add -A
git commit -m "chore: build fixes if any"
```
(if no changes, skip this step)
