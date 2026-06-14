use std::path::Path;

use crate::events::ConfigInfo;

/// Build ConfigInfo from loaded configuration.
pub fn build_config_info(config: &robit_ai::config::RobitConfig, working_dir: &Path) -> ConfigInfo {
    let model = config
        .default_model
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    let auto_approve = config
        .app
        .as_ref()
        .and_then(|a| a.auto_approve)
        .unwrap_or(false);

    ConfigInfo {
        model,
        version: env!("CARGO_PKG_VERSION").to_string(),
        tools_enabled: 0,
        tools_total: 0,
        skills_enabled: 0,
        skills_total: 0,
        auto_approve,
        working_dir: working_dir.display().to_string(),
    }
}
