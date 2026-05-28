//! Configuration loading for llms.toml, settings.toml, and .env files.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::LlmError;

// ============================================================================
// llms.toml structures
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct LlmConfig {
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
    pub providers: HashMap<String, ProviderConfig>,
}

#[derive(Debug, Deserialize)]
pub struct ProviderConfig {
    pub name: Option<String>,
    pub base_url: String,
    pub api_key: String,
    pub models: Vec<ModelConfig>,
}

#[derive(Debug, Deserialize)]
pub struct ModelConfig {
    pub id: String,
    pub name: Option<String>,
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub supports_images: Option<bool>,
    pub supports_tools: Option<bool>,
}

// ============================================================================
// settings.toml structures
// ============================================================================

#[derive(Debug, Deserialize, Default)]
pub struct SettingsConfig {
    pub model: Option<String>,
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
// Resolved model reference (provider key + model id)
// ============================================================================

#[derive(Debug, Clone)]
pub struct ResolvedModel {
    pub provider_key: String,
    pub model_id: String,
    pub base_url: String,
    pub api_key: String,
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

/// Load .env from ~/.robit/.env if it exists.
pub fn load_env() {
    if let Ok(robit_dir) = robit_home() {
        let env_path = robit_dir.join(".env");
        if env_path.exists() {
            let _ = dotenvy::from_path(&env_path);
        }
    }
}

/// Load and parse ~/.robit/llms.toml.
/// Resolves `${ENV_VAR}` in api_key fields after loading.
pub fn load_llm_config() -> Result<LlmConfig, LlmError> {
    let path = robit_home()?.join("llms.toml");
    let content = std::fs::read_to_string(&path).map_err(|e| {
        LlmError::ConfigError(format!("无法读取 {}: {}", path.display(), e))
    })?;

    let mut config: LlmConfig = toml::from_str(&content).map_err(|e| {
        LlmError::ConfigError(format!("解析 llms.toml 失败: {}", e))
    })?;

    // Resolve environment variables in api_key fields
    for provider in config.providers.values_mut() {
        provider.api_key = resolve_env_var(&provider.api_key);
    }

    Ok(config)
}

/// Load and parse ~/.robit/settings.toml.
/// Returns default SettingsConfig if the file does not exist.
pub fn load_settings() -> Result<SettingsConfig, LlmError> {
    let path = robit_home()?.join("settings.toml");
    if !path.exists() {
        return Ok(SettingsConfig::default());
    }

    let content = std::fs::read_to_string(&path).map_err(|e| {
        LlmError::ConfigError(format!("无法读取 {}: {}", path.display(), e))
    })?;

    let config: SettingsConfig = toml::from_str(&content).map_err(|e| {
        LlmError::ConfigError(format!("解析 settings.toml 失败: {}", e))
    })?;

    Ok(config)
}

/// Resolve which provider and model to use.
///
/// Priority: settings.toml `model` > llms.toml `default_model` > first available model.
pub fn resolve_model(
    llm_config: &LlmConfig,
    settings: &SettingsConfig,
) -> Result<ResolvedModel, LlmError> {
    // Determine the "provider/model" string
    let model_ref = settings
        .model
        .clone()
        .or_else(|| llm_config.default_model.clone())
        .or_else(|| {
            // Fallback: first provider, first model
            llm_config.providers.iter().find_map(|(key, provider)| {
                provider.models.first().map(|m| format!("{}/{}", key, m.id))
            })
        })
        .ok_or_else(|| {
            LlmError::ConfigError("未找到可用的模型配置".to_string())
        })?;

    // Parse "provider/model"
    let parts: Vec<&str> = model_ref.splitn(2, '/').collect();
    if parts.len() != 2 {
        return Err(LlmError::ConfigError(format!(
            "模型引用格式错误: '{}'，应为 'provider/model'",
            model_ref
        )));
    }

    let provider_key = parts[0];
    let model_id = parts[1];

    let provider = llm_config
        .providers
        .get(provider_key)
        .ok_or_else(|| {
            LlmError::ConfigError(format!("提供商 '{}' 未在 llms.toml 中定义", provider_key))
        })?;

    // Verify model exists in provider
    let _model = provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .ok_or_else(|| {
            LlmError::ModelNotFound {
                model: model_ref.clone(),
            }
        })?;

    if provider.api_key.is_empty() || provider.api_key.starts_with("${") {
        return Err(LlmError::ConfigError(format!(
            "提供商 '{}' 的 API Key 未配置或环境变量未设置",
            provider_key
        )));
    }

    Ok(ResolvedModel {
        provider_key: provider_key.to_string(),
        model_id: model_id.to_string(),
        base_url: provider.base_url.clone(),
        api_key: provider.api_key.clone(),
    })
}

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
    fn test_parse_llm_config_toml() {
        let toml_str = r#"
            default_provider = "deepseek"
            default_model = "deepseek/deepseek-chat"

            [providers.deepseek]
            name = "DeepSeek"
            base_url = "https://api.deepseek.com/v1"
            api_key = "sk-test-key"

            [[providers.deepseek.models]]
            id = "deepseek-chat"
            name = "DeepSeek Chat"
            context_window = 65536
            max_output_tokens = 8192
            supports_images = false
            supports_tools = true
        "#;

        let config: LlmConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.default_provider.as_deref(), Some("deepseek"));
        assert_eq!(config.default_model.as_deref(), Some("deepseek/deepseek-chat"));
        assert!(config.providers.contains_key("deepseek"));

