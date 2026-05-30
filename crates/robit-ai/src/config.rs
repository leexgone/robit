//! Configuration loading for robit.toml.
//!
//! Loads a single unified config file from:
//!   1. `cwd/config/robit.toml` (project-local, highest priority)
//!   2. `~/.robit/robit.toml`   (global fallback)
//!
//! Environment variable substitution is supported in string fields via `${ENV_VAR}` syntax.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::LlmError;

// ============================================================================
// robit.toml structures
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct RobitConfig {
    pub llm: LlmSection,
    pub app: Option<AppConfig>,
}

#[derive(Debug, Deserialize)]
pub struct LlmSection {
    pub default_profile: Option<String>,
    pub profiles: HashMap<String, LlmProfile>,
}

#[derive(Debug, Deserialize)]
pub struct LlmProfile {
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub supports_images: Option<bool>,
    pub supports_tools: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct AppConfig {
    pub log_level: Option<String>,
    pub max_steps: Option<usize>,
    pub enabled_tools: Option<Vec<String>>,
    pub context: Option<ContextConfig>,
    pub retry: Option<RetryConfig>,
}

#[derive(Debug, Deserialize)]
pub struct ContextConfig {
    pub max_output_lines: Option<usize>,
    pub max_output_bytes: Option<usize>,
    pub reserve_ratio: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct RetryConfig {
    pub max_retries: Option<u32>,
    pub initial_backoff_ms: Option<u64>,
    pub max_backoff_ms: Option<u64>,
}

// ============================================================================
// Resolved profile reference
// ============================================================================

/// A fully resolved LLM profile ready for client construction.
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    pub profile_name: String,
    pub model_id: String,
    pub base_url: String,
    pub api_key: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub context_window: Option<u64>,
}

// ============================================================================
// Loader
// ============================================================================

/// Returns the ~/.robit/ directory path.
fn robit_home() -> Result<PathBuf, LlmError> {
    let home = dirs::home_dir().ok_or_else(|| {
        LlmError::ConfigError("无法获取用户主目录".to_string())
    })?;
    Ok(home.join(".robit"))
}

/// Replace `${ENV_VAR}` patterns with actual environment variable values.
fn resolve_env_var(value: &str) -> String {
    if let Some(var_name) = value.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
        std::env::var(var_name).unwrap_or_else(|_| value.to_string())
    } else {
        value.to_string()
    }
}

/// Load and parse the robit.toml config file.
///
/// Search order:
///   1. `cwd/config/robit.toml` (project-local)
///   2. `~/.robit/robit.toml`   (global fallback)
///
/// Load .env from ~/.robit/.env if it exists.
pub fn load_env() {
    if let Ok(robit_dir) = robit_home() {
        let env_path = robit_dir.join(".env");
        if env_path.exists() {
            let _ = dotenvy::from_path(&env_path);
        }
    }
}

/// Load the unified robit.toml config.
///
/// Automatically loads `~/.robit/.env` before resolving `${ENV_VAR}` patterns.
pub fn load_config() -> Result<RobitConfig, LlmError> {
    // Load .env first so ${ENV_VAR} substitutions work
    load_env();

    let path = find_config_path()?;

    let content = std::fs::read_to_string(&path).map_err(|e| {
        LlmError::ConfigError(format!("无法读取 {}: {}", path.display(), e))
    })?;

    let mut config: RobitConfig = toml::from_str(&content).map_err(|e| {
        LlmError::ConfigError(format!("解析 robit.toml 失败: {}", e))
    })?;

    // Resolve environment variables in api_key fields
    for profile in config.llm.profiles.values_mut() {
        profile.api_key = resolve_env_var(&profile.api_key);
    }

    Ok(config)
}

/// Find the config file path following the search order.
fn find_config_path() -> Result<PathBuf, LlmError> {
    // 1. Project-local: cwd/config/robit.toml
    if let Ok(cwd) = std::env::current_dir() {
        let local_path = cwd.join("config").join("robit.toml");
        if local_path.exists() {
            return Ok(local_path);
        }
    }

    // 2. Global: ~/.robit/robit.toml
    let global_path = robit_home()?.join("robit.toml");
    if global_path.exists() {
        return Ok(global_path);
    }

    Err(LlmError::ConfigError(format!(
        "未找到配置文件 robit.toml。\n\
         请创建以下任一文件:\n\
         - 项目本地: config/robit.toml\n\
         - 全局: {}",
        global_path.display()
    )))
}

