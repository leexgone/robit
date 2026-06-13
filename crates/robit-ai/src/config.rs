//! Configuration loading for config.toml.
//!
//! Loads a single unified config file from:
//!   1. `cwd/.robit/config.toml` (project-local, highest priority)
//!   2. `~/.robit/config.toml`   (global fallback)
//!
//! Configuration format uses a providers + models structure:
//! ```toml
//! default_model = "deepseek/deepseek-chat"
//!
//! [providers.deepseek]
//! name = "DeepSeek"
//! base_url = "https://api.deepseek.com/v1"
//! api_key = "${DEEPSEEK_API_KEY}"
//!
//! [[providers.deepseek.models]]
//! id = "deepseek-chat"
//! context_window = 65536
//! ```
//!
//! Environment variable substitution is supported in `api_key` fields via `${ENV_VAR}` syntax.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::LlmError;

// ============================================================================
// config.toml structures
// ============================================================================

/// Top-level config.toml configuration.
#[derive(Debug, Deserialize)]
pub struct RobitConfig {
    /// Default model in "provider/model" format (e.g. "deepseek/deepseek-chat").
    pub default_model: Option<String>,
    /// Provider definitions keyed by provider name.
    pub providers: HashMap<String, ProviderConfig>,
    /// Application settings.
    pub app: Option<AppConfig>,
}

/// A single LLM provider (one API endpoint with multiple models).
#[derive(Debug, Deserialize)]
pub struct ProviderConfig {
    /// Display name for the provider (optional).
    pub name: Option<String>,
    /// API base URL (must be OpenAI-compatible).
    pub base_url: String,
    /// API key (supports `${ENV_VAR}` substitution).
    pub api_key: String,
    /// Available models under this provider.
    pub models: Vec<ModelConfig>,
}

/// A single model definition within a provider.
#[derive(Debug, Deserialize)]
pub struct ModelConfig {
    /// Model ID used in API calls (e.g. "deepseek-chat").
    pub id: String,
    /// Display name (optional).
    pub name: Option<String>,
    /// Context window size in tokens (optional).
    pub context_window: Option<u64>,
    /// Maximum output tokens (optional).
    pub max_output_tokens: Option<u64>,
    /// Sampling temperature (optional, runtime parameter).
    pub temperature: Option<f32>,
    /// Maximum completion tokens (optional, runtime parameter).
    pub max_tokens: Option<u32>,
    /// Whether this model supports image inputs (optional, default false).
    pub supports_images: Option<bool>,
    /// Whether this model supports tool calling (optional, default false).
    pub supports_tools: Option<bool>,
}

// ============================================================================
// Application config (unchanged from previous version)
// ============================================================================

