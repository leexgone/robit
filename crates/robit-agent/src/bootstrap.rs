//! Bootstrap module — common setup for loading skills and creating tools.
//!
//! This module provides reusable functions for frontends (robit-tui, robit-gui, etc.)
//! to avoid duplicating skill loading and tool creation code.

use std::path::PathBuf;
use std::sync::Arc;

use robit_ai::config::{
    resolve_image_provider, resolve_memory_mode, resolve_profile, resolve_vision_model,
    MemoryMode, RobitConfig,
};
use robit_ai::LlmClient;

use crate::image_gen::ImageGenClient;
use crate::skill::{load_skills, Skill, SkillRegistry};
use crate::tool::bash::BashTool;
use crate::tool::edit::EditTool;
use crate::tool::find::FindTool;
use crate::tool::generate_image::GenerateImageTool;
use crate::tool::grep::GrepTool;
use crate::tool::load_skill::LoadSkillTool;
use crate::tool::ls::LsTool;
use crate::tool::memory::{ForgetTool, ListMemoriesTool, MemorizeTool, RecallTool};
use crate::tool::query_task::QueryTaskTool;
use crate::tool::read::ReadTool;
use crate::tool::read_image::ReadImageTool;
use crate::tool::search_history::SearchHistoryTool;
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
/// - Memory tools (`memorize`/`recall`/`forget`/`list_memories`) are registered
///   only when `memory_mode = "tools"`; `file`/`off` modes skip them entirely.
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

    // Whether the configured default model supports image inputs. The `read`
    // tool uses this to decide whether to encode image files and to advertise
    // image support in its description.
    let supports_images = resolve_profile(config, None)
        .map(|m| m.supports_images)
        .unwrap_or(false);

    // Max dimension for images encoded into the context (downscaled + JPEG
    // beyond that; 0 disables).
    let max_image_dimension = context_config
        .and_then(|c| c.max_image_dimension)
        .unwrap_or(1024);

    // Memory tools are only registered in "tools" mode. The default is
    // "file" (file-based memory), where they are entirely absent from the
    // ToolRegistry and thus invisible to the LLM.
    let memory_mode = resolve_memory_mode(config);
    let memory_tools_enabled = memory_mode == MemoryMode::Tools;

    // Always register read, load_skill, history, and query_task tools
    // (required for basic functionality / async task visibility)
    tools.register(ReadTool::new(
        max_lines,
        max_bytes,
        supports_images,
        max_image_dimension,
    ));
    tools.register(LoadSkillTool::new(skill_registry));
    if memory_tools_enabled {
        tools.register(MemorizeTool::new());
        tools.register(RecallTool::new());
        tools.register(ForgetTool::new());
        tools.register(ListMemoriesTool::new());
    }
    tools.register(SearchHistoryTool::new());
    tools.register(QueryTaskTool::new());

    // Try to build the image generation client. Returns None when no image
    // providers are configured (the tool is simply not registered in that case).
    let mut image_client = build_image_client(config);

    // The read_image tool is only useful when the main model cannot see
    // images itself. When the main model supports images, `read` already
    // injects them into the context and read_image would be redundant.
    let mut vision_client = if supports_images {
        if config.default_vision_model.is_some() {
            tracing::info!(
                "Main model supports image inputs; default_vision_model is ignored and \
                 the read_image tool is not registered"
            );
        }
        None
    } else {
        build_vision_client(config)
    };

    // Get enabled tools from config
    let enabled_tools = config.app.as_ref().and_then(|a| a.enabled_tools.as_ref());

    match enabled_tools {
        Some(list) => {
            // Configured: only register specified tools (always available tools already registered)
            for tool_name in list {
                match tool_name.as_str() {
                    "read" => {} // already registered
                    "load_skill" => {} // already registered
                    "memorize" | "recall" | "forget" | "list_memories" => {
                        // Only effective in "tools" mode; skipped otherwise.
                        if !memory_tools_enabled {
                            tracing::warn!(
                                "memory tool {} listed in enabled_tools but memory_mode \
                                 != \"tools\", skipping",
                                tool_name
                            );
                        }
                    }
                    "search_history" => {} // already registered
                    "query_task" => {} // already registered
                    "bash" => tools.register(BashTool::new(max_bytes)),
                    "write" => tools.register(WriteTool::new()),
                    "edit" => tools.register(EditTool::new()),
                    "ls" => tools.register(LsTool::new()),
                    "find" => tools.register(FindTool::new(max_bytes)),
                    "grep" => tools.register(GrepTool::new(max_lines, max_bytes)),
                    "generate_image" => match image_client.take() {
                        Some(client) => tools.register(GenerateImageTool::new(client)),
                        None => tracing::warn!(
                            "generate_image listed in enabled_tools but no image_provider \
                             is configured, skipping"
                        ),
                    },
                    "read_image" => match vision_client.take() {
                        Some(client) => tools.register(ReadImageTool::new(
                            client,
                            max_image_dimension,
                            max_bytes,
                        )),
                        None => tracing::warn!(
                            "read_image listed in enabled_tools but unavailable (main model \
                             supports images, or default_vision_model is not configured/invalid), \
                             skipping"
                        ),
                    },
                    _ => tracing::warn!("Unknown tool in enabled_tools config: {}", tool_name),
                }
            }
        }
        None => {
            // Not configured: register all remaining tools
            tools.register(BashTool::new(max_bytes));
            tools.register(WriteTool::new());
            tools.register(EditTool::new());
            tools.register(LsTool::new());
            tools.register(FindTool::new(max_bytes));
            tools.register(GrepTool::new(max_lines, max_bytes));
            if let Some(client) = image_client.take() {
                tools.register(GenerateImageTool::new(client));
            }
            if let Some(client) = vision_client.take() {
                tools.register(ReadImageTool::new(client, max_image_dimension, max_bytes));
            }
        }
    }

    // The file-memory mechanism instructs the agent (via the system prompt)
    // to create/update memory files with write/edit. If those tools are not
    // registered, the instructions are unactionable and memory silently
    // never persists — warn loudly at startup.
    if memory_mode == MemoryMode::File {
        let missing = missing_file_memory_tools(&tools.tool_names());
        if !missing.is_empty() {
            tracing::warn!(
                "memory_mode = \"file\" but tool(s) [{}] are not enabled; the agent \
                 cannot persist memory files (the system prompt instructs it to use \
                 write/edit). Enable them, or set memory_mode = \"off\" to hide the \
                 mechanism.",
                missing.join(", ")
            );
        }
    }

    tools
}

