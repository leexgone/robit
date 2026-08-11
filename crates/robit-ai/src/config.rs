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
    /// Communication channel configurations (QQ Bot, Feishu, etc.).
    #[serde(default)]
    pub channels: Option<ChannelsConfig>,
    /// Default image generation model in "provider/model" format
    /// (e.g. "wanxiang/wan2.7-image-pro"). Only effective when
    /// `image_providers` is also configured.
    pub default_image_model: Option<String>,
    /// Image generation provider definitions, keyed by provider name.
    #[serde(default)]
    pub image_providers: HashMap<String, ImageProviderConfig>,
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
// Image generation provider config
// ============================================================================

/// Protocol used by an image generation provider.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImageProtocol {
    /// OpenAI-compatible Images API (`POST /images/generations`).
    Openai,
    /// DashScope native protocol (Wanxiang, supports sync/async modes).
    Dashscope,
}

impl Default for ImageProtocol {
    fn default() -> Self {
        Self::Openai
    }
}

/// Call mode for DashScope image generation.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImageCallMode {
    /// Synchronous call: one request returns the result directly.
    Sync,
    /// Asynchronous call: submit a task, then poll until completion.
    Async,
}

impl Default for ImageCallMode {
    fn default() -> Self {
        Self::Sync
    }
}

/// A single image generation model definition.
#[derive(Debug, Deserialize, Clone)]
pub struct ImageModelConfig {
    /// Model ID used in API calls (e.g. "wan2.7-image-pro").
    pub id: String,
    /// Display name (optional).
    pub name: Option<String>,
}

/// An image generation provider (one API endpoint with multiple models).
#[derive(Debug, Deserialize, Clone)]
pub struct ImageProviderConfig {
    /// Display name for the provider (optional).
    pub name: Option<String>,
    /// API base URL. For DashScope this includes the `/api/v1` prefix
    /// (e.g. `https://dashscope.aliyuncs.com/api/v1`). For OpenAI-compatible
    /// providers, include the `/v1` prefix (e.g. `https://api.openai.com/v1`).
    /// This mirrors how chat `providers` configure `base_url` - the client
    /// only appends the endpoint path, not a version prefix.
    pub base_url: String,
    /// API key (supports `${ENV_VAR}` substitution).
    pub api_key: String,
    /// Protocol used by this provider (default: openai).
    #[serde(default)]
    pub protocol: ImageProtocol,
    /// Call mode, only effective for `Dashscope` protocol (default: sync).
    #[serde(default)]
    pub mode: ImageCallMode,
    /// Available models under this provider.
    pub models: Vec<ImageModelConfig>,
    /// Polling interval in seconds for async mode (default: 3).
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    /// Total polling timeout in seconds for async mode (default: 300).
    #[serde(default = "default_poll_timeout")]
    pub poll_timeout_secs: u64,
}

fn default_poll_interval() -> u64 {
    3
}

fn default_poll_timeout() -> u64 {
    300
}

// ============================================================================
// Application config (unchanged from previous version)
// ============================================================================