#[derive(Debug, Deserialize, Default)]
pub struct AppConfig {
    pub log_level: Option<String>,
    pub max_steps: Option<usize>,
    pub enabled_tools: Option<Vec<String>>,
    pub enabled_skills: Option<Vec<String>>,
    pub context: Option<ContextConfig>,
    pub retry: Option<RetryConfig>,
    pub auto_approve: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ContextConfig {
    pub max_output_lines: Option<usize>,
    pub max_output_bytes: Option<usize>,
    pub reserve_ratio: Option<f32>,
    /// Token threshold for triggering compression (default 5000).
    /// Only compress when removed messages exceed this token count.
    pub compression_token_threshold: Option<usize>,
    /// Enable/disable context compression (default true).
    pub compression_enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct RetryConfig {
    pub max_retries: Option<u32>,
    pub initial_backoff_ms: Option<u64>,
    pub max_backoff_ms: Option<u64>,
}

// ============================================================================
// Resolved model reference
// ============================================================================

/// A fully resolved model ready for client construction.
///
/// Merges provider-level settings (base_url, api_key) with model-level
/// settings (context_window, temperature, etc).
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
        LlmError::ConfigError("Cannot determine home directory".to_string())
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

/// Load and parse the config.toml config file.
///
/// Automatically loads `~/.robit/.env` before resolving `${ENV_VAR}` patterns.
///
/// Search order:
///   1. `workdir/.robit/config.toml` (project-local, if workdir provided)
///   2. `cwd/.robit/config.toml` (project-local, if workdir not provided)
///   3. `~/.robit/config.toml`   (global fallback)
pub fn load_config(workdir: Option<&std::path::Path>) -> Result<RobitConfig, LlmError> {
    // Load .env first so ${ENV_VAR} substitutions work
    load_env();

    let path = find_config_path(workdir)?;

    let content = std::fs::read_to_string(&path).map_err(|e| {
        LlmError::ConfigError(format!("Failed to read {}: {}", path.display(), e))
    })?;

    let mut config: RobitConfig = toml::from_str(&content).map_err(|e| {
        LlmError::ConfigError(format!("Failed to parse config.toml: {}", e))
    })?;

    // Resolve environment variables in api_key fields
    for provider in config.providers.values_mut() {
        provider.api_key = resolve_env_var(&provider.api_key);
    }

    Ok(config)
}

/// Load .env from ~/.robit/.env if it exists.
pub fn load_env() {
    if let Ok(robit_dir) = robit_home() {
        let env_path = robit_dir.join(".env");
        if env_path.exists() {
            let _ = dotenvy::from_path(&env_path);
        }
    }
}

/// Find the config file path following the search order.
fn find_config_path(workdir: Option<&std::path::Path>) -> Result<PathBuf, LlmError> {
    // 1. Project-local: workdir/.robit/config.toml (if workdir provided)
    if let Some(workdir) = workdir {
        let local_path = workdir.join(".robit").join("config.toml");
        if local_path.exists() {
            return Ok(local_path);
        }
    }

    // 2. Project-local: cwd/.robit/config.toml (if workdir not provided or no config there)
    if let Ok(cwd) = std::env::current_dir() {
        let local_path = cwd.join(".robit").join("config.toml");
        if local_path.exists() {
            return Ok(local_path);
        }
    }

    // 3. Global: ~/.robit/config.toml
    let global_path = robit_home()?.join("config.toml");
    if global_path.exists() {
        return Ok(global_path);
    }

    Err(LlmError::ConfigError(format!(
        "Configuration file config.toml not found.\n\
         Please create one of the following:\n\
         - Project-local: .robit/config.toml\n\
         - Global: {}",
        global_path.display()
    )))
}

/// Resolve which model to use.
///
/// `default_model` uses "provider/model" format.
/// Priority: explicit `provider_name` argument > `default_model` field > first available.
///
/// When `provider_name` is None, parses `default_model` (e.g. "deepseek/deepseek-chat")
/// into provider key and model ID.
pub fn resolve_profile(
    config: &RobitConfig,
    provider_name: Option<&str>,
) -> Result<ResolvedModel, LlmError> {
    let (provider_key, model_id) = if let Some(name) = provider_name {
        // Explicit provider override — use its first model
        let provider = config.providers.get(name).ok_or_else(|| {
            LlmError::ConfigError(format!(
                "Provider '{}' is not defined in config.toml. Available providers: {:?}",
                name,
                config.providers.keys().collect::<Vec<_>>()
            ))
        })?;
        let first_model = provider.models.first().ok_or_else(|| {
            LlmError::ConfigError(format!(
                "Provider '{}' has no models defined",
                name
            ))
        })?;
        (name.to_string(), first_model.id.clone())
    } else if let Some(ref default_model) = config.default_model {
        parse_default_model(default_model)?
    } else {
        // Fall back to first available provider + first model
        let (key, provider) = config.providers.iter().next().ok_or_else(|| {
            LlmError::ConfigError("No providers defined in config.toml".to_string())
        })?;
        let first_model = provider.models.first().ok_or_else(|| {
            LlmError::ConfigError(format!(
                "Provider '{}' has no models defined",
                key
            ))
        })?;
        (key.clone(), first_model.id.clone())
    };

    let provider = config.providers.get(&provider_key).ok_or_else(|| {
        LlmError::ConfigError(format!(
            "Provider '{}' is not defined in config.toml. Available providers: {:?}",
            provider_key,
            config.providers.keys().collect::<Vec<_>>()
        ))
    })?;

    // Find the matching model
    let model = provider.models.iter().find(|m| m.id == model_id).ok_or_else(|| {
        let available: Vec<&str> = provider.models.iter().map(|m| m.id.as_str()).collect();
        LlmError::ConfigError(format!(
            "Model '{}' not found in provider '{}'. Available models: {:?}",
            model_id, provider_key, available
        ))
    })?;

    // Validate API key
    if provider.api_key.is_empty() || provider.api_key.starts_with("${") {
        return Err(LlmError::ConfigError(format!(
            "Provider '{}' API key is not configured or the environment variable is not set",
            provider_key
        )));
    }

    Ok(ResolvedModel {
        profile_name: provider_key,
        model_id: model.id.clone(),
        base_url: provider.base_url.clone(),
        api_key: provider.api_key.clone(),
        max_tokens: model.max_tokens,
        temperature: model.temperature,
        context_window: model.context_window,
    })
}

/// Parse "provider/model" format from default_model.
///
/// Returns (provider_key, model_id).
fn parse_default_model(default_model: &str) -> Result<(String, String), LlmError> {
    let parts: Vec<&str> = default_model.splitn(2, '/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(LlmError::ConfigError(format!(
            "Invalid default_model '{}' format, expected 'provider/model' (e.g. 'deepseek/deepseek-chat')",
            default_model
        )));
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
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
            default_model = "deepseek/deepseek-chat"