        let provider = &config.providers["deepseek"];
        assert_eq!(provider.base_url, "https://api.deepseek.com/v1");
        assert_eq!(provider.api_key, "sk-test-key");
        assert_eq!(provider.models.len(), 1);
        assert_eq!(provider.models[0].id, "deepseek-chat");
        assert_eq!(provider.models[0].context_window, Some(65536));
    }

    #[test]
    fn test_parse_settings_toml() {
        let toml_str = r#"
            model = "deepseek/deepseek-chat"
            enabled_tools = ["read", "bash"]

            [context]
            max_output_lines = 500
            reserve_ratio = 0.2

            [retry]
            max_retries = 3
        "#;

        let settings: SettingsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(settings.model.as_deref(), Some("deepseek/deepseek-chat"));
        assert_eq!(
            settings.enabled_tools.as_deref(),
            Some(&["read".to_string(), "bash".to_string()][..])
        );
        assert!(settings.context.is_some());
        assert_eq!(settings.context.as_ref().unwrap().max_output_lines, Some(500));
        assert!(settings.retry.is_some());
        assert_eq!(settings.retry.as_ref().unwrap().max_retries, Some(3));
    }

    #[test]
    fn test_parse_settings_default() {
        let settings = SettingsConfig::default();
        assert!(settings.model.is_none());
        assert!(settings.enabled_tools.is_none());
        assert!(settings.context.is_none());
        assert!(settings.retry.is_none());
    }

    #[test]
    fn test_resolve_model_from_settings() {
        let toml_str = r#"
            default_provider = "deepseek"
            default_model = "deepseek/deepseek-chat"

            [providers.deepseek]
            base_url = "https://api.deepseek.com/v1"
            api_key = "sk-test"

            [[providers.deepseek.models]]
            id = "deepseek-chat"
        "#;

        let llm_config: LlmConfig = toml::from_str(toml_str).unwrap();
        let settings = SettingsConfig {
            model: Some("deepseek/deepseek-chat".to_string()),
            ..Default::default()
        };

        let resolved = resolve_model(&llm_config, &settings).unwrap();
        assert_eq!(resolved.provider_key, "deepseek");
        assert_eq!(resolved.model_id, "deepseek-chat");
        assert_eq!(resolved.base_url, "https://api.deepseek.com/v1");
        assert_eq!(resolved.api_key, "sk-test");
    }

    #[test]
    fn test_resolve_model_fallback_to_default() {
        let toml_str = r#"
            default_model = "deepseek/deepseek-chat"

            [providers.deepseek]
            base_url = "https://api.deepseek.com/v1"
            api_key = "sk-test"

            [[providers.deepseek.models]]
            id = "deepseek-chat"
        "#;

        let llm_config: LlmConfig = toml::from_str(toml_str).unwrap();
        let settings = SettingsConfig::default();

        let resolved = resolve_model(&llm_config, &settings).unwrap();
        assert_eq!(resolved.model_id, "deepseek-chat");
    }

    #[test]
    fn test_resolve_model_not_found() {
        let toml_str = r#"
            [providers.deepseek]
            base_url = "https://api.deepseek.com/v1"
            api_key = "sk-test"

            [[providers.deepseek.models]]
            id = "deepseek-chat"
        "#;

        let llm_config: LlmConfig = toml::from_str(toml_str).unwrap();
        let settings = SettingsConfig {
            model: Some("deepseek/nonexistent".to_string()),
            ..Default::default()
        };

        let result = resolve_model(&llm_config, &settings);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_model_invalid_format() {
        let toml_str = r#"
            [providers.deepseek]
            base_url = "https://api.deepseek.com/v1"
            api_key = "sk-test"

            [[providers.deepseek.models]]
            id = "deepseek-chat"
        "#;

        let llm_config: LlmConfig = toml::from_str(toml_str).unwrap();
        let settings = SettingsConfig {
            model: Some("no-slash-here".to_string()),
            ..Default::default()
        };

        let result = resolve_model(&llm_config, &settings);
        assert!(result.is_err());
    }
}