#[derive(Debug, Deserialize, Default)]
pub struct AppConfig {
    pub log_level: Option<String>,
    /// Whether to log to file (default: false).
    pub log_file: Option<bool>,
    /// Days of daily log files to keep. On startup, `robit-YYYY-MM-DD.log`
    /// files older than this are deleted. `None` = default 14 days; `Some(0)`
    /// disables cleanup (keep all). Only `robit-*.log` files are touched.
    pub log_retention_days: Option<u32>,
    pub max_steps: Option<usize>,
    pub enabled_tools: Option<Vec<String>>,
    pub enabled_skills: Option<Vec<String>>,
    pub context: Option<ContextConfig>,
    pub retry: Option<RetryConfig>,
    pub auto_approve: Option<bool>,
    pub global_storage: Option<bool>,
    /// Bot platform settings (shared across Bot frontends).
    pub bot: Option<BotConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContextConfig {
    pub max_output_lines: Option<usize>,
    pub max_output_bytes: Option<usize>,
    pub reserve_ratio: Option<f32>,
    /// Fraction of max_tokens at which truncation triggers (default 0.7).
    /// Lower = earlier truncation, more headroom for estimation errors.
    pub truncation_ratio: Option<f32>,
    /// Minimum conversation rounds to keep after truncation (default 3).
    /// Prevents losing all recent context when truncation is aggressive.
    pub min_keep_rounds: Option<usize>,
    /// Safety multiplier applied to token estimates (default 1.3).
    /// Compensates for heuristic underestimation vs actual tokenizer counts.
    pub token_safety_margin: Option<f32>,
    /// Token threshold for triggering compression (default 5000).
    /// Only compress when removed messages exceed this token count.
    pub compression_token_threshold: Option<usize>,
    /// Enable/disable context compression (default true).
    pub compression_enabled: Option<bool>,
    /// Maximum tool calls allowed per turn before forcing early termination (default 30).
    /// Prevents a single user turn from exploding the context with excessive tool calls.
    pub max_tool_calls_per_turn: Option<usize>,
    /// Enable progressive segmented compression (default true).
    /// When false, falls back to the old single-shot truncation + one summary behavior.
    pub progressive_compression: Option<bool>,
    /// Number of full conversation rounds per summary segment (default 3).
    /// Each compression converts the oldest N full rounds into one summary segment.
    pub rounds_per_summary: Option<usize>,
    /// Maximum number of summary segments to keep (default 5).
    /// When exceeded, the oldest segments are merged (or discarded if merge limit reached).
    pub max_summary_segments: Option<usize>,
    /// Number of segments to merge each time (default 2).
    pub merge_count: Option<usize>,
    /// Maximum times a single summary segment may be merged before being discarded (default 2).
    /// Controls information distortion — each merge loses detail; discard when limit is hit.
    pub max_merges_per_segment: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct RetryConfig {
    pub max_retries: Option<u32>,
    pub initial_backoff_ms: Option<u64>,
    pub max_backoff_ms: Option<u64>,
}

// ============================================================================
// Communication channels config (QQ Bot, Feishu, etc.)
// ============================================================================

/// Communication channel configurations, separate from LLM `providers`.
#[derive(Debug, Deserialize, Default)]
pub struct ChannelsConfig {
    /// QQ Official Bot channel.
    pub qq_bot: Option<QqBotConfig>,
}

/// QQ Official Bot credentials (from `[channels.qq_bot]`).
#[derive(Debug, Deserialize, Clone)]
pub struct QqBotConfig {
    pub app_id: String,
    pub app_secret: String,
}

// ============================================================================
// Bot platform app config
// ============================================================================

/// Shared Bot platform settings under `[app.bot]`.
#[derive(Debug, Deserialize, Default)]
pub struct BotConfig {
    /// Timeout (seconds) for waiting on a tool confirmation reply.
    pub confirm_timeout_secs: Option<u64>,
    /// Idle session expiry (minutes) before cleanup.
    pub session_timeout_minutes: Option<u64>,
    /// Custom confirm/reject keywords.
    pub confirm_keywords: Option<ConfirmKeywordsConfig>,
}

/// Confirm/reject keyword lists for inline tool confirmation.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct ConfirmKeywordsConfig {
    pub approve: Option<Vec<String>>,
    pub reject: Option<Vec<String>>,
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
    /// Whether this model supports image inputs.
    pub supports_images: bool,
    /// Whether this model supports tool calling.
    pub supports_tools: bool,
}

// ============================================================================
// Loader
// ============================================================================

/// Returns the ~/.robit/ directory path.
fn robit_home() -> Result<PathBuf, LlmError> {
    let home = dirs::home_dir()
        .ok_or_else(|| LlmError::ConfigError("Cannot determine home directory".to_string()))?;
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
/// Automatically loads .env files before resolving `${ENV_VAR}` patterns:
///   1. `~/.robit/.env` (global, lower priority)
///   2. `workdir/.robit/.env` (project-local, higher priority)
///
/// Search order for config.toml:
///   1. `workdir/.robit/config.toml` (project-local, if workdir provided)
///   2. `cwd/.robit/config.toml` (project-local, if workdir not provided)
///   3. `~/.robit/config.toml`   (global fallback)
pub fn load_config(workdir: Option<&std::path::Path>) -> Result<RobitConfig, LlmError> {
    // Load .env first so ${ENV_VAR} substitutions work
    load_env_from(workdir);

    let path = find_config_path(workdir)?;

    let content = std::fs::read_to_string(&path)
        .map_err(|e| LlmError::ConfigError(format!("Failed to read {}: {}", path.display(), e)))?;

    let mut config: RobitConfig = toml::from_str(&content)
        .map_err(|e| LlmError::ConfigError(format!("Failed to parse config.toml: {}", e)))?;

    // Resolve environment variables in api_key fields
    for provider in config.providers.values_mut() {
        provider.api_key = resolve_env_var(&provider.api_key);
    }

    // Resolve env vars in image provider configs
    for provider in config.image_providers.values_mut() {
        provider.api_key = resolve_env_var(&provider.api_key);
    }

    // Also resolve env vars in channel configs
    if let Some(ref mut channels) = config.channels {
        if let Some(ref mut qq_bot) = channels.qq_bot {
            qq_bot.app_id = resolve_env_var(&qq_bot.app_id);
            qq_bot.app_secret = resolve_env_var(&qq_bot.app_secret);
        }
    }

    Ok(config)
}

/// Load .env files in order: workdir first (higher priority), then global (lower priority).
/// Workdir vars will override global vars.
pub fn load_env_from(workdir: Option<&std::path::Path>) {
    // Collect all env paths, workdir first (higher priority)
    let mut env_paths = Vec::new();

    // Workdir-specific .env (highest priority)
    if let Some(workdir) = workdir {
        let local_env = workdir.join(".robit").join(".env");
        if local_env.exists() {
            env_paths.push(local_env);
        }
    } else if let Ok(cwd) = std::env::current_dir() {
        let local_env = cwd.join(".robit").join(".env");
        if local_env.exists() {
            env_paths.push(local_env);
        }
    }

    // Global .env (lowest priority)
    if let Ok(robit_dir) = robit_home() {
        let env_path = robit_dir.join(".env");
        if env_path.exists() {
            env_paths.push(env_path);
        }
    }

    // Load in reverse order (global first, then workdir) so workdir overrides global
    // Use dotenvy::from_path_iter to load and manually set vars to enable overriding
    for path in env_paths.iter().rev() {
        if let Ok(iter) = dotenvy::from_path_iter(path) {
            for item in iter {
                if let Ok((key, value)) = item {
                    std::env::set_var(key, value);
                }
            }
        }
    }
}

/// Load .env from ~/.robit/.env if it exists (deprecated, use load_env_from).
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
            LlmError::ConfigError(format!("Provider '{}' has no models defined", name))
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
            LlmError::ConfigError(format!("Provider '{}' has no models defined", key))
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
    let model = provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .ok_or_else(|| {
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
        supports_images: model.supports_images.unwrap_or(false),
        supports_tools: model.supports_tools.unwrap_or(false),
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
// Image provider resolution
// ============================================================================

/// A fully resolved image generation provider, ready for client construction.
///
/// Resolved from `default_image_model` (in "provider/model" format) together
/// with the matching `ImageProviderConfig`.
#[derive(Debug, Clone)]
pub struct ResolvedImageProvider {
    /// Provider key in config (e.g. "wanxiang").
    pub provider_name: String,
    /// Model ID parsed from `default_image_model` (e.g. "wan2.7-image-pro").
    pub model_id: String,
    /// API base URL.
    pub base_url: String,
    /// API key (env vars already resolved).
    pub api_key: String,
    /// Protocol used by this provider.
    pub protocol: ImageProtocol,
    /// Call mode (only effective for DashScope).
    pub mode: ImageCallMode,
    /// Polling interval in seconds for async mode.
    pub poll_interval_secs: u64,
    /// Total polling timeout in seconds for async mode.
    pub poll_timeout_secs: u64,
}

/// Resolve the image generation provider to use.
///
/// Requires `default_image_model` ("provider/model" format) to be configured.
/// Without it, image generation is considered disabled and the tool is not
/// registered.
///
/// Returns an error if no image providers are configured, `default_image_model`
/// is absent, the referenced provider/model is not found, or the API key is
/// empty.
pub fn resolve_image_provider(config: &RobitConfig) -> Result<ResolvedImageProvider, LlmError> {
    if config.image_providers.is_empty() {
        return Err(LlmError::ConfigError(
            "No image providers defined in config.toml".to_string(),
        ));
    }

    // default_image_model is required - without it we don't know which
    // provider/model to use, so image generation is considered disabled.
    let default = config.default_image_model.as_ref().ok_or_else(|| {
        LlmError::ConfigError(
            "default_image_model is not configured. Set it to \"provider/model\" \
             (e.g. \"wanxiang/wan2.7-image-pro\") to enable image generation."
                .to_string(),
        )
    })?;

    let (provider_key, model_id) = parse_default_model(default)?;

    let provider = config.image_providers.get(&provider_key).ok_or_else(|| {
        let available: Vec<&str> = config.image_providers.keys().map(|s| s.as_str()).collect();
        LlmError::ConfigError(format!(
            "Image provider '{}' is not defined in config.toml. Available image providers: {:?}",
            provider_key, available
        ))
    })?;

    // Validate that the model exists in this provider
    let model_exists = provider.models.iter().any(|m| m.id == model_id);
    if !model_exists {
        let available: Vec<&str> = provider.models.iter().map(|m| m.id.as_str()).collect();
        return Err(LlmError::ConfigError(format!(
            "Image model '{}' not found in provider '{}'. Available models: {:?}",
            model_id, provider_key, available
        )));
    }

    // Validate API key
    if provider.api_key.is_empty() || provider.api_key.starts_with("${") {
        return Err(LlmError::ConfigError(format!(
            "Image provider '{}' API key is not configured or the environment variable is not set",
            provider_key
        )));
    }

    Ok(ResolvedImageProvider {
        provider_name: provider_key,
        model_id,
        base_url: provider.base_url.clone(),
        api_key: provider.api_key.clone(),
        protocol: provider.protocol.clone(),
        mode: provider.mode.clone(),
        poll_interval_secs: provider.poll_interval_secs,
        poll_timeout_secs: provider.poll_timeout_secs,
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
            global_storage = true

            [app.context]
            max_output_lines = 500
            reserve_ratio = 0.2

            [app.retry]
            max_retries = 3
        "#;

        let config: RobitConfig = toml::from_str(toml_str).unwrap();

        // Default model
        assert_eq!(
            config.default_model.as_deref(),
            Some("deepseek/deepseek-chat")
        );

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
        assert_eq!(app.global_storage, Some(true));
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
        assert_eq!(
            resolved.base_url,
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        );
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
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid default_model"));
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

    #[test]
    fn test_parse_channels_and_bot_sections() {
        let toml_str = r#"
            default_model = "deepseek/deepseek-chat"

            [providers.deepseek]
            base_url = "https://api.deepseek.com"
            api_key = "sk-test"

            [[providers.deepseek.models]]
            id = "deepseek-chat"

            [channels.qq_bot]
            app_id = "123456789"
            app_secret = "secret-value"

            [app.bot]
            confirm_timeout_secs = 60
            session_timeout_minutes = 30

            [app.bot.confirm_keywords]
            approve = ["确认", "yes"]
            reject = ["取消", "no"]
        "#;

        let config: RobitConfig = toml::from_str(toml_str).unwrap();

        // channels.qq_bot
        let qq = config
            .channels
            .as_ref()
            .and_then(|c| c.qq_bot.as_ref())
            .expect("qq_bot config missing");
        assert_eq!(qq.app_id, "123456789");
        assert_eq!(qq.app_secret, "secret-value");

        // app.bot
        let bot = config.app.as_ref().unwrap().bot.as_ref().unwrap();
        assert_eq!(bot.confirm_timeout_secs, Some(60));
        assert_eq!(bot.session_timeout_minutes, Some(30));
        let kw = bot.confirm_keywords.as_ref().unwrap();
        assert_eq!(kw.approve.as_ref().unwrap(), &vec!["确认".to_string(), "yes".to_string()]);
        assert_eq!(kw.reject.as_ref().unwrap(), &vec!["取消".to_string(), "no".to_string()]);
    }

    #[test]
    fn test_config_without_channels_still_parses() {
        let toml_str = r#"
            [providers.deepseek]
            base_url = "https://api.deepseek.com"
            api_key = "sk-test"

            [[providers.deepseek.models]]
            id = "deepseek-chat"
        "#;

        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        assert!(config.channels.is_none());
        assert!(config.app.is_none() || config.app.as_ref().unwrap().bot.is_none());
    }

    // ------------------------------------------------------------------
    // Image provider config tests
    // ------------------------------------------------------------------

    fn make_image_test_config() -> RobitConfig {
        let toml_str = r#"
            default_image_model = "wanxiang/wan2.7-image-pro"

            [providers.test]
            base_url = "https://api.test.com"
            api_key = "sk-test"

            [[providers.test.models]]
            id = "test-model"

            [image_providers.wanxiang]
            name = "通义万相"
            base_url = "https://ws.cn-beijing.maas.aliyuncs.com"
            api_key = "sk-test"
            protocol = "dashscope"
            mode = "async"

            [[image_providers.wanxiang.models]]
            id = "wan2.7-image-pro"
            name = "万相2.7 Pro"

            [[image_providers.wanxiang.models]]
            id = "wan2.7-image"

            [image_providers.dalle]
            base_url = "https://api.openai.com/v1"
            api_key = "sk-openai"

            [[image_providers.dalle.models]]
            id = "dall-e-3"
        "#;
        toml::from_str(toml_str).unwrap()
    }

    #[test]
    fn test_parse_image_providers() {
        let config = make_image_test_config();

        assert_eq!(
            config.default_image_model.as_deref(),
            Some("wanxiang/wan2.7-image-pro")
        );
        assert_eq!(config.image_providers.len(), 2);

        let wx = &config.image_providers["wanxiang"];
        assert_eq!(wx.name.as_deref(), Some("通义万相"));
        assert_eq!(wx.base_url, "https://ws.cn-beijing.maas.aliyuncs.com");
        assert_eq!(wx.api_key, "sk-test");
        assert_eq!(wx.protocol, ImageProtocol::Dashscope);
        assert_eq!(wx.mode, ImageCallMode::Async);
        assert_eq!(wx.poll_interval_secs, 3);
        assert_eq!(wx.poll_timeout_secs, 300);
        assert_eq!(wx.models.len(), 2);
        assert_eq!(wx.models[0].id, "wan2.7-image-pro");

        // Defaults: openai protocol + sync mode
        let dalle = &config.image_providers["dalle"];
        assert_eq!(dalle.protocol, ImageProtocol::Openai);
        assert_eq!(dalle.mode, ImageCallMode::Sync);
    }

    #[test]
    fn test_resolve_image_provider_from_default() {
        let config = make_image_test_config();
        let resolved = resolve_image_provider(&config).unwrap();
        assert_eq!(resolved.provider_name, "wanxiang");
        assert_eq!(resolved.model_id, "wan2.7-image-pro");
        assert_eq!(resolved.base_url, "https://ws.cn-beijing.maas.aliyuncs.com");
        assert_eq!(resolved.protocol, ImageProtocol::Dashscope);
        assert_eq!(resolved.mode, ImageCallMode::Async);
    }

    #[test]
    fn test_resolve_image_provider_no_default_model() {
        // image_providers configured but default_image_model absent -> error
        // (image generation is considered disabled in this case)
        let toml_str = r#"
            [providers.test]
            base_url = "https://api.test.com"
            api_key = "sk-test"

            [[providers.test.models]]
            id = "test-model"

            [image_providers.wanxiang]
            base_url = "https://ws.cn-beijing.maas.aliyuncs.com"
            api_key = "sk-test"

            [[image_providers.wanxiang.models]]
            id = "wan2.7-image-pro"
        "#;
        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        assert!(resolve_image_provider(&config).is_err());
    }

    #[test]
    fn test_resolve_image_provider_none_configured() {
        let toml_str = r#"
            [providers.deepseek]
            base_url = "https://api.deepseek.com"
            api_key = "sk-test"

            [[providers.deepseek.models]]
            id = "deepseek-chat"
        "#;
        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        assert!(resolve_image_provider(&config).is_err());
    }

    #[test]
    fn test_resolve_image_provider_empty_api_key() {
        let toml_str = r#"
            default_image_model = "wanxiang/wan2.7-image-pro"

            [providers.test]
            base_url = "https://api.test.com"
            api_key = "sk-test"

            [[providers.test.models]]
            id = "test-model"

            [image_providers.wanxiang]
            base_url = "https://ws.cn-beijing.maas.aliyuncs.com"
            api_key = ""

            [[image_providers.wanxiang.models]]
            id = "wan2.7-image-pro"
        "#;
        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        assert!(resolve_image_provider(&config).is_err());
    }

    #[test]
    fn test_resolve_image_provider_model_not_found() {
        let toml_str = r#"
            default_image_model = "wanxiang/nonexistent-model"

            [providers.test]
            base_url = "https://api.test.com"
            api_key = "sk-test"

            [[providers.test.models]]
            id = "test-model"

            [image_providers.wanxiang]
            base_url = "https://ws.cn-beijing.maas.aliyuncs.com"
            api_key = "sk-test"

            [[image_providers.wanxiang.models]]
            id = "wan2.7-image-pro"
        "#;
        let config: RobitConfig = toml::from_str(toml_str).unwrap();
        assert!(resolve_image_provider(&config).is_err());
    }

    #[test]
    fn test_resolve_image_provider_env_var_substitution() {
        std::env::set_var("ROBIT_IMG_TEST_KEY", "sk-from-env");
        let toml_str = r#"
            default_image_model = "wanxiang/wan2.7-image-pro"

            [providers.test]
            base_url = "https://api.test.com"
            api_key = "sk-test"

            [[providers.test.models]]
            id = "test-model"

            [image_providers.wanxiang]
            base_url = "https://ws.cn-beijing.maas.aliyuncs.com"
            api_key = "${ROBIT_IMG_TEST_KEY}"

            [[image_providers.wanxiang.models]]
            id = "wan2.7-image-pro"
        "#;
        // load_config resolves env vars; here we test resolve_image_provider
        // after manual substitution (load_config path is covered elsewhere).
        let mut config: RobitConfig = toml::from_str(toml_str).unwrap();
        for provider in config.image_providers.values_mut() {
            provider.api_key = resolve_env_var(&provider.api_key);
        }
        let resolved = resolve_image_provider(&config).unwrap();
        assert_eq!(resolved.api_key, "sk-from-env");
        std::env::remove_var("ROBIT_IMG_TEST_KEY");
    }
}
