//! Skill system — predefined prompt templates loaded from YAML frontmatter + Markdown body files.
//!
//! Skills are declarative data (no trait, no execute method). They are loaded at startup,
//! their descriptions are injected into the system prompt, and their full content is injected
//! when a user triggers them via a slash command.

pub mod loader;
mod registry;

use std::path::PathBuf;

use serde::Deserialize;

pub use loader::{load_skills, parse_skill_file, SkillLoadError};
pub use registry::SkillRegistry;

// ============================================================================
// Frontmatter
// ============================================================================

/// YAML frontmatter fields parsed from the skill file header.
#[derive(Debug, Clone, Deserialize)]
pub struct SkillFrontmatter {
    /// Skill unique identifier (required).
    pub name: String,
    /// Skill description for user and Agent (required).
    pub description: String,
    /// Semantic version (optional, default "1.0.0").
    #[serde(default = "default_version")]
    pub version: String,
    /// Trigger command list (optional, e.g. "/review"). Empty = only system prompt injection.
    #[serde(default)]
    pub triggers: Vec<String>,
    /// Required tools list (optional). If a required tool is unavailable, a warning is logged.
    #[serde(default)]
    pub tools_required: Vec<String>,
    /// Whether this skill is enabled (optional, default true).
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

fn default_enabled() -> bool {
    true
}

// ============================================================================
// Skill
// ============================================================================

/// A fully parsed skill with its frontmatter and markdown body content.
#[derive(Debug, Clone)]
pub struct Skill {
    /// The parsed frontmatter metadata.
    pub frontmatter: SkillFrontmatter,
    /// The markdown body content (everything after the closing `---`).
    pub content: String,
    /// The source file path (for debugging and display).
    pub source_path: PathBuf,
}