/// Tools the file-memory mechanism relies on to create/update memory files.
const FILE_MEMORY_REQUIRED_TOOLS: [&str; 2] = ["write", "edit"];

/// Required file-memory tools missing from `names` (empty when all present).
/// Pure helper split out from `create_tools_from_config` for testability.
fn missing_file_memory_tools(names: &[&str]) -> Vec<&'static str> {
    FILE_MEMORY_REQUIRED_TOOLS
        .into_iter()
        .filter(|tool| !names.contains(tool))
        .collect()
}

/// Build the image generation client from config.
///
/// Returns `None` (without warning) when image generation is not configured -
/// either no image providers are defined, or `default_image_model` is absent.
/// Both are normal "user doesn't need image generation" states. Returns `None`
/// with a warning only when configuration is present but invalid (e.g. missing
/// API key or invalid model reference).
fn build_image_client(config: &RobitConfig) -> Option<ImageGenClient> {
    if config.image_providers.is_empty() {
        return None;
    }
    // default_image_model is required; if absent, image generation is
    // considered disabled (no warning - this is a valid "not using it" state).
    if config.default_image_model.is_none() {
        return None;
    }
    match resolve_image_provider(config) {
        Ok(provider) => {
            tracing::info!(
                "Image generation provider configured: {}/{} (protocol: {:?}, mode: {:?})",
                provider.provider_name,
                provider.model_id,
                provider.protocol,
                provider.mode
            );
            Some(ImageGenClient::new(provider))
        }
        Err(e) => {
            tracing::warn!("Failed to resolve image generation provider: {}", e);
            None
        }
    }
}