/// Resolve which profile to use.
///
/// Priority: explicit `profile_name` > `llm.default_profile` > "default" > first available.
pub fn resolve_profile(
    config: &RobitConfig,
    profile_name: Option<&str>,
) -> Result<ResolvedModel, LlmError> {
    let name = profile_name
        .map(|s| s.to_string())
        .or_else(|| config.llm.default_profile.clone())
        .unwrap_or_else(|| "default".to_string());

    let profile = config.llm.profiles.get(&name).ok_or_else(|| {
        LlmError::ConfigError(format!(
            "Profile '{}' 未在 robit.toml 中定义。可用 profiles: {:?}",
            name,
            config.llm.profiles.keys().collect::<Vec<_>>()
        ))
    })?;

    if profile.api_key.is_empty() || profile.api_key.starts_with("${") {
        return Err(LlmError::ConfigError(format!(
            "Profile '{}' 的 API Key 未配置或环境变量未设置",
            name
        )));
    }

    Ok(ResolvedModel {
        profile_name: name,
        model_id: profile.model.clone(),
        base_url: profile.base_url.clone(),
        api_key: profile.api_key.clone(),
        max_tokens: profile.max_tokens,
        temperature: profile.temperature,
        context_window: profile.context_window,
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_env_var_with_env_set() {
        std::env::set_var("ROBIT_TEST_KEY", "test-value-123");
        assert_eq!(resolve_env_var("${ROBIT_TEST_KEY}"), "test-value-123");
        std::env::remove_var("ROBIT_TEST_KEY");
    }

    #[test]
    fn test_resolve_env_var_without_env() {
        assert_eq!(
            resolve_env_var("${ROBIT_NONEXISTENT_KEY}"),
            "${ROBIT_NONEXISTENT_KEY}"
        );
    }

    #[test]
    fn test_resolve_env_var_plain_string() {
        assert_eq!(resolve_env_var("plain-key"), "plain-key");
    }

    #[test]
    fn test_parse_robit_config() {
        let toml_str = r#"
            [llm]
            default_profile = "default"

            [llm.profiles.default]
            model = "deepseek-chat"
            base_url = "https://api.deepseek.com"
            api_key = "sk-test-key"
            max_tokens = 4096
            temperature = 0.0
            context_window = 65536

            [llm.profiles.chat]
            model = "deepseek-chat"
            base_url = "https://api.deepseek.com"
            api_key = "sk-test-key"

            [llm.profiles.reasoner]
            model = "deepseek-reasoner"
            base_url = "https://api.deepseek.com"
            api_key = "sk-test-key"

            [app]
            log_level = "DEBUG"
            max_steps = 10

            [app.context]
            max_output_lines = 500
            reserve_ratio = 0.2

            [app.retry]
            max_retries = 3
        "#;

        let config: RobitConfig = toml::from_str(toml_str).unwrap();

        // LLM section
        assert_eq!(config.llm.default_profile.as_deref(), Some("default"));
        assert_eq!(config.llm.profiles.len(), 3);

        let default = &config.llm.profiles["default"];
        assert_eq!(default.model, "deepseek-chat");
        assert_eq!(default.base_url, "https://api.deepseek.com");
        assert_eq!(default.api_key, "sk-test-key");
        assert_eq!(default.max_tokens, Some(4096));
        assert_eq!(default.temperature, Some(0.0));
        assert_eq!(default.context_window, Some(65536));

        let reasoner = &config.llm.profiles["reasoner"];
        assert_eq!(reasoner.model, "deepseek-reasoner");

        // App section
        let app = config.app.as_ref().unwrap();
        assert_eq!(app.log_level.as_deref(), Some("DEBUG"));
        assert_eq!(app.max_steps, Some(10));
        assert!(app.context.is_some());
        assert_eq!(app.context.as_ref().unwrap().max_output_lines, Some(500));
        assert!(app.retry.is_some());
        assert_eq!(app.retry.as_ref().unwrap().max_retries, Some(3));
    }

    #[test]
    fn test_parse_config_minimal() {
        let toml_str = r#"
            [llm]
            [llm.profiles.default]
            model = "deepseek-chat"
            base_url = "https://api.deepseek.com"
            api_key = "sk-test"
        "#;

        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        assert!(config.llm.default_profile.is_none());
        assert!(config.app.is_none());
    }

    #[test]
    fn test_resolve_profile_default() {
        let config = make_test_config();
        let resolved = resolve_profile(&config, None).unwrap();
        assert_eq!(resolved.profile_name, "default");
        assert_eq!(resolved.model_id, "deepseek-chat");
        assert_eq!(resolved.base_url, "https://api.deepseek.com");
        assert_eq!(resolved.api_key, "sk-test");
    }

    #[test]
    fn test_resolve_profile_explicit() {
        let config = make_test_config();
        let resolved = resolve_profile(&config, Some("reasoner")).unwrap();
        assert_eq!(resolved.profile_name, "reasoner");
        assert_eq!(resolved.model_id, "deepseek-reasoner");
    }

    #[test]
    fn test_resolve_profile_from_default_profile_field() {
        let toml_str = r#"
            [llm]
            default_profile = "chat"

            [llm.profiles.default]
            model = "model-a"
            base_url = "https://api.test.com"
            api_key = "sk-test"

            [llm.profiles.chat]
            model = "model-b"
            base_url = "https://api.test.com"
            api_key = "sk-test"
        "#;

        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        let resolved = resolve_profile(&config, None).unwrap();
        assert_eq!(resolved.profile_name, "chat");
        assert_eq!(resolved.model_id, "model-b");
    }

    #[test]
    fn test_resolve_profile_not_found() {
        let config = make_test_config();
        let result = resolve_profile(&config, Some("nonexistent"));
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_profile_empty_api_key() {
        let toml_str = r#"
            [llm]
            [llm.profiles.default]
            model = "deepseek-chat"
            base_url = "https://api.deepseek.com"
            api_key = ""
        "#;

        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        let result = resolve_profile(&config, None);
        assert!(result.is_err());
    }

    fn make_test_config() -> RobitConfig {
        let toml_str = r#"
            [llm]
            default_profile = "default"

            [llm.profiles.default]
            model = "deepseek-chat"
            base_url = "https://api.deepseek.com"
            api_key = "sk-test"

            [llm.profiles.reasoner]
            model = "deepseek-reasoner"
            base_url = "https://api.deepseek.com"
            api_key = "sk-test"
        "#;

        toml::from_str(toml_str).unwrap()
    }
}
