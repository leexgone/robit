//! `load_skill` tool — loads a skill's full content by name.
//!
//! Returns metadata (name, description), the markdown body, and the source
//! file path so the LLM can optionally use `read` to inspect the raw file.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use super::{Tool, ToolContext, ToolResult};
use crate::error::Result;
use crate::skill::SkillRegistry;

pub struct LoadSkillTool {
    skills: Arc<SkillRegistry>,
}

#[derive(Debug, Deserialize)]
struct LoadSkillArgs {
    /// Skill name to load. Use /skills to list available skills.
    skill_name: String,
}

impl LoadSkillTool {
    pub fn new(skills: Arc<SkillRegistry>) -> Self {
        Self { skills }
    }
}

#[async_trait]
impl Tool for LoadSkillTool {
    fn name(&self) -> &str {
        "load_skill"
    }

    fn description(&self) -> &str {
        "加载指定技能的详细内容。返回技能元数据（名称、描述）、完整的 Markdown 内容以及源文件路径。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "skill_name": {
                    "type": "string",
                    "description": "要加载的技能名称（英文名称，精确匹配）"
                }
            },
            "required": ["skill_name"]
        })
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        let parsed: LoadSkillArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("参数解析失败: {}", e))),
        };

        let skill_name = parsed.skill_name.trim();

        match self.skills.get(skill_name) {
            Some(skill) => {
                let output = format!(
                    "## 技能: {name}\n\n\
                     **描述**: {description}\n\
                     **版本**: {version}\n\
                     **触发命令**: {triggers}\n\
                     **源文件**: {source_path}\n\n\
                     ---\n\n\
                     {content}",
                    name = skill.frontmatter.name,
                    description = skill.frontmatter.description,
                    version = skill.frontmatter.version,
                    triggers = if skill.frontmatter.triggers.is_empty() {
                        "(无)".to_string()
                    } else {
                        skill.frontmatter.triggers.join(", ")
                    },
                    source_path = skill.source_path.display(),
                    content = skill.content,
                );
                Ok(ToolResult::success(output))
            }
            None => {
                let available: Vec<&str> = self.skills.skill_names();
                Ok(ToolResult::error(format!(
                    "技能 '{}' 不存在。可用技能: {:?}",
                    skill_name, available
                )))
            }
        }
    }
}
