// pub const LICENSE: &str = include_str!("../../../LICENSE");

use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LLMConfig {
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u16,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
}

fn default_max_tokens() -> u16 {
    4096
}

fn default_temperature() -> f32 {
    1.0
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LLMConfigOpt {
    model: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
    max_tokens: Option<u16>,
    temperature: Option<f32>,
}

impl LLMConfigOpt {
    pub fn into_config(&self, config: &LLMConfig) -> LLMConfig {
        LLMConfig {
            model: self.model.as_ref().cloned().unwrap_or_else(|| config.model.clone()),
            base_url: self.base_url.as_ref().cloned().unwrap_or_else(|| config.base_url.clone()),
            api_key: self.api_key.as_ref().cloned().unwrap_or_else(|| config.api_key.clone()),
            max_tokens: self.max_tokens.unwrap_or(config.max_tokens),
            temperature: self.temperature.unwrap_or(config.temperature),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LLMs {
    default: LLMConfig,
    chat: Option<LLMConfigOpt>,
    reasoner: Option<LLMConfigOpt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub log_level: String,
    pub max_steps: u16,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            log_level: "INFO".to_string(),
            max_steps: 10,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    llm: LLMs,
    #[serde(default)]
    app: AppConfig,
}

impl Config {
    pub fn llm(&self) -> &LLMConfig {
        &self.llm.default
    }

    pub fn llm_chat(&self) -> LLMConfig {
        if let Some(ref chat_llm) = self.llm.chat {
            chat_llm.into_config(&self.llm.default)
        } else {
            self.llm.default.clone()
        }
    }

    pub fn llm_reasoner(&self) -> LLMConfig {
        if let Some(ref reasoner_llm) = self.llm.reasoner {
            reasoner_llm.into_config(&self.llm.default)
        } else {
            self.llm.default.clone()
        }
    }

    pub fn app(&self) -> &AppConfig {
        &self.app
    }
}

pub fn load_config() -> Result<Config> {
    let paths: Vec<&Path> = if cfg!(debug_assertions) || cfg!(test) {
        vec![
            Path::new("robit.toml"),
            Path::new("config/robit.toml"),
            Path::new("../../config/robit.toml"),
        ]
    } else {
        vec![
            Path::new("robit.toml"),
            Path::new("config/robit.toml"),
        ]
    };

    let config_path = paths.iter().find(|p| p.exists())
        .ok_or_else(|| anyhow::anyhow!("robit.toml not found in current or config directory"))?;

    let content = fs::read_to_string(config_path)?;
    let config: Config = toml::from_str(&content)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use std::env;

    use super::*;

    #[test]
    fn test_load() {
        println!("Current directory: {}", env::current_dir().unwrap().display());

        let config = load_config().unwrap();
        println!("{:#?}", config);
    }
}
