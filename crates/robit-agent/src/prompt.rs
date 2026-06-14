//! System prompt builder — assembles the system prompt from multiple modules.

use std::path::PathBuf;

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
        // Check for custom prompt file
        let custom_path = Self::custom_prompt_path();
        let custom_prompt = if let Some(path) = custom_path {
            std::fs::read_to_string(&path).ok()
        } else {
            None
        };

        Self { custom_prompt }
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

    fn custom_prompt_path() -> Option<PathBuf> {
        dirs::home_dir().map(|home| home.join(".robit/prompts/system.txt"))
    }
}

impl Default for PromptBuilder {
    fn default() -> Self {
        Self::new()
    }
}
