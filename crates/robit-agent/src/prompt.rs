//! System prompt builder — assembles the system prompt from multiple modules.

use std::path::PathBuf;

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
        let date = chrono_date();

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
            return "(无可用工具)".to_string();
        }

        let mut section = String::new();
        for tool in tools {
            section.push_str(&format!(
                "- **{}**: {}{}\n",
                tool.name(),
                tool.description(),
                if tool.requires_confirmation() {
                    "（需用户确认）"
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
            return "(无可用技能)".to_string();
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

/// Get today's date as a string (YYYY-MM-DD).
fn chrono_date() -> String {
    // Simple date formatting without chrono dependency.
    // Uses std::time which doesn't have formatting, so we use a basic approach.
    use std::time::SystemTime;
    let now = SystemTime::now();
    let duration = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();

    // Convert unix timestamp to date components
    let days = secs / 86400;
    let (year, month, day) = days_to_date(days);
    format!("{:04}-{:02}-{:02}", year, month, day)
}

/// Convert days since epoch to (year, month, day).
fn days_to_date(days: u64) -> (u64, u64, u64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