/// Build the vision model client used by the `read_image` tool.
///
/// Returns `None` (without warning) when `default_vision_model` is absent —
/// a normal "user doesn't need image recognition" state. Returns `None` with
/// a warning only when the configuration is present but invalid.
fn build_vision_client(config: &RobitConfig) -> Option<LlmClient> {
    if config.default_vision_model.is_none() {
        return None;
    }
    match resolve_vision_model(config) {
        Ok(resolved) => {
            // ModelConfig.supports_images defaults to false, so a user adding
            // e.g. qwen-vl-max may have forgotten to mark it. Warn but don't
            // refuse — the model id is what matters here.
            if !resolved.supports_images {
                tracing::warn!(
                    "Vision model '{}/{}' is not marked supports_images = true in config.toml; \
                     the read_image tool may fail if the model cannot process images",
                    resolved.profile_name,
                    resolved.model_id
                );
            }
            tracing::info!(
                "Vision model configured: {}/{} (for the read_image tool)",
                resolved.profile_name,
                resolved.model_id
            );
            Some(LlmClient::from_resolved(resolved))
        }
        Err(e) => {
            tracing::warn!("Failed to resolve vision model: {}", e);
            None
        }
    }
}

/// Log any skill load errors as warnings.
///
/// Convenience function for frontends to log errors without duplicating code.
pub fn log_skill_errors(errors: &[SkillLoadError]) {
    for err in errors {
        tracing::warn!("Skill load error: {:?}", err);
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use robit_ai::config::{AppConfig, MemoryMode, ModelConfig, ProviderConfig, RobitConfig};
    use std::collections::HashMap;

    const MEMORY_TOOL_NAMES: [&str; 4] = ["memorize", "recall", "forget", "list_memories"];

    fn config_with_memory_mode(mode: Option<MemoryMode>) -> RobitConfig {
        RobitConfig {
            default_model: None,
            providers: HashMap::new(),
            app: Some(AppConfig {
                memory_mode: mode,
                ..Default::default()
            }),
            channels: None,
            default_image_model: None,
            image_providers: HashMap::new(),
            default_vision_model: None,
        }
    }

    fn registry_for(config: &RobitConfig) -> ToolRegistry {
        let skills = Arc::new(SkillRegistry::new(Vec::new(), &[]));
        create_tools_from_config(config, skills)
    }

    #[test]
    fn memory_tools_registered_in_tools_mode() {
        let registry = registry_for(&config_with_memory_mode(Some(MemoryMode::Tools)));
        let names = registry.tool_names();
        for tool in MEMORY_TOOL_NAMES {
            assert!(names.contains(&tool), "missing {}", tool);
        }
    }

    #[test]
    fn memory_tools_not_registered_in_file_mode() {
        let registry = registry_for(&config_with_memory_mode(Some(MemoryMode::File)));
        let names = registry.tool_names();
        for tool in MEMORY_TOOL_NAMES {
            assert!(!names.contains(&tool), "{} should not be registered", tool);
        }
        assert!(names.contains(&"read"));
    }

    #[test]
    fn memory_tools_not_registered_by_default() {
        // 未配置 memory_mode（老用户升级路径）→ 默认 file，不注册记忆工具
        let registry = registry_for(&config_with_memory_mode(None));
        let names = registry.tool_names();
        for tool in MEMORY_TOOL_NAMES {
            assert!(!names.contains(&tool));
        }
    }

    #[test]
    fn memory_tools_not_registered_in_off_mode() {
        let registry = registry_for(&config_with_memory_mode(Some(MemoryMode::Off)));
        for tool in MEMORY_TOOL_NAMES {
            assert!(!registry.tool_names().contains(&tool));
        }
    }

    #[test]
    fn memory_tools_in_enabled_tools_list_ignored_in_file_mode() {
        // enabled_tools 显式列出记忆工具但模式非 tools → 跳过注册（warn），不报错
        let mut config = config_with_memory_mode(Some(MemoryMode::File));
        config.app.as_mut().unwrap().enabled_tools =
            Some(vec!["read".into(), "memorize".into(), "recall".into()]);
        let registry = registry_for(&config);
        let names = registry.tool_names();
        assert!(names.contains(&"read"));
        for tool in MEMORY_TOOL_NAMES {
            assert!(!names.contains(&tool));
        }
    }

    #[test]
    fn missing_file_memory_tools_empty_when_all_present() {
        assert!(missing_file_memory_tools(&["read", "write", "edit", "bash"]).is_empty());
    }

    #[test]
    fn missing_file_memory_tools_reports_each_missing() {
        assert_eq!(missing_file_memory_tools(&["read", "edit"]), vec!["write"]);
        assert_eq!(missing_file_memory_tools(&["read", "write"]), vec!["edit"]);
        assert_eq!(missing_file_memory_tools(&["read"]), vec!["write", "edit"]);
    }

    /// Config with a valid main model (image support configurable) and an
    /// optional `default_vision_model` referencing "test/vl-model".
    fn vision_test_config(
        main_supports_images: bool,
        vision_model: Option<&str>,
        enabled_tools: Option<Vec<String>>,
    ) -> RobitConfig {
        let model = |id: &str, supports_images: bool| ModelConfig {
            id: id.to_string(),
            name: None,
            context_window: None,
            max_output_tokens: None,
            temperature: None,
            max_tokens: None,
            supports_images: Some(supports_images),
            supports_tools: Some(true),
        };
        RobitConfig {
            default_model: Some("test/main-chat".to_string()),
            providers: HashMap::from([(
                "test".to_string(),
                ProviderConfig {
                    name: None,
                    base_url: "https://example.com/v1".to_string(),
                    api_key: "sk-test".to_string(),
                    models: vec![
                        model("main-chat", main_supports_images),
                        model("vl-model", true),
                    ],
                },
            )]),
            app: Some(AppConfig {
                enabled_tools,
                ..Default::default()
            }),
            channels: None,
            default_image_model: None,
            image_providers: HashMap::new(),
            default_vision_model: vision_model.map(|s| s.to_string()),
        }
    }

    #[test]
    fn read_image_not_registered_when_not_configured() {
        // Main model lacks image support but no default_vision_model → skip.
        let config = vision_test_config(false, None, None);
        assert!(!registry_for(&config).tool_names().contains(&"read_image"));
    }

    #[test]
    fn read_image_registered_when_main_model_lacks_image_support() {
        let config = vision_test_config(false, Some("test/vl-model"), None);
        assert!(registry_for(&config).tool_names().contains(&"read_image"));
    }

    #[test]
    fn read_image_registered_via_enabled_tools_list() {
        let config =
            vision_test_config(false, Some("test/vl-model"), Some(vec!["read_image".into()]));
        assert!(registry_for(&config).tool_names().contains(&"read_image"));
    }

    #[test]
    fn read_image_not_registered_when_main_model_supports_images() {
        // Main model sees images itself → read is enough, vision model ignored.
        let config = vision_test_config(true, Some("test/vl-model"), None);
        assert!(!registry_for(&config).tool_names().contains(&"read_image"));
    }

    #[test]
    fn read_image_not_registered_when_reference_invalid() {
        // default_vision_model points at a model that doesn't exist → warn + skip.
        let config = vision_test_config(false, Some("test/nonexistent"), None);
        assert!(!registry_for(&config).tool_names().contains(&"read_image"));
    }

    #[test]
    fn read_image_skipped_in_enabled_tools_when_unavailable() {
        // Listed in enabled_tools but the main model supports images → skip
        // with a warning, no panic.
        let config =
            vision_test_config(true, Some("test/vl-model"), Some(vec!["read_image".into()]));
        assert!(!registry_for(&config).tool_names().contains(&"read_image"));
    }
}
