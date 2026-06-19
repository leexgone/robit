//! System prompt builder — assembles the system prompt from multiple modules.

use std::path::Path;

use crate::datetime::current_date;
use crate::tool::Tool;

/// Default system prompt template.
/// Placeholders: {os}, {cwd}, {date}, {tools_section}, {skills_section}
const DEFAULT_PROMPT: &str = include_str!("../prompts/default.md");

pub struct PromptBuilder {
    custom_prompt: Option<String>,
}

impl PromptBuilder {
    pub fn new() -> Self {
        Self::with_working_dir(None)
    }

    /// Create PromptBuilder with a specific working directory to check for
    /// project-local custom prompt.
    /// Priority: {cwd}/.robit/prompts/system.md > ~/.robit/prompts/system.md
    pub fn with_working_dir(working_dir: Option<&Path>) -> Self {
        // Check project-local prompt first (higher priority)
        let local_prompt = working_dir.and_then(|cwd| {
            let path = cwd.join(".robit/prompts/system.md");
            std::fs::read_to_string(&path).ok()
        });

        if local_prompt.is_some() {
            return Self {
                custom_prompt: local_prompt,
            };
        }

        // Fallback to global prompt
        let global_prompt = dirs::home_dir().and_then(|home| {
            let path = home.join(".robit/prompts/system.md");
            std::fs::read_to_string(&path).ok()
        });

        Self {
            custom_prompt: global_prompt,
        }
    }

    /// Build the complete system prompt.
    ///
    /// `skills` is a list of (name, description) pairs for enabled skills.
    pub fn build_system_prompt(
        &self,
        tools: &[&dyn Tool],
        skills: &[(&str, &str)],
        working_dir: &std::path::Path,
    ) -> String {
        let tools_section = Self::build_tools_section(tools);
        let skills_section = Self::build_skills_section(skills);
        let os = std::env::consts::OS;
        let cwd = working_dir.display().to_string();
        let date = current_date();

        if let Some(custom) = &self.custom_prompt {
            // Custom prompt: still inject dynamic variables
            custom
                .replace("{os}", os)
                .replace("{cwd}", &cwd)
                .replace("{date}", &date)
                .replace("{tools_section}", &tools_section)
                .replace("{skills_section}", &skills_section)
        } else {
            DEFAULT_PROMPT
                .replace("{os}", os)
                .replace("{cwd}", &cwd)
                .replace("{date}", &date)
                .replace("{tools_section}", &tools_section)
                .replace("{skills_section}", &skills_section)
        }
    }

    /// Build the tools description section.
    fn build_tools_section(tools: &[&dyn Tool]) -> String {
        if tools.is_empty() {
            return "(no available tools)".to_string();
        }

        let mut section = String::new();
        for tool in tools {
            section.push_str(&format!(
                "- **{}**: {}{}\n",
                tool.name(),
                tool.description(),
                if tool.requires_confirmation() {
                    " (requires user confirmation)"
                } else {
                    ""
                }
            ));
        }
        section
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
