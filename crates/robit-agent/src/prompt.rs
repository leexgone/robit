//! System prompt builder — assembles the system prompt from multiple modules.

use std::path::Path;

use crate::datetime::current_date;

/// Default agent prompt template (user-editable part).
/// Does NOT include Skills/Environment sections - those are appended automatically.
const DEFAULT_AGENT_PROMPT: &str = include_str!("../prompts/default.md");

/// Built-in system prompt - automatically appended to all prompts.
/// Contains Skills and Environment sections (never overridden by users).
///
/// Note: tools are NOT listed here. They are exposed to the LLM exclusively
/// through the OpenAI function-calling `tools` request parameter (see
/// `ToolRegistry::tool_schemas`), which also carries each tool's parameter
/// JSON Schema.
const SYSTEM_PROMPT: &str = include_str!("../prompts/system.md");

pub struct PromptBuilder {
    custom_prompt: Option<String>,
}

impl PromptBuilder {
    pub fn new() -> Self {
        Self::with_working_dir(None)
    }

    /// Create PromptBuilder with a specific working directory to check for
    /// project-local custom prompt.
    /// Priority: {cwd}/.robit/prompts/agent.md > ~/.robit/prompts/agent.md
    pub fn with_working_dir(working_dir: Option<&Path>) -> Self {
        // Check project-local prompt first (higher priority)
        let local_prompt = working_dir.and_then(|cwd| {
            let path = cwd.join(".robit/prompts/agent.md");
            std::fs::read_to_string(&path).ok()
        });

        if local_prompt.is_some() {
            return Self {
                custom_prompt: local_prompt,
            };
        }

        // Fallback to global prompt
        let global_prompt = dirs::home_dir().and_then(|home| {
            let path = home.join(".robit/prompts/agent.md");
            std::fs::read_to_string(&path).ok()
        });

        Self {
            custom_prompt: global_prompt,
        }
    }

    /// Build the complete system prompt.
    ///
    /// The prompt is composed of:
    /// 1. Agent prompt (user-provided from agent.md, or default from default.md)
    /// 2. System prompt (Skills, Environment) - automatically appended,
    ///    defined in system.md (built-in, never overridden by users)
    ///
    /// `skills` is a list of (name, description) pairs for enabled skills.
    ///
    /// Tools are intentionally NOT part of the system prompt: they are exposed
    /// to the LLM via the OpenAI function-calling `tools` request parameter
    /// (`ToolRegistry::tool_schemas`), which also carries parameter schemas.
    pub fn build_system_prompt(
        &self,
        skills: &[(&str, &str)],
        working_dir: &std::path::Path,
    ) -> String {
        let skills_section = Self::build_skills_section(skills);
        let os = std::env::consts::OS;
        let cwd = working_dir.display().to_string();
        let date = current_date();

        // Select base prompt: custom agent prompt or default agent prompt
        let agent_prompt = self.custom_prompt.as_deref().unwrap_or(DEFAULT_AGENT_PROMPT);

        // Replace variables in the system prompt (Skills, Environment sections)
        let system_part = SYSTEM_PROMPT
            .replace("{os}", os)
            .replace("{cwd}", &cwd)
            .replace("{date}", &date)
            .replace("{skills_section}", &skills_section);

        // Combine: agent prompt + system prompt
        format!("{}\n\n{}", agent_prompt.trim(), system_part)
    }

    /// Build the skills description section.
    fn build_skills_section(skills: &[(&str, &str)]) -> String {
        if skills.is_empty() {
            return "(no available skills)".to_string();
        }

        let mut section = String::new();
        for (name, description) in skills {
            section.push_str(&format!("- **{}**: {}\n", name, description));
        }
        section
    }

}

impl Default for PromptBuilder {
    fn default() -> Self {
        Self::new()
    }
}
