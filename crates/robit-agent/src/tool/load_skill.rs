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
        "Load detailed content of a specified skill. Returns skill metadata (name, description), full Markdown content, and source file path."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "skill_name": {
                    "type": "string",
                    "description": "Name of the skill to load (English name, exact match)"
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
            Err(e) => return Ok(ToolResult::error(format!("Argument parsing failed: {}", e))),
        };

        let skill_name = parsed.skill_name.trim();

        match self.skills.get(skill_name) {
            Some(skill) => {
                let output = format!(
                    "## Skill: {name}\n\n\
                     **Description**: {description}\n\
                     **Version**: {version}\n\
                     **Trigger commands**: {triggers}\n\
                     **Source file**: {source_path}\n\n\
                     ---\n\n\
                     {content}",
                    name = skill.frontmatter.name,
                    description = skill.frontmatter.description,
                    version = skill.frontmatter.version,
                    triggers = if skill.frontmatter.triggers.is_empty() {
                        "(None)".to_string()
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
                    "Skill '{}' does not exist. Available skills: {:?}",
                    skill_name, available
                )))
            }
        }
    }
}
