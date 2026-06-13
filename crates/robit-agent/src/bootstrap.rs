//! Bootstrap module — common setup for loading skills and creating tools.
//!
//! This module provides reusable functions for frontends (robit-tui, robit-gui, etc.)
//! to avoid duplicating skill loading and tool creation code.

use std::path::PathBuf;
use std::sync::Arc;

use robit_ai::config::RobitConfig;

use crate::skill::{load_skills, Skill, SkillRegistry};
use crate::tool::bash::BashTool;
use crate::tool::edit::EditTool;
use crate::tool::find::FindTool;
use crate::tool::grep::GrepTool;
use crate::tool::load_skill::LoadSkillTool;
use crate::tool::ls::LsTool;
use crate::tool::read::ReadTool;
use crate::tool::write::WriteTool;
use crate::tool::ToolRegistry;
use crate::SkillLoadError;

// ============================================================================
// BootstrapResult
// ============================================================================

/// Result of bootstrapping skills and tools.
pub struct BootstrapResult {
    /// The skill registry, ready for use.
    pub skill_registry: Arc<SkillRegistry>,
    /// The tool registry, ready for use.
    pub tool_registry: Arc<ToolRegistry>,
    /// Total skills loaded (before filtering by enabled_skills).
    pub total_skills_loaded: usize,
    /// Any errors that occurred during skill loading (non-fatal).
    pub skill_load_errors: Vec<SkillLoadError>,
}

// ============================================================================
// Bootstrap functions
// ============================================================================

/// Bootstrap both skills and tools in one call.
///
/// This is the main entry point for frontends. It:
/// 1. Loads skills from global and project directories
/// 2. Filters skills by config.enabled_skills
/// 3. Creates SkillRegistry
/// 4. Creates ToolRegistry with all standard tools
///
/// Returns a BootstrapResult with both registries and metadata.
pub fn bootstrap(
    config: &RobitConfig,
    working_dir: &PathBuf,
    base_tool_names: &[&str],
) -> BootstrapResult {
    let (skills, skill_load_errors) = load_all_skills(working_dir);
    let total_skills_loaded = skills.len();

    let filtered_skills = filter_skills_by_config(skills, config);

    let skill_registry = Arc::new(SkillRegistry::new(filtered_skills, base_tool_names));
    let tool_registry = Arc::new(create_tools_from_config(config, Arc::clone(&skill_registry)));

    BootstrapResult {
        skill_registry,
        tool_registry,
        total_skills_loaded,
        skill_load_errors,
    }
}

/// Load skills from standard locations (global ~/.robit/skills and project .robit/skills).
///
/// Returns (loaded_skills, load_errors).
pub fn load_all_skills(working_dir: &PathBuf) -> (Vec<Skill>, Vec<SkillLoadError>) {
    let global_skills_dir = dirs::home_dir().map(|h| h.join(".robit/skills"));
    let project_skills_dir = Some(working_dir.join(".robit/skills"));

    load_skills(global_skills_dir, project_skills_dir)
}

/// Filter skills by the enabled_skills list in config, if present.
pub fn filter_skills_by_config(skills: Vec<Skill>, config: &RobitConfig) -> Vec<Skill> {
    let enabled_skills = config.app.as_ref().and_then(|a| a.enabled_skills.as_ref());

    match enabled_skills {
        Some(list) => skills
            .into_iter()
            .filter(|s| list.contains(&s.frontmatter.name))
            .collect(),
        None => skills,
    }
}

/// Create a ToolRegistry with tools filtered by config.enabled_tools.
///
/// - If enabled_tools is not specified: all tools are registered
/// - If enabled_tools is specified: only register tools in the list
/// - `read` and `load_skill` are always registered (required for basic functionality)
pub fn create_tools_from_config(
    config: &RobitConfig,
    skill_registry: Arc<SkillRegistry>,
) -> ToolRegistry {
    let mut tools = ToolRegistry::new();
    let context_config = config.app.as_ref().and_then(|a| a.context.as_ref());
    let max_lines = context_config.and_then(|c| c.max_output_lines).unwrap_or(500);
    let max_bytes = context_config
        .and_then(|c| c.max_output_bytes)
        .unwrap_or(51200);

    // Always register read and load_skill (required for basic functionality)
    tools.register(ReadTool::new(max_lines, max_bytes));
    tools.register(LoadSkillTool::new(skill_registry));

    // Get enabled tools from config
    let enabled_tools = config.app.as_ref().and_then(|a| a.enabled_tools.as_ref());

    match enabled_tools {
        Some(list) => {
            // Configured: only register specified tools (read and load_skill already registered)
            for tool_name in list {
                match tool_name.as_str() {
                    "read" => {} // already registered
                    "load_skill" => {} // already registered
                    "bash" => tools.register(BashTool::new(max_bytes)),
                    "write" => tools.register(WriteTool::new()),
                    "edit" => tools.register(EditTool::new()),
                    "ls" => tools.register(LsTool::new()),
                    "find" => tools.register(FindTool::new(max_bytes)),
                    "grep" => tools.register(GrepTool::new(max_lines, max_bytes)),
                    _ => tracing::warn!("Unknown tool in enabled_tools config: {}", tool_name),
                }
            }
        }
        None => {
            // Not configured: register all tools
            tools.register(BashTool::new(max_bytes));
            tools.register(WriteTool::new());
            tools.register(EditTool::new());
            tools.register(LsTool::new());
            tools.register(FindTool::new(max_bytes));
            tools.register(GrepTool::new(max_lines, max_bytes));
        }
    }

    tools
}

/// Log any skill load errors as warnings.
///
/// Convenience function for frontends to log errors without duplicating code.
pub fn log_skill_errors(errors: &[SkillLoadError]) {
    for err in errors {
        tracing::warn!("Skill load error: {:?}", err);
    }
}