            [providers.deepseek]
            name = "DeepSeek"
            base_url = "https://api.deepseek.com"
            api_key = "sk-test-key"

            [[providers.deepseek.models]]
            id = "deepseek-chat"
            name = "DeepSeek Chat"
            context_window = 65536
            max_output_tokens = 8192
            temperature = 0.0
            max_tokens = 4096

            [[providers.deepseek.models]]
            id = "deepseek-reasoner"
            name = "DeepSeek Reasoner"
            context_window = 65536
            temperature = 0.6

            [providers.qwen]
            name = "通义千问"
            base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
            api_key = "sk-qwen-key"

            [[providers.qwen.models]]
            id = "qwen-max"
            name = "Qwen Max"
            context_window = 32768

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

        // Default model
        assert_eq!(config.default_model.as_deref(), Some("deepseek/deepseek-chat"));

        // Providers
        assert_eq!(config.providers.len(), 2);

        // DeepSeek provider
        let ds = &config.providers["deepseek"];
        assert_eq!(ds.name.as_deref(), Some("DeepSeek"));
        assert_eq!(ds.base_url, "https://api.deepseek.com");
        assert_eq!(ds.api_key, "sk-test-key");
        assert_eq!(ds.models.len(), 2);
        assert_eq!(ds.models[0].id, "deepseek-chat");
        assert_eq!(ds.models[0].context_window, Some(65536));
        assert_eq!(ds.models[0].temperature, Some(0.0));
        assert_eq!(ds.models[0].max_tokens, Some(4096));
        assert_eq!(ds.models[1].id, "deepseek-reasoner");
        assert_eq!(ds.models[1].temperature, Some(0.6));

        // Qwen provider
        let qw = &config.providers["qwen"];
        assert_eq!(qw.name.as_deref(), Some("通义千问"));
        assert_eq!(qw.models.len(), 1);
        assert_eq!(qw.models[0].id, "qwen-max");

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
            [providers.default]
            base_url = "https://api.deepseek.com"
            api_key = "sk-test"

