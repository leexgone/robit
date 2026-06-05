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
                        "匹配 {} (第 {} 行, 匹配度 {}):\n  期望: {}\n  实际: {}\n",
                        i + 1,
                        m.line_number,
                        m.score,
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
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len).collect();
        format!("{}...", truncated)
    }
}
