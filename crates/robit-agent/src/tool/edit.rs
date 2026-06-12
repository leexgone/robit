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

impl Default for EditTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Precisely replace text in a file. old_string must have a unique match in the file. Returns similar matches on failure to help with correction."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Target file path (relative or absolute)"
                },
                "old_string": {
                    "type": "string",
                    "description": "The original text to replace (must exist uniquely in the file)"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement text"
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
            Err(e) => return Ok(ToolResult::error(format!("Argument parsing failed: {}", e))),
        };

        // Validate inputs
        if parsed.old_string.is_empty() {
            return Ok(ToolResult::error("old_string cannot be empty".to_string()));
        }
        if parsed.file_path.trim().is_empty() {
            return Ok(ToolResult::error("File path cannot be empty".to_string()));
        }

        let path = resolve_path(&parsed.file_path, &ctx.working_dir);

        if !path.exists() {
            return Ok(ToolResult::error(format!("File not found: {}", path.display())));
        }

        if path.is_dir() {
            return Ok(ToolResult::error(format!(
                "'{}' is a directory, not a file",
                path.display()
            )));
        }

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult::error(format!(
                    "Failed to read file '{}': {}",
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
                    "No exact match found for old_string in the file.\n\
                     The following are the {} most similar matches. Please check if the selection is correct:\n\n",
                    similar.len()
                );
                for (i, m) in similar.iter().enumerate() {
                    msg.push_str(&format!(
                        "Match {} (line {}, score {}):\n  expected: {}\n  actual: {}\n",
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
                            "Modified file: {} (line {})",
                            path.display(),
                            line_num
                        )))
                    }
                    Err(e) => Ok(ToolResult::error(format!(
                        "Failed to write file '{}': {}",
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
                    "old_string appears {} times",
                    n
                );
                let lines_str: Vec<String> = line_positions.iter().map(|l| l.to_string()).collect();
                msg.push_str(&format!(" (lines {})", lines_str.join(", ")));
                msg.push_str(", cannot determine a unique replacement location.\nPlease provide more context to make old_string unique.\n\n");

                // Show context for each match (up to first 5)
                for &line_1based in line_positions.iter().take(5) {
                    let line_idx = line_1based - 1; // 0-based index into lines[]
                    let start = line_idx.saturating_sub(1);
                    let end = (line_idx + 2).min(lines.len());
                    msg.push_str("---\n");
                    msg.push_str(&format!("Line {}:\n", line_1based));
                    for (j, line_text) in lines.iter().enumerate().skip(start).take(end - start) {
                        if j == line_idx {
                            msg.push_str(&format!("> {}\n", line_text));
                        } else {
                            msg.push_str(&format!("  {}\n", line_text));
                        }
                    }
                    msg.push_str("---\n");
                }
                if n > 5 {
                    msg.push_str(&format!("... and {} more matches not shown\n", n - 5));
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

/// Find lines most similar to old_string using word overlap scoring.
fn find_similar_matches(content: &str, target: &str) -> Vec<SimilarMatch> {
    let lines: Vec<&str> = content.lines().collect();
    let target_lower = target.to_lowercase();
    // Filter out words shorter than 2 chars to avoid noise (e.g., "a", "i")
    let target_words: Vec<&str> = target_lower
        .split_whitespace()
        .filter(|w| w.len() >= 2)
        .collect();

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
    scored.sort_by_key(|b| std::cmp::Reverse(b.1));

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

/// Count how many words from target appear as exact matches in the line.
fn word_overlap_score(target_words: &[&str], line: &str) -> usize {
    let line_words: Vec<&str> = line.split_whitespace().collect();
    let mut score = 0;
    for &tw in target_words {
        if line_words.contains(&tw) {
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