            [[providers.default.models]]
            id = "deepseek-chat"
        "#;

        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        assert!(config.default_model.is_none());
        assert!(config.app.is_none());
        assert_eq!(config.providers.len(), 1);
    }

    #[test]
    fn test_resolve_profile_from_default_model() {
        let config = make_test_config();
        let resolved = resolve_profile(&config, None).unwrap();
        assert_eq!(resolved.profile_name, "deepseek");
        assert_eq!(resolved.model_id, "deepseek-chat");
        assert_eq!(resolved.base_url, "https://api.deepseek.com");
        assert_eq!(resolved.api_key, "sk-test");
        assert_eq!(resolved.context_window, Some(65536));
        assert_eq!(resolved.temperature, Some(0.0));
        assert_eq!(resolved.max_tokens, Some(4096));
    }

    #[test]
    fn test_resolve_profile_explicit_provider() {
        let config = make_test_config();
        // Explicit provider — uses first model of that provider
        let resolved = resolve_profile(&config, Some("qwen")).unwrap();
        assert_eq!(resolved.profile_name, "qwen");
        assert_eq!(resolved.model_id, "qwen-max");
        assert_eq!(resolved.base_url, "https://dashscope.aliyuncs.com/compatible-mode/v1");
    }

    #[test]
    fn test_resolve_profile_first_available() {
        // No default_model and no explicit provider — use first available
        let toml_str = r#"
            [providers.deepseek]
            base_url = "https://api.deepseek.com"
            api_key = "sk-test"

            [[providers.deepseek.models]]
            id = "deepseek-chat"
        "#;
        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        let resolved = resolve_profile(&config, None).unwrap();
        assert_eq!(resolved.profile_name, "deepseek");
        assert_eq!(resolved.model_id, "deepseek-chat");
    }

    #[test]
    fn test_resolve_profile_not_found() {
        let config = make_test_config();
        let result = resolve_profile(&config, Some("nonexistent"));
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_profile_model_not_found() {
        let toml_str = r#"
            default_model = "deepseek/nonexistent-model"

            [providers.deepseek]
            base_url = "https://api.deepseek.com"
            api_key = "sk-test"

            [[providers.deepseek.models]]
            id = "deepseek-chat"
        "#;
        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        let result = resolve_profile(&config, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_profile_invalid_default_model_format() {
        let toml_str = r#"
            default_model = "invalid-no-slash"

            [providers.deepseek]
            base_url = "https://api.deepseek.com"
            api_key = "sk-test"

            [[providers.deepseek.models]]
            id = "deepseek-chat"
        "#;
        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        let result = resolve_profile(&config, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid default_model"));
    }

    #[test]
    fn test_resolve_profile_empty_api_key() {
        let toml_str = r#"
            [providers.deepseek]
            base_url = "https://api.deepseek.com"
            api_key = ""

            [[providers.deepseek.models]]
            id = "deepseek-chat"
        "#;
        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        let result = resolve_profile(&config, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_enabled_skills() {
        let toml_str = r#"
            default_model = "deepseek/deepseek-chat"

            [providers.deepseek]
            base_url = "https://api.deepseek.com"
            api_key = "sk-test"

            [[providers.deepseek.models]]
            id = "deepseek-chat"

            [app]
            enabled_skills = ["code-review", "refactor"]
        "#;

        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        let app = config.app.as_ref().unwrap();
        assert!(app.enabled_skills.is_some());
        let skills = app.enabled_skills.as_ref().unwrap();
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0], "code-review");
        assert_eq!(skills[1], "refactor");
    }

    #[test]
    fn test_parse_enabled_tools() {
        let toml_str = r#"
            default_model = "deepseek/deepseek-chat"

            [providers.deepseek]
            base_url = "https://api.deepseek.com"
            api_key = "sk-test"

            [[providers.deepseek.models]]
            id = "deepseek-chat"

            [app]
            enabled_tools = ["read", "bash", "edit", "write", "grep", "find", "ls"]
        "#;

        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        let app = config.app.as_ref().unwrap();
        assert!(app.enabled_tools.is_some());
        let tools = app.enabled_tools.as_ref().unwrap();
        assert_eq!(tools.len(), 7);
        assert_eq!(tools[0], "read");
        assert_eq!(tools[1], "bash");
        assert_eq!(tools[2], "edit");
        assert_eq!(tools[3], "write");
        assert_eq!(tools[4], "grep");
        assert_eq!(tools[5], "find");
        assert_eq!(tools[6], "ls");
    }

    #[test]
    fn test_parse_auto_approve() {
        let toml_str = r#"
            default_model = "deepseek/deepseek-chat"

            [providers.deepseek]
            base_url = "https://api.deepseek.com"
            api_key = "sk-test"

            [[providers.deepseek.models]]
            id = "deepseek-chat"

            [app]
            auto_approve = true
        "#;

        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        let app = config.app.as_ref().unwrap();
        assert_eq!(app.auto_approve, Some(true));
    }

    #[test]
    fn test_parse_auto_approve_default_none() {
        let toml_str = r#"
            default_model = "deepseek/deepseek-chat"

            [providers.deepseek]
            base_url = "https://api.deepseek.com"
            api_key = "sk-test"

            [[providers.deepseek.models]]
            id = "deepseek-chat"

            [app]
        "#;

        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        let app = config.app.as_ref().unwrap();
        assert_eq!(app.auto_approve, None);
    }

    fn make_test_config() -> RobitConfig {
        let toml_str = r#"
            default_model = "deepseek/deepseek-chat"

            [providers.deepseek]
            base_url = "https://api.deepseek.com"
            api_key = "sk-test"

            [[providers.deepseek.models]]
            id = "deepseek-chat"
            context_window = 65536
            temperature = 0.0
            max_tokens = 4096

            [providers.qwen]
            base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
            api_key = "sk-qwen-test"

            [[providers.qwen.models]]
            id = "qwen-max"
            context_window = 32768
        "#;

        toml::from_str(toml_str).unwrap()
    }
}
